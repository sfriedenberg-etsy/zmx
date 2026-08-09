//! PTY management and terminal control: forkpty-based spawning, window
//! sizes, raw mode, signal flags, and poll() helpers.

use std::ffi::CString;
use std::io;
use std::os::fd::RawFd;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::ipc::Resize;

pub static SIGWINCH_RECEIVED: AtomicBool = AtomicBool::new(false);
pub static SIGTERM_RECEIVED: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_sigwinch(_: libc::c_int) {
    SIGWINCH_RECEIVED.store(true, Ordering::Release);
}

extern "C" fn handle_sigterm(_: libc::c_int) {
    SIGTERM_RECEIVED.store(true, Ordering::Release);
}

fn install_handler(signal: libc::c_int, handler: extern "C" fn(libc::c_int)) {
    unsafe {
        let mut act: libc::sigaction = std::mem::zeroed();
        act.sa_sigaction = handler as libc::sighandler_t;
        libc::sigemptyset(&mut act.sa_mask);
        libc::sigaction(signal, &act, std::ptr::null_mut());
    }
}

pub fn setup_sigwinch_handler() {
    install_handler(libc::SIGWINCH, handle_sigwinch);
}

pub fn setup_sigterm_handler() {
    install_handler(libc::SIGTERM, handle_sigterm);
}

pub fn get_terminal_size(fd: RawFd) -> Resize {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) } == 0;
    if ok && ws.ws_row > 0 && ws.ws_col > 0 {
        Resize {
            rows: ws.ws_row,
            cols: ws.ws_col,
        }
    } else {
        Resize { rows: 24, cols: 80 }
    }
}

pub fn set_pty_size(fd: RawFd, size: Resize) {
    let ws = libc::winsize {
        ws_row: size.rows,
        ws_col: size.cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    unsafe {
        libc::ioctl(fd, libc::TIOCSWINSZ, &ws);
    }
}

pub fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL, 0);
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Spawn the session process on a new PTY. Returns (master_fd, child_pid).
/// `command` empty means "login shell from $SHELL".
pub fn spawn_pty(
    session_name: &str,
    group: &str,
    command: &[String],
) -> io::Result<(RawFd, libc::pid_t)> {
    let size = get_terminal_size(libc::STDOUT_FILENO);
    let ws = libc::winsize {
        ws_row: size.rows,
        ws_col: size.cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    let mut master_fd: libc::c_int = -1;
    let pid = unsafe { libc::forkpty(&mut master_fd, std::ptr::null_mut(), std::ptr::null(), &ws) };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }

    if pid == 0 {
        // Child: runs the session command on the PTY slave.
        std::env::set_var("ZMX_SESSION", session_name);
        std::env::set_var("ZMX_GROUP", group);

        let exit_err = |msg: &str| -> ! {
            eprintln!("zmx: {msg}: {}", io::Error::last_os_error());
            std::process::exit(1);
        };

        if !command.is_empty() {
            let cmd = CString::new(command[0].as_str()).unwrap_or_else(|_| {
                std::process::exit(1);
            });
            let args: Vec<CString> = command
                .iter()
                .filter_map(|a| CString::new(a.as_str()).ok())
                .collect();
            let mut argv: Vec<*const libc::c_char> = args.iter().map(|a| a.as_ptr()).collect();
            argv.push(std::ptr::null());
            unsafe {
                libc::execvp(cmd.as_ptr(), argv.as_ptr());
            }
            exit_err("execvp failed");
        } else {
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
            let basename = shell.rsplit('/').next().unwrap_or("sh");
            // Use "-shellname" as argv[0] to signal a login shell
            // (traditional method).
            let argv0 = CString::new(format!("-{basename}")).unwrap();
            let path = CString::new(shell.as_str()).unwrap();
            let argv: [*const libc::c_char; 2] = [argv0.as_ptr(), std::ptr::null()];
            unsafe {
                libc::execv(path.as_ptr(), argv.as_ptr());
            }
            exit_err("execv failed");
        }
    }

    set_nonblocking(master_fd)?;
    Ok((master_fd, pid))
}

/// Saved termios state restored (with mode-reset sequences) on drop.
pub struct RawModeGuard {
    orig: libc::termios,
}

impl RawModeGuard {
    /// Switch stdin to raw mode for the attached client.
    pub fn enter() -> RawModeGuard {
        let mut orig: libc::termios = unsafe { std::mem::zeroed() };
        unsafe {
            libc::tcgetattr(libc::STDIN_FILENO, &mut orig);
        }

        let mut raw = orig;
        unsafe {
            libc::cfmakeraw(&mut raw);
        }
        // Additional granular raw mode settings for precise control
        // (matches what abduco and shpool do).
        raw.c_cc[libc::VLNEXT] = 0; // Disable literal-next (Ctrl-V)
                                    // Intercept Ctrl+\ (SIGQUIT) so it can be used as the detach key.
        raw.c_cc[libc::VQUIT] = 0;
        raw.c_cc[libc::VMIN] = 1; // Return after 1 byte
        raw.c_cc[libc::VTIME] = 0; // No read timeout

        unsafe {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw);
        }
        RawModeGuard { orig }
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        unsafe {
            // TCSAFLUSH discards any unread input, preventing stale input
            // after detach.
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSAFLUSH, &self.orig);
        }
        // Reset terminal modes on detach:
        // - Mouse: 1000=basic, 1002=button-event, 1003=any-event, 1006=SGR extended
        // - 2004=bracketed paste, 1004=focus events, 1049=alt screen
        // - 25h=show cursor
        // NOTE: We intentionally do NOT clear the screen or home the cursor
        // here to avoid corrupting programs that rely on it (including
        // terminal session restore).
        let restore_seq = b"\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?2004l\x1b[?1004l\x1b[?1049l\x1b[?25h";
        unsafe {
            libc::write(
                libc::STDOUT_FILENO,
                restore_seq.as_ptr() as *const libc::c_void,
                restore_seq.len(),
            );
        }
    }
}

/// Thin wrapper over poll(2).
pub fn poll(fds: &mut [libc::pollfd], timeout_ms: i32) -> io::Result<usize> {
    let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout_ms) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(n as usize)
}

pub fn pollfd(fd: RawFd, events: libc::c_short) -> libc::pollfd {
    libc::pollfd {
        fd,
        events,
        revents: 0,
    }
}

pub fn read_fd(fd: RawFd, buf: &mut [u8]) -> io::Result<usize> {
    let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(n as usize)
}

pub fn write_fd(fd: RawFd, buf: &[u8]) -> io::Result<usize> {
    let n = unsafe { libc::write(fd, buf.as_ptr() as *const libc::c_void, buf.len()) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(n as usize)
}
