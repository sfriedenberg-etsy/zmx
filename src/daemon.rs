//! Daemon side of a session: owns the PTY and the terminal model, accepts
//! client connections, broadcasts PTY output, and applies client input.

use std::io;
use std::os::fd::RawFd;
use std::path::Path;
use std::time::Duration;

use crate::config::Cfg;
use crate::ipc::{self, Resize, Tag};
use crate::logger::{self, log_debug, log_err, log_info, log_warn};
use crate::names;
use crate::pty;
use crate::session::{self, close_fd};
use crate::title::TitleTracker;
use crate::vt;

pub struct ClientConn {
    pub fd: RawFd,
    pub has_pending_output: bool,
    pub read_buf: ipc::SocketBuffer,
    pub write_buf: Vec<u8>,
    // Last terminal size this client reported (via Init/Resize). A zero
    // dimension means "not yet reported" and is ignored when computing the
    // shared PTY size. See min_client_size / Daemon::apply_client_size.
    pub cols: u16,
    pub rows: u16,
}

impl ClientConn {
    fn new(fd: RawFd) -> ClientConn {
        ClientConn {
            fd,
            has_pending_output: false,
            read_buf: ipc::SocketBuffer::new(),
            write_buf: Vec::with_capacity(4096),
            cols: 0,
            rows: 0,
        }
    }
}

impl Drop for ClientConn {
    fn drop(&mut self) {
        close_fd(self.fd);
    }
}

/// Fold one client's reported size into the running elementwise minimum.
/// Sizes with a zero dimension are "not yet reported" and ignored, so a
/// freshly-accepted client that hasn't sent its size doesn't collapse the
/// PTY to 0x0.
fn fold_min_size(acc: Option<Resize>, s: Resize) -> Option<Resize> {
    if s.cols == 0 || s.rows == 0 {
        return acc;
    }
    match acc {
        None => Some(s),
        Some(a) => Some(Resize {
            rows: a.rows.min(s.rows),
            cols: a.cols.min(s.cols),
        }),
    }
}

/// Elementwise minimum across a list of reported sizes, or None if none have
/// been reported yet.
#[cfg(test)]
pub fn min_size(sizes: &[Resize]) -> Option<Resize> {
    sizes.iter().fold(None, |acc, &s| fold_min_size(acc, s))
}

/// Elementwise minimum size across all attached clients (tmux
/// `window-size smallest`): the PTY is sized so every active client can
/// render it without truncation. None when no client has reported a size.
fn min_client_size(clients: &[ClientConn]) -> Option<Resize> {
    clients.iter().fold(None, |acc, cl| {
        fold_min_size(
            acc,
            Resize {
                rows: cl.rows,
                cols: cl.cols,
            },
        )
    })
}

pub struct Daemon<'a> {
    pub cfg: &'a Cfg,
    pub clients: Vec<ClientConn>,
    pub session_name: String,
    pub socket_path: String,
    pub running: bool,
    pub pid: libc::pid_t,
    pub command: Option<Vec<String>>,
    pub cwd: String,
    pub has_pty_output: bool,
    pub has_had_client: bool,
    // Tracks the latest OSC 0/2 window title from PTY output so it can be
    // re-emitted on re-attach (issue #6).
    pub title_tracker: TitleTracker,
    // PTY master fd and active terminal model, wired up once the daemon loop
    // starts. Stored so client teardown (close_client) can recompute the
    // shared window size after a detach (issue #8).
    pub pty_fd: RawFd,
    pub term: Option<vt::Terminal>,
}

