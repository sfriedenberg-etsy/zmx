//! Unix socket helpers: connecting to, creating, probing, and cleaning up
//! session sockets.

use std::io;
use std::os::fd::RawFd;
use std::os::unix::fs::FileTypeExt;
use std::path::Path;

use crate::ipc::{self, Tag};
use crate::logger::log_warn;
use crate::pty;

pub fn close_fd(fd: RawFd) {
    unsafe {
        libc::close(fd);
    }
}

fn sockaddr_un(path: &str) -> io::Result<(libc::sockaddr_un, libc::socklen_t)> {
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    let bytes = path.as_bytes();
    if bytes.len() >= addr.sun_path.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "socket path too long",
        ));
    }
    for (i, &b) in bytes.iter().enumerate() {
        addr.sun_path[i] = b as libc::c_char;
    }
    let len = std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t;
    Ok((addr, len))
}

pub fn session_connect(path: &str) -> io::Result<RawFd> {
    let (addr, len) = sockaddr_un(path)?;
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let rc = unsafe { libc::connect(fd, &addr as *const _ as *const libc::sockaddr, len) };
    if rc < 0 {
        let err = io::Error::last_os_error();
        close_fd(fd);
        return Err(err);
    }
    Ok(fd)
}

pub fn create_socket(path: &str) -> io::Result<RawFd> {
    // AF_UNIX: Unix domain socket for local IPC with client processes.
    // SOCK_STREAM: reliable, bidirectional communication.
    // SOCK_NONBLOCK: non-blocking accept loop in the daemon.
    let (addr, len) = sockaddr_un(path)?;
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let rc = unsafe { libc::bind(fd, &addr as *const _ as *const libc::sockaddr, len) };
    if rc < 0 {
        let err = io::Error::last_os_error();
        close_fd(fd);
        return Err(err);
    }
    if unsafe { libc::listen(fd, 128) } < 0 {
        let err = io::Error::last_os_error();
        close_fd(fd);
        return Err(err);
    }
    Ok(fd)
}

/// True when `name` exists in `dir` and is a Unix socket. A non-socket file
/// with that name is an error.
pub fn session_exists(dir: &str, name: &str) -> io::Result<bool> {
    let path = Path::new(dir).join(name);
    match std::fs::metadata(&path) {
        Ok(meta) => {
            if meta.file_type().is_socket() {
                Ok(true)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "file is not a unix socket",
                ))
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

pub fn cleanup_stale_socket(dir: &str, encoded_name: &str) {
    log_warn!("stale socket found, cleaning up session={encoded_name}");
    let path = Path::new(dir).join(encoded_name);
    if let Err(err) = std::fs::remove_file(&path) {
        log_warn!("failed to delete stale socket err={err}");
    }
}

pub struct ProbeResult {
    pub fd: RawFd,
    pub info: ipc::Info,
}

/// Connect to a session socket, request Info, and wait (up to 1s) for the
/// reply. The returned fd stays open for follow-up messages.
pub fn probe_session(socket_path: &str) -> Result<ProbeResult, String> {
    const TIMEOUT_MS: i32 = 1000;
    let fd = session_connect(socket_path).map_err(|err| err.to_string())?;

    let result = (|| {
        ipc::send(fd, Tag::Info, b"").map_err(|err| err.to_string())?;

        let mut fds = [pty::pollfd(fd, libc::POLLIN)];
        let n = pty::poll(&mut fds, TIMEOUT_MS).map_err(|err| err.to_string())?;
        if n == 0 {
            return Err("Timeout".to_string());
        }

        let mut sb = ipc::SocketBuffer::new();
        let n = sb.read(fd).map_err(|err| err.to_string())?;
        if n == 0 {
            return Err("connection closed".to_string());
        }

        while let Some(msg) = sb.next() {
            if msg.tag == Tag::Info {
                if let Some(info) = ipc::Info::decode(&msg.payload) {
                    return Ok(info);
                }
            }
        }
        Err("no info reply".to_string())
    })();

    match result {
        Ok(info) => Ok(ProbeResult { fd, info }),
        Err(err) => {
            close_fd(fd);
            Err(err)
        }
    }
}
