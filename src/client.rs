//! Client side of a session: attaches to the daemon socket, relays stdin to
//! the daemon and daemon output to stdout, and handles the detach key.

use std::io;
use std::os::fd::RawFd;

use crate::daemon::{ensure_session, Daemon};
use crate::ipc::{self, Tag};
use crate::logger::{log_err, log_info};
use crate::pty;
use crate::session;

/// Detects Kitty keyboard protocol escape sequence for Ctrl+\
/// 92 = backslash, 5 = ctrl modifier, :1 = key press event
pub fn is_kitty_ctrl_backslash(buf: &[u8]) -> bool {
    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }
    contains(buf, b"\x1b[92;5u") || contains(buf, b"\x1b[92;5:1u")
}

pub fn attach(daemon: &mut Daemon, detach: bool) -> Result<(), String> {
    if !detach && std::env::var_os("ZMX_SESSION").is_some() {
        return Err("cannot attach to a session from inside a session".to_string());
    }

    let result = ensure_session(daemon)?;
    if result.is_daemon {
        return Ok(());
    }

    if detach {
        if result.created {
            println!("session \"{}\" created", daemon.session_name);
        } else {
            println!("session \"{}\" already exists", daemon.session_name);
        }
        return Ok(());
    }

    let client_sock =
        session::session_connect(&daemon.socket_path).map_err(|err| err.to_string())?;
    log_info!("attached session={}", daemon.session_name);

    // Raw mode for the attached terminal; restored (with mode-reset
    // sequences) when the guard drops on detach.
    let _raw = pty::RawModeGuard::enter();

    // Clear screen before attaching. This provides a clean slate before
    // the session restore.
    let _ = pty::write_fd(libc::STDOUT_FILENO, b"\x1b[2J\x1b[H");

    let res = client_loop(client_sock);
    session::close_fd(client_sock);
    res.map_err(|err| err.to_string())
}

fn client_loop(client_sock_fd: RawFd) -> io::Result<()> {
    pty::setup_sigwinch_handler();

    // Make socket non-blocking to avoid blocking on writes.
    pty::set_nonblocking(client_sock_fd)?;

    // Buffer for outgoing socket writes.
    let mut sock_write_buf: Vec<u8> = Vec::with_capacity(4096);

    // Send init message with terminal size (buffered).
    let size = pty::get_terminal_size(libc::STDOUT_FILENO);
    ipc::append_message(&mut sock_write_buf, Tag::Init, &size.encode());

    let mut read_buf = ipc::SocketBuffer::new();
    let mut stdout_buf: Vec<u8> = Vec::with_capacity(4096);

    let stdin_fd = libc::STDIN_FILENO;
    pty::set_nonblocking(stdin_fd)?;

    loop {
        // Check for pending SIGWINCH.
        if pty::SIGWINCH_RECEIVED.swap(false, std::sync::atomic::Ordering::AcqRel) {
            let next_size = pty::get_terminal_size(libc::STDOUT_FILENO);
            ipc::append_message(&mut sock_write_buf, Tag::Resize, &next_size.encode());
        }

        let mut poll_fds = Vec::with_capacity(3);
        poll_fds.push(pty::pollfd(stdin_fd, libc::POLLIN));

        // Poll socket for read, and also for write if we have pending data.
        let mut sock_events = libc::POLLIN;
        if !sock_write_buf.is_empty() {
            sock_events |= libc::POLLOUT;
        }
        poll_fds.push(pty::pollfd(client_sock_fd, sock_events));

        if !stdout_buf.is_empty() {
            poll_fds.push(pty::pollfd(libc::STDOUT_FILENO, libc::POLLOUT));
        }

        match pty::poll(&mut poll_fds, -1) {
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue, // EINTR from signal
            Err(err) => return Err(err),
        }

        const ERRS: libc::c_short = libc::POLLHUP | libc::POLLERR | libc::POLLNVAL;

        // Handle stdin -> socket (Input).
        if poll_fds[0].revents & (libc::POLLIN | ERRS) != 0 {
            let mut buf = [0u8; 4096];
            let n_opt = match pty::read_fd(stdin_fd, &mut buf) {
                Ok(n) => Some(n),
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => None,
                Err(err) => return Err(err),
            };

            if let Some(n) = n_opt {
                if n == 0 {
                    // EOF on stdin.
                    return Ok(());
                }
                // Check for detach sequences (ctrl+\ as first byte or Kitty
                // escape sequence).
                if buf[0] == 0x1C || is_kitty_ctrl_backslash(&buf[..n]) {
                    ipc::append_message(&mut sock_write_buf, Tag::Detach, b"");
                } else {
                    ipc::append_message(&mut sock_write_buf, Tag::Input, &buf[..n]);
                }
            }
        }

        // Handle socket read (incoming Output messages from daemon).
        if poll_fds[1].revents & libc::POLLIN != 0 {
            let n = match read_buf.read(client_sock_fd) {
                Ok(n) => n,
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => continue,
                Err(err)
                    if err.kind() == io::ErrorKind::ConnectionReset
                        || err.kind() == io::ErrorKind::BrokenPipe =>
                {
                    return Ok(());
                }
                Err(err) => {
                    log_err!("daemon read err={err}");
                    return Err(err);
                }
            };
            if n == 0 {
                return Ok(()); // Server closed connection
            }

            while let Some(msg) = read_buf.next() {
                if msg.tag == Tag::Output && !msg.payload.is_empty() {
                    stdout_buf.extend_from_slice(&msg.payload);
                }
            }
        }

        // Handle socket write (flush buffered messages to daemon).
        if poll_fds[1].revents & libc::POLLOUT != 0 && !sock_write_buf.is_empty() {
            match pty::write_fd(client_sock_fd, &sock_write_buf) {
                Ok(n) => {
                    if n > 0 {
                        sock_write_buf.drain(..n);
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                Err(err)
                    if err.kind() == io::ErrorKind::ConnectionReset
                        || err.kind() == io::ErrorKind::BrokenPipe =>
                {
                    return Ok(());
                }
                Err(err) => return Err(err),
            }
        }

        if !stdout_buf.is_empty() {
            match pty::write_fd(libc::STDOUT_FILENO, &stdout_buf) {
                Ok(n) => {
                    if n > 0 {
                        stdout_buf.drain(..n);
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                Err(err) => return Err(err),
            }
        }

        if poll_fds[1].revents & ERRS != 0 {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kitty_ctrl_backslash_detection() {
        assert!(is_kitty_ctrl_backslash(b"\x1b[92;5u"));
        assert!(is_kitty_ctrl_backslash(b"\x1b[92;5:1u"));
        assert!(!is_kitty_ctrl_backslash(b"\x1b[92;5:3u"));
        assert!(!is_kitty_ctrl_backslash(b"\x1b[92;1u"));
        assert!(!is_kitty_ctrl_backslash(b"garbage"));
    }
}