impl<'a> Daemon<'a> {
    pub fn new(
        cfg: &'a Cfg,
        session_name: &str,
        command: Option<Vec<String>>,
        cwd: String,
    ) -> Result<Daemon<'a>, String> {
        let socket_path = names::socket_path(&cfg.socket_dir, session_name)?;
        log_info!("socket path={socket_path}");
        Ok(Daemon {
            cfg,
            clients: Vec::with_capacity(10),
            session_name: session_name.to_string(),
            socket_path,
            running: true,
            pid: 0,
            command,
            cwd,
            has_pty_output: false,
            has_had_client: false,
            title_tracker: TitleTracker::new(),
            pty_fd: -1,
            term: None,
        })
    }

    fn shutdown(&mut self) {
        log_info!("shutting down daemon session_name={}", self.session_name);
        self.running = false;
        self.clients.clear();
    }

    /// Remove client `i`. Returns true when this was the last client and
    /// `shutdown_on_last` asked for a shutdown.
    fn close_client(&mut self, i: usize, shutdown_on_last: bool) -> bool {
        let fd = self.clients[i].fd;
        self.clients.remove(i);
        log_info!(
            "client disconnected fd={fd} remaining={}",
            self.clients.len()
        );
        if shutdown_on_last && self.clients.is_empty() {
            self.shutdown();
            return true;
        }
        // A client left; resize the PTY to the new minimum across the
        // remaining clients so the session grows back when the smallest
        // client detaches (issue #8).
        self.apply_client_size();
        false
    }

    /// Resize the PTY (and terminal model) to the elementwise-minimum size
    /// across all attached clients, so every active client can render the
    /// session without truncation (tmux `window-size smallest`). No-op until
    /// the daemon loop has wired up pty_fd/term, or while no client has
    /// reported a size.
    fn apply_client_size(&mut self) {
        let Some(term) = self.term.as_mut() else {
            return;
        };
        let Some(size) = min_client_size(&self.clients) else {
            return;
        };
        pty::set_pty_size(self.pty_fd, size);
        term.resize(size.cols, size.rows);
    }

    fn handle_input(&mut self, payload: &[u8]) {
        if !payload.is_empty() {
            let _ = pty::write_fd(self.pty_fd, payload);
        }
    }

    fn handle_init(&mut self, i: usize, payload: &[u8]) {
        let Some(resize) = Resize::decode(payload) else {
            return;
        };

        // Record this client's size and resize the PTY to the minimum across
        // all attached clients (issue #8), rather than unconditionally
        // adopting the newest client's size.
        self.clients[i].cols = resize.cols;
        self.clients[i].rows = resize.rows;
        self.apply_client_size();

        // Serialize terminal state BEFORE the shell's SIGWINCH-triggered
        // redraw lands. Only serialize on re-attach (has_had_client), not
        // first attach, to avoid interfering with shell initialization
        // (DA1 queries, etc.)
        if self.has_pty_output && self.has_had_client {
            if let Some(term) = self.term.as_ref() {
                let cursor = term.cursor();
                log_debug!(
                    "cursor before serialize: x={} y={} pending_wrap={}",
                    cursor.x,
                    cursor.y,
                    cursor.pending_wrap
                );
                if let Some(output) = term.serialize_state() {
                    log_debug!("serialize terminal state");
                    ipc::append_message(&mut self.clients[i].write_buf, Tag::Output, &output);
                    self.clients[i].has_pending_output = true;
                }
            }

            // Restore the window title (issue #6). The shell emitted its OSC
            // 0/2 sequence in the past; the terminal model doesn't serialize
            // it, so re-emit the last captured title for this re-attaching
            // client so the real terminal's title is restored.
            if let Some(t) = self.title_tracker.current() {
                let mut seq = Vec::with_capacity(t.text.len() + 8);
                seq.extend_from_slice(format!("\x1b]{};", t.code).as_bytes());
                seq.extend_from_slice(t.text);
                seq.push(0x07);
                log_debug!("restore window title len={}", t.text.len());
                ipc::append_message(&mut self.clients[i].write_buf, Tag::Output, &seq);
                self.clients[i].has_pending_output = true;
            }
        }

        // Mark that we've had a client init, so subsequent clients get
        // terminal state.
        self.has_had_client = true;

        log_debug!("init resize rows={} cols={}", resize.rows, resize.cols);
    }

    fn handle_resize(&mut self, i: usize, payload: &[u8]) {
        let Some(resize) = Resize::decode(payload) else {
            return;
        };
        // Update this client's reported size and re-apply the shared minimum
        // across all attached clients (issue #8).
        self.clients[i].cols = resize.cols;
        self.clients[i].rows = resize.rows;
        self.apply_client_size();
        log_debug!("resize rows={} cols={}", resize.rows, resize.cols);
    }

    fn handle_detach(&mut self, i: usize) {
        log_info!("client detach fd={}", self.clients[i].fd);
        self.close_client(i, false);
    }

    fn handle_detach_all(&mut self) {
        log_info!("detach all clients={}", self.clients.len());
        self.clients.clear();
    }

    pub fn handle_kill(&mut self) {
        log_info!("kill received session={}", self.session_name);
        self.shutdown();
        // Gracefully shut down shell processes; shells tend to ignore SIGTERM
        // so we send SIGHUP instead
        //   https://www.gnu.org/software/bash/manual/html_node/Signals.html
        // Negative pid means kill process and children.
        log_info!(
            "sending SIGHUP session={} pid={}",
            self.session_name,
            self.pid
        );
        if unsafe { libc::kill(-self.pid, libc::SIGHUP) } < 0 {
            log_warn!(
                "failed to send SIGHUP to pty child err={}",
                io::Error::last_os_error()
            );
        }
        std::thread::sleep(Duration::from_millis(500));
        if unsafe { libc::kill(-self.pid, libc::SIGKILL) } < 0 {
            log_warn!(
                "failed to send SIGKILL to pty child err={}",
                io::Error::last_os_error()
            );
        }
    }

    fn handle_info(&mut self, i: usize) {
        let clients_len = (self.clients.len() - 1) as u64;

        let cmd: Vec<u8> = match &self.command {
            Some(args) => {
                let joined = args.join(" ");
                let mut bytes = joined.into_bytes();
                bytes.truncate(ipc::MAX_CMD_LEN);
                bytes
            }
            None => Vec::new(),
        };
        let mut cwd = self.cwd.clone().into_bytes();
        cwd.truncate(ipc::MAX_CWD_LEN);

        let info = ipc::Info {
            clients_len,
            pid: self.pid,
            cmd,
            cwd,
        };
        ipc::append_message(&mut self.clients[i].write_buf, Tag::Info, &info.encode());
        self.clients[i].has_pending_output = true;
    }

    fn handle_history(&mut self, i: usize, payload: &[u8]) {
        let format = if payload.is_empty() {
            vt::Format::Plain
        } else {
            vt::Format::from_u8(payload[0])
        };
        let output = self
            .term
            .as_ref()
            .and_then(|term| term.serialize(format))
            .unwrap_or_default();
        ipc::append_message(&mut self.clients[i].write_buf, Tag::History, &output);
        self.clients[i].has_pending_output = true;
    }

    fn handle_run(&mut self, i: usize, payload: &[u8]) {
        if !payload.is_empty() {
            let _ = pty::write_fd(self.pty_fd, payload);
        }
        ipc::append_message(&mut self.clients[i].write_buf, Tag::Ack, b"");
        self.clients[i].has_pending_output = true;
        self.has_had_client = true;
        log_debug!("run command len={}", payload.len());
    }
}

pub struct EnsureSessionResult {
    pub created: bool,
    pub is_daemon: bool,
}

/// Make sure the session exists, forking a daemon process for it when
/// needed. In the daemon child this runs the full session lifecycle and
/// returns with `is_daemon = true` once the session ends.
pub fn ensure_session(daemon: &mut Daemon) -> Result<EnsureSessionResult, String> {
    let encoded_name = names::encode_session_name(&daemon.session_name);

    let exists = session::session_exists(&daemon.cfg.socket_dir, &encoded_name)
        .map_err(|err| err.to_string())?;
    let mut should_create = !exists;

    if exists {
        match session::probe_session(&daemon.socket_path) {
            Ok(result) => {
                close_fd(result.fd);
                if daemon.command.is_some() {
                    log_warn!(
                        "session already exists, ignoring command session={}",
                        daemon.session_name
                    );
                }
            }
            Err(_) => {
                session::cleanup_stale_socket(&daemon.cfg.socket_dir, &encoded_name);
                should_create = true;
            }
        }
    }

    if should_create {
        log_info!("creating session={}", daemon.session_name);
        let server_sock_fd =
            session::create_socket(&daemon.socket_path).map_err(|err| err.to_string())?;

        let pid = unsafe { libc::fork() };
        if pid < 0 {
            close_fd(server_sock_fd);
            return Err(io::Error::last_os_error().to_string());
        }
        if pid == 0 {
            // Child (daemon).
            run_daemon_child(daemon, server_sock_fd, &encoded_name)?;
            return Ok(EnsureSessionResult {
                created: true,
                is_daemon: true,
            });
        }
        close_fd(server_sock_fd);
        std::thread::sleep(Duration::from_millis(10));
        return Ok(EnsureSessionResult {
            created: true,
            is_daemon: false,
        });
    }

    Ok(EnsureSessionResult {
        created: false,
        is_daemon: false,
    })
}

fn run_daemon_child(
    daemon: &mut Daemon,
    server_sock_fd: RawFd,
    encoded_name: &str,
) -> Result<(), String> {
    unsafe {
        libc::setsid();
    }

    // Detach from the parent's stdio so non-TTY callers (scripts, CI,
    // pipes) don't block waiting for EOF on a fd the daemon still holds.
    // Without this, `zmx attach --detach` works from an interactive
    // terminal but hangs the calling process when run from a pipe.
    unsafe {
        let devnull = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
        if devnull >= 0 {
            libc::dup2(devnull, libc::STDIN_FILENO);
            libc::dup2(devnull, libc::STDOUT_FILENO);
            libc::dup2(devnull, libc::STDERR_FILENO);
            libc::close(devnull);
        }
    }

    logger::deinit();
    let session_log_path = format!("{}/{}.log", daemon.cfg.log_dir, encoded_name);
    logger::init(&session_log_path).map_err(|err| err.to_string())?;

    let socket_file = Path::new(&daemon.cfg.socket_dir).join(encoded_name);
    let cleanup = |server_sock_fd: RawFd| {
        close_fd(server_sock_fd);
        let _ = std::fs::remove_file(&socket_file);
    };

    let command: Vec<String> = daemon.command.clone().unwrap_or_default();
    let (pty_fd, pid) = match pty::spawn_pty(&daemon.session_name, &daemon.cfg.group, &command) {
        Ok(v) => v,
        Err(err) => {
            cleanup(server_sock_fd);
            return Err(err.to_string());
        }
    };
    daemon.pid = pid;
    log_info!("pty spawned session={} pid={}", daemon.session_name, pid);

    let loop_result = daemon_loop(daemon, server_sock_fd, pty_fd);
    daemon.handle_kill();
    unsafe {
        let mut status: libc::c_int = 0;
        libc::waitpid(daemon.pid, &mut status, 0);
    }
    close_fd(pty_fd);
    log_info!("deleting socket file session_name={}", daemon.session_name);
    cleanup(server_sock_fd);
    loop_result
}

fn daemon_loop(daemon: &mut Daemon, server_sock_fd: RawFd, pty_fd: RawFd) -> Result<(), String> {
    log_info!(
        "daemon started session={} pty_fd={pty_fd}",
        daemon.session_name
    );
    pty::setup_sigterm_handler();

    let init_size = pty::get_terminal_size(pty_fd);
    let term = vt::Terminal::new(init_size.cols, init_size.rows, daemon.cfg.max_scrollback);

    // Wire the PTY fd and terminal model onto the daemon so client teardown
    // can recompute the shared window size after a detach (issue #8).
    daemon.pty_fd = pty_fd;
    daemon.term = Some(term);

    'daemon_loop: while daemon.running {
        if pty::SIGTERM_RECEIVED.swap(false, std::sync::atomic::Ordering::AcqRel) {
            log_info!(
                "SIGTERM received, shutting down gracefully session={}",
                daemon.session_name
            );
            break 'daemon_loop;
        }

        let mut poll_fds = Vec::with_capacity(2 + daemon.clients.len());
        poll_fds.push(pty::pollfd(server_sock_fd, libc::POLLIN));
        poll_fds.push(pty::pollfd(pty_fd, libc::POLLIN));
        for client in &daemon.clients {
            let mut events = libc::POLLIN;
            if client.has_pending_output {
                events |= libc::POLLOUT;
            }
            poll_fds.push(pty::pollfd(client.fd, events));
        }

        match pty::poll(&mut poll_fds, -1) {
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err.to_string()),
        }

        const ERRS: libc::c_short = libc::POLLERR | libc::POLLHUP | libc::POLLNVAL;

        if poll_fds[0].revents & ERRS != 0 {
            log_err!("server socket error revents={}", poll_fds[0].revents);
            break 'daemon_loop;
        } else if poll_fds[0].revents & libc::POLLIN != 0 {
            let client_fd = unsafe {
                libc::accept4(
                    server_sock_fd,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                )
            };
            if client_fd >= 0 {
                daemon.clients.push(ClientConn::new(client_fd));
                log_info!(
                    "client connected fd={client_fd} total={}",
                    daemon.clients.len()
                );
            }
        }

        if poll_fds[1].revents & (libc::POLLIN | ERRS) != 0 {
            // Read from PTY.
            let mut buf = [0u8; 4096];
            let n_opt = match pty::read_fd(pty_fd, &mut buf) {
                Ok(n) => Some(n),
                Err(err)
                    if err.kind() == io::ErrorKind::WouldBlock
                        || err.kind() == io::ErrorKind::Interrupted =>
                {
                    None
                }
                Err(_) => Some(0),
            };

            if let Some(n) = n_opt {
                if n == 0 {
                    // EOF: shell exited.
                    log_info!("shell exited pty_fd={pty_fd}");
                    break 'daemon_loop;
                }
                // Feed PTY output to the terminal emulator for state tracking.
                if let Some(term) = daemon.term.as_mut() {
                    term.feed(&buf[..n]);
                }
                // Track the window title separately: the terminal model
                // doesn't capture OSC 0/2, so we scan the raw stream
                // (issue #6).
                daemon.title_tracker.feed(&buf[..n]);
                daemon.has_pty_output = true;

                // Broadcast data to all clients.
                for client in daemon.clients.iter_mut() {
                    ipc::append_message(&mut client.write_buf, Tag::Output, &buf[..n]);
                    client.has_pending_output = true;
                }
            }
        }

        // Only iterate over clients that were present when poll_fds was
        // constructed: poll_fds contains [server, pty, client0, client1, ...]
        let num_polled_clients = poll_fds.len() - 2;
        let mut i = daemon.clients.len().min(num_polled_clients);

        'clients_loop: while i > 0 {
            i -= 1;
            let revents = poll_fds[i + 2].revents;

            if revents & libc::POLLIN != 0 {
                let read_result = {
                    let client = &mut daemon.clients[i];
                    client.read_buf.read(client.fd)
                };
                let n = match read_result {
                    Ok(n) => n,
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => continue,
                    Err(err) => {
                        log_debug!("client read err={err} fd={}", daemon.clients[i].fd);
                        daemon.close_client(i, false);
                        continue;
                    }
                };

                if n == 0 {
                    // Client closed connection.
                    daemon.close_client(i, false);
                    continue;
                }

                while let Some(msg) = daemon.clients[i].read_buf.next() {
                    match msg.tag {
                        Tag::Input => daemon.handle_input(&msg.payload),
                        Tag::Init => daemon.handle_init(i, &msg.payload),
                        Tag::Resize => daemon.handle_resize(i, &msg.payload),
                        Tag::Detach => {
                            daemon.handle_detach(i);
                            break 'clients_loop;
                        }
                        Tag::DetachAll => {
                            daemon.handle_detach_all();
                            break 'clients_loop;
                        }
                        Tag::Kill => {
                            break 'daemon_loop;
                        }
                        Tag::Info => daemon.handle_info(i),
                        Tag::History => daemon.handle_history(i, &msg.payload),
                        Tag::Run => daemon.handle_run(i, &msg.payload),
                        Tag::Output | Tag::Ack => {}
                    }
                }
            }

            if revents & libc::POLLOUT != 0 {
                // Flush pending output buffers.
                let client = &mut daemon.clients[i];
                let write_result = pty::write_fd(client.fd, &client.write_buf);
                match write_result {
                    Ok(n) => {
                        if n > 0 {
                            client.write_buf.drain(..n);
                        }
                        if client.write_buf.is_empty() {
                            client.has_pending_output = false;
                        }
                    }
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                    Err(_) => {
                        // Error on write, close client.
                        daemon.close_client(i, false);
                        continue;
                    }
                }
            }

            if revents & ERRS != 0 {
                daemon.close_client(i, false);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_size_returns_none_when_no_client_has_reported() {
        assert!(min_size(&[]).is_none());
        let unset = [
            Resize { rows: 0, cols: 0 },
            Resize { rows: 0, cols: 80 },
            Resize { rows: 24, cols: 0 },
        ];
        assert!(min_size(&unset).is_none());
    }

    #[test]
    fn min_size_returns_the_only_reported_size() {
        let sizes = [Resize { rows: 24, cols: 80 }];
        let m = min_size(&sizes).unwrap();
        assert_eq!(m, Resize { rows: 24, cols: 80 });
    }

    #[test]
    fn min_size_takes_the_elementwise_minimum_across_clients() {
        // One client is shorter, the other narrower: each dimension's min wins.
        let sizes = [
            Resize { rows: 40, cols: 80 },
            Resize {
                rows: 24,
                cols: 100,
            },
        ];
        let m = min_size(&sizes).unwrap();
        assert_eq!(m, Resize { rows: 24, cols: 80 });
    }

    #[test]
    fn min_size_ignores_not_yet_reported_clients() {
        let sizes = [
            Resize {
                rows: 50,
                cols: 200,
            },
            Resize { rows: 0, cols: 0 },
            Resize { rows: 30, cols: 90 },
        ];
        let m = min_size(&sizes).unwrap();
        assert_eq!(m, Resize { rows: 30, cols: 90 });
    }
}
