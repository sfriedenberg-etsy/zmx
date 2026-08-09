//! CLI subcommands: list, groups, detach, kill, history, run, fork.

use std::fmt::Write as _;
use std::io::{Read as _, Write as _};

use crate::config::Cfg;
use crate::daemon::{ensure_session, Daemon};
use crate::ipc::{self, Tag};
use crate::logger::{log_err, log_info};
use crate::names;
use crate::pty;
use crate::session::{self, close_fd};
use crate::vt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListFormat {
    Default,
    Short,
    Json,
}

pub struct SessionEntry {
    pub name: String,
    pub pid: Option<i32>,
    pub clients_len: Option<u64>,
    pub is_error: bool,
    pub error_name: Option<String>,
    pub cmd: Option<String>,
    pub cwd: Option<String>,
}

const CURRENT_ARROW: &str = "→";

fn current_session() -> Option<String> {
    std::env::var("ZMX_SESSION").ok()
}

fn send_ignoring_disconnect(fd: i32, tag: Tag) -> Result<(), String> {
    match ipc::send(fd, tag, b"") {
        Ok(()) => Ok(()),
        Err(err)
            if err.kind() == std::io::ErrorKind::BrokenPipe
                || err.kind() == std::io::ErrorKind::ConnectionReset =>
        {
            Ok(())
        }
        Err(err) => Err(err.to_string()),
    }
}

pub fn list(cfg: &Cfg, format: ListFormat) -> Result<(), String> {
    let current = current_session();

    let dir = std::fs::read_dir(&cfg.socket_dir).map_err(|err| err.to_string())?;
    let mut sessions: Vec<SessionEntry> = Vec::new();

    for entry in dir.flatten() {
        let fname = entry.file_name();
        let Some(fname) = fname.to_str() else {
            continue;
        };
        let exists = match session::session_exists(&cfg.socket_dir, fname) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if !exists {
            continue;
        }
        // Decode the filename to get the original session name.
        let name = names::decode_session_name(fname);
        let socket_path = format!("{}/{}", cfg.socket_dir, fname);

        match session::probe_session(&socket_path) {
            Ok(result) => {
                close_fd(result.fd);
                let cmd = if result.info.cmd.is_empty() {
                    None
                } else {
                    Some(String::from_utf8_lossy(&result.info.cmd).into_owned())
                };
                let cwd = if result.info.cwd.is_empty() {
                    None
                } else {
                    Some(String::from_utf8_lossy(&result.info.cwd).into_owned())
                };
                sessions.push(SessionEntry {
                    name,
                    pid: Some(result.info.pid),
                    clients_len: Some(result.info.clients_len),
                    is_error: false,
                    error_name: None,
                    cmd,
                    cwd,
                });
            }
            Err(err) => {
                sessions.push(SessionEntry {
                    name,
                    pid: None,
                    clients_len: None,
                    is_error: true,
                    error_name: Some(err),
                    cmd: None,
                    cwd: None,
                });
                session::cleanup_stale_socket(&cfg.socket_dir, fname);
            }
        }
    }

    let mut out = String::new();
    if sessions.is_empty() {
        match format {
            ListFormat::Short => return Ok(()),
            ListFormat::Json => {
                ipc::print_stdout("[]\n");
                return Ok(());
            }
            ListFormat::Default => {
                ipc::print_stdout(&format!("no sessions found in {}\n", cfg.socket_dir));
                return Ok(());
            }
        }
    }

    sessions.sort_by(|a, b| a.name.cmp(&b.name));

    if format == ListFormat::Json {
        write_json_list(&mut out, &sessions, current.as_deref());
        ipc::print_stdout(&out);
        return Ok(());
    }

    for session in &sessions {
        write_session_line(
            &mut out,
            session,
            format == ListFormat::Short,
            current.as_deref(),
        );
    }
    ipc::print_stdout(&out);
    Ok(())
}

/// Formats a session entry for list output (only the name when `short` is
/// true), adding a prefix to indicate the current session, if there is one.
fn write_session_line(
    out: &mut String,
    session: &SessionEntry,
    short: bool,
    current: Option<&str>,
) {
    let prefix = match current {
        Some(cur) if cur == session.name => {
            let mut p = String::from(CURRENT_ARROW);
            p.push(' ');
            p
        }
        Some(_) => "  ".to_string(),
        None => String::new(),
    };

    if short {
        if session.is_error {
            return;
        }
        let _ = writeln!(out, "{}", session.name);
        return;
    }

    if session.is_error {
        let _ = writeln!(
            out,
            "{}session_name={}\tstatus={}\t(cleaning up)",
            prefix,
            session.name,
            session.error_name.as_deref().unwrap_or("unknown"),
        );
        return;
    }

    let _ = write!(
        out,
        "{}session_name={}\tpid={}\tclients={}",
        prefix,
        session.name,
        session.pid.unwrap(),
        session.clients_len.unwrap(),
    );
    if let Some(cwd) = &session.cwd {
        let _ = write!(out, "\tstarted_in={cwd}");
    }
    if let Some(cmd) = &session.cmd {
        let _ = write!(out, "\tcmd={cmd}");
    }
    let _ = writeln!(out);
}

fn write_json_string(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ch if (ch as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", ch as u32);
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
}

fn write_json_list(out: &mut String, sessions: &[SessionEntry], current: Option<&str>) {
    out.push('[');
    for (i, session) in sessions.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"name\":");
        write_json_string(out, &session.name);
        if session.is_error {
            out.push_str(",\"error\":true,\"status\":");
            write_json_string(out, session.error_name.as_deref().unwrap_or("unknown"));
        } else {
            let is_current = current.is_some_and(|cur| cur == session.name);
            let _ = write!(
                out,
                ",\"pid\":{},\"clients\":{}",
                session.pid.unwrap(),
                session.clients_len.unwrap()
            );
            if let Some(cwd) = &session.cwd {
                out.push_str(",\"cwd\":");
                write_json_string(out, cwd);
            }
            if let Some(cmd) = &session.cmd {
                out.push_str(",\"cmd\":");
                write_json_string(out, cmd);
            }
            let _ = write!(
                out,
                ",\"current\":{}",
                if is_current { "true" } else { "false" }
            );
        }
        out.push('}');
    }
    out.push_str("]\n");
}

pub fn list_groups(cfg: &Cfg) -> Result<(), String> {
    let base = match std::fs::read_dir(&cfg.socket_base) {
        Ok(d) => d,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.to_string()),
    };

    let mut groups: Vec<String> = Vec::new();
    for entry in base.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let Ok(group_dir) = std::fs::read_dir(entry.path()) else {
            continue;
        };
        let has_sockets = group_dir.flatten().any(|sub| {
            sub.file_type()
                .map(|t| {
                    t.is_file() || {
                        use std::os::unix::fs::FileTypeExt;
                        t.is_socket()
                    }
                })
                .unwrap_or(false)
        });
        if has_sockets {
            if let Some(name) = entry.file_name().to_str() {
                groups.push(name.to_string());
            }
        }
    }

    groups.sort();
    let mut out = String::new();
    for name in groups {
        let _ = writeln!(out, "{name}");
    }
    ipc::print_stdout(&out);
    Ok(())
}

/// `zmx detach` with no name: detach all clients from the current session.
pub fn detach_current(cfg: &Cfg) -> Result<(), String> {
    let Some(session_name) = current_session() else {
        log_err!("ZMX_SESSION env var not found: are you inside a zmx session?");
        return Ok(());
    };
    detach_session(cfg, &session_name)
}

pub fn detach_session(cfg: &Cfg, session_name: &str) -> Result<(), String> {
    let encoded = names::encode_session_name(session_name);
    let exists = session::session_exists(&cfg.socket_dir, &encoded).map_err(|e| e.to_string())?;
    if !exists {
        log_err!("session does not exist session_name={session_name}");
        return Ok(());
    }

    let socket_path = names::socket_path(&cfg.socket_dir, session_name)?;
    let result = match session::probe_session(&socket_path) {
        Ok(r) => r,
        Err(err) => {
            log_err!("session unresponsive: {err}");
            session::cleanup_stale_socket(&cfg.socket_dir, &encoded);
            return Ok(());
        }
    };
    let res = send_ignoring_disconnect(result.fd, Tag::DetachAll);
    close_fd(result.fd);
    res
}

/// `zmx detach-all`: detach all clients from every session in the group.
pub fn detach_all_sessions(cfg: &Cfg) -> Result<(), String> {
    let dir = std::fs::read_dir(&cfg.socket_dir).map_err(|err| err.to_string())?;
    for entry in dir.flatten() {
        let fname = entry.file_name();
        let Some(fname) = fname.to_str() else {
            continue;
        };
        let exists = match session::session_exists(&cfg.socket_dir, fname) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if !exists {
            continue;
        }
        let socket_path = format!("{}/{}", cfg.socket_dir, fname);
        let result = match session::probe_session(&socket_path) {
            Ok(r) => r,
            Err(_) => {
                session::cleanup_stale_socket(&cfg.socket_dir, fname);
                continue;
            }
        };
        let res = send_ignoring_disconnect(result.fd, Tag::DetachAll);
        close_fd(result.fd);
        res?;
    }
    Ok(())
}

pub fn kill(cfg: &Cfg, session_name: &str) -> Result<(), String> {
    let encoded = names::encode_session_name(session_name);
    let exists = session::session_exists(&cfg.socket_dir, &encoded).map_err(|e| e.to_string())?;
    if !exists {
        log_err!("cannot kill session because it does not exist session_name={session_name}");
        return Ok(());
    }

    let socket_path = names::socket_path(&cfg.socket_dir, session_name)?;
    let result = match session::probe_session(&socket_path) {
        Ok(r) => r,
        Err(err) => {
            log_err!("session unresponsive: {err}");
            session::cleanup_stale_socket(&cfg.socket_dir, &encoded);
            ipc::print_stdout(&format!("cleaned up stale session {session_name}\n"));
            return Ok(());
        }
    };
    let res = send_ignoring_disconnect(result.fd, Tag::Kill);
    close_fd(result.fd);
    res?;

    ipc::print_stdout(&format!("killed session {session_name}\n"));
    Ok(())
}

pub fn history(cfg: &Cfg, session_name: &str, format: vt::Format) -> Result<(), String> {
    let encoded = names::encode_session_name(session_name);
    let exists = session::session_exists(&cfg.socket_dir, &encoded).map_err(|e| e.to_string())?;
    if !exists {
        log_err!("session does not exist session_name={session_name}");
        return Ok(());
    }

    let socket_path = names::socket_path(&cfg.socket_dir, session_name)?;
    let result = match session::probe_session(&socket_path) {
        Ok(r) => r,
        Err(err) => {
            log_err!("session unresponsive: {err}");
            session::cleanup_stale_socket(&cfg.socket_dir, &encoded);
            return Ok(());
        }
    };

    let res = (|| {
        match ipc::send(result.fd, Tag::History, &[format as u8]) {
            Ok(()) => {}
            Err(err)
                if err.kind() == std::io::ErrorKind::BrokenPipe
                    || err.kind() == std::io::ErrorKind::ConnectionReset =>
            {
                return Ok(());
            }
            Err(err) => return Err(err.to_string()),
        }

        let mut sb = ipc::SocketBuffer::new();
        loop {
            let mut fds = [pty::pollfd(result.fd, libc::POLLIN)];
            let n = match pty::poll(&mut fds, 5000) {
                Ok(n) => n,
                Err(_) => return Ok(()),
            };
            if n == 0 {
                log_err!("timeout waiting for history response");
                return Ok(());
            }

            let n = match sb.read(result.fd) {
                Ok(n) => n,
                Err(_) => return Ok(()),
            };
            if n == 0 {
                return Ok(());
            }

            while let Some(msg) = sb.next() {
                if msg.tag == Tag::History {
                    let _ = std::io::stdout().write_all(&msg.payload);
                    let _ = std::io::stdout().flush();
                    return Ok(());
                }
            }
        }
    })();
    close_fd(result.fd);
    res
}

pub fn run(daemon: &mut Daemon, command_args: &[String]) -> Result<(), String> {
    let result = ensure_session(daemon)?;
    if result.is_daemon {
        return Ok(());
    }

    if result.created {
        ipc::print_stdout(&format!("session \"{}\" created\n", daemon.session_name));
    }

    let cmd_to_send: Vec<u8> = if !command_args.is_empty() {
        let mut buf = command_args.join(" ").into_bytes();
        buf.push(b'\n');
        buf
    } else {
        let stdin_is_tty = unsafe { libc::isatty(libc::STDIN_FILENO) } == 1;
        if stdin_is_tty {
            return Err("command required".to_string());
        }
        let mut buf = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buf)
            .map_err(|err| err.to_string())?;
        if buf.is_empty() {
            return Err("command required".to_string());
        }
        if buf.last() != Some(&b'\n') {
            buf.push(b'\n');
        }
        buf
    };

    let probe_result = session::probe_session(&daemon.socket_path).map_err(|err| {
        log_err!("session not ready: {err}");
        "session not ready".to_string()
    })?;

    let res = (|| {
        ipc::send(probe_result.fd, Tag::Run, &cmd_to_send).map_err(|err| err.to_string())?;

        let mut fds = [pty::pollfd(probe_result.fd, libc::POLLIN)];
        let n = pty::poll(&mut fds, 5000).map_err(|_| "poll failed".to_string())?;
        if n == 0 {
            log_err!("timeout waiting for ack");
            return Err("timeout".to_string());
        }

        let mut sb = ipc::SocketBuffer::new();
        let n = sb
            .read(probe_result.fd)
            .map_err(|_| "read failed".to_string())?;
        if n == 0 {
            return Err("connection closed".to_string());
        }

        while let Some(msg) = sb.next() {
            if msg.tag == Tag::Ack {
                ipc::print_stdout("command sent\n");
                return Ok(());
            }
        }
        Err("no ack received".to_string())
    })();
    close_fd(probe_result.fd);
    res
}

pub fn fork_session(cfg: &Cfg, explicit_name: Option<&str>) -> Result<(), String> {
    if let Some(name) = explicit_name {
        return fork(cfg, name);
    }

    // Auto-generate name from $ZMX_SESSION.
    let Some(source_name) = current_session() else {
        log_err!("ZMX_SESSION env var not found: are you inside a zmx session?");
        return Ok(());
    };

    let auto_name = next_fork_name(cfg, &source_name)?;
    fork(cfg, &auto_name)
}

fn next_fork_name(cfg: &Cfg, base_name: &str) -> Result<String, String> {
    for i in 1..1000u32 {
        let candidate = format!("{base_name}-{i}");
        let encoded = names::encode_session_name(&candidate);
        let exists = session::session_exists(&cfg.socket_dir, &encoded).unwrap_or(false);
        if !exists {
            return Ok(candidate);
        }
    }
    Err("too many sessions".to_string())
}

fn fork(cfg: &Cfg, target_name: &str) -> Result<(), String> {
    // Must be inside a zmx session.
    let Some(source_name) = current_session() else {
        log_err!("ZMX_SESSION env var not found: are you inside a zmx session?");
        return Ok(());
    };

    // Probe source session for cmd + cwd.
    let source_socket_path = names::socket_path(&cfg.socket_dir, &source_name)?;
    let source_encoded = names::encode_session_name(&source_name);

    let result = match session::probe_session(&source_socket_path) {
        Ok(r) => r,
        Err(err) => {
            log_err!("source session unresponsive: {err}");
            session::cleanup_stale_socket(&cfg.socket_dir, &source_encoded);
            return Ok(());
        }
    };
    close_fd(result.fd);

    // Check target doesn't already exist.
    let target_encoded = names::encode_session_name(target_name);
    let exists = session::session_exists(&cfg.socket_dir, &target_encoded).unwrap_or(false);
    if exists {
        log_err!("session already exists: {target_name}");
        return Ok(());
    }

    // Extract command args from the space-joined string.
    let cmd_str = String::from_utf8_lossy(&result.info.cmd).into_owned();
    let command_args: Vec<String> = cmd_str
        .split(' ')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    let command = if command_args.is_empty() {
        None
    } else {
        Some(command_args)
    };

    // chdir to source cwd so the new daemon inherits it.
    let source_cwd = String::from_utf8_lossy(&result.info.cwd).into_owned();
    if !source_cwd.is_empty() {
        if let Err(err) = std::env::set_current_dir(&source_cwd) {
            crate::logger::log_warn!("could not chdir to {source_cwd}: {err}");
        }
    }

    // Spawn new session without attaching.
    let mut daemon = Daemon::new(cfg, target_name, command, source_cwd)?;
    log_info!("forking session={target_name} from={source_name}");
    let ensure_result = ensure_session(&mut daemon)?;
    let _ = ensure_result.is_daemon;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> SessionEntry {
        SessionEntry {
            name: name.to_string(),
            pid: Some(123),
            clients_len: Some(2),
            is_error: false,
            error_name: None,
            cmd: None,
            cwd: None,
        }
    }

    #[test]
    fn write_session_line_formats_output_for_current_session_and_short_output() {
        struct Case {
            short: bool,
            current: Option<&'static str>,
            expected: &'static str,
        }
        let cases = [
            Case {
                short: false,
                current: Some("dev"),
                expected: "→ session_name=dev\tpid=123\tclients=2\n",
            },
            Case {
                short: false,
                current: Some("other"),
                expected: "  session_name=dev\tpid=123\tclients=2\n",
            },
            Case {
                short: false,
                current: None,
                expected: "session_name=dev\tpid=123\tclients=2\n",
            },
            Case {
                short: true,
                current: Some("dev"),
                expected: "dev\n",
            },
            Case {
                short: true,
                current: Some("other"),
                expected: "dev\n",
            },
            Case {
                short: true,
                current: None,
                expected: "dev\n",
            },
        ];

        for case in cases {
            let mut out = String::new();
            write_session_line(&mut out, &entry("dev"), case.short, case.current);
            assert_eq!(out, case.expected);
        }
    }

    #[test]
    fn write_json_string_escapes_special_characters() {
        let cases = [
            ("hello", "\"hello\""),
            ("say \"hi\"", "\"say \\\"hi\\\"\""),
            ("back\\slash", "\"back\\\\slash\""),
            ("new\nline", "\"new\\nline\""),
            ("tab\there", "\"tab\\there\""),
            ("ctrl\u{1}char", "\"ctrl\\u0001char\""),
            ("", "\"\""),
        ];
        for (input, expected) in cases {
            let mut out = String::new();
            write_json_string(&mut out, input);
            assert_eq!(out, expected);
        }
    }

    #[test]
    fn write_json_list_formats_sessions_as_json_array() {
        let sessions = vec![
            SessionEntry {
                name: "dev".to_string(),
                pid: Some(123),
                clients_len: Some(2),
                is_error: false,
                error_name: None,
                cmd: Some("bash".to_string()),
                cwd: Some("/home/user".to_string()),
            },
            SessionEntry {
                name: "broken".to_string(),
                pid: None,
                clients_len: None,
                is_error: true,
                error_name: Some("ConnectionRefused".to_string()),
                cmd: None,
                cwd: None,
            },
        ];
        let mut out = String::new();
        write_json_list(&mut out, &sessions, Some("dev"));
        assert_eq!(
            out,
            "[{\"name\":\"dev\",\"pid\":123,\"clients\":2,\"cwd\":\"/home/user\",\"cmd\":\"bash\",\"current\":true},{\"name\":\"broken\",\"error\":true,\"status\":\"ConnectionRefused\"}]\n"
        );
    }

    #[test]
    fn write_json_list_outputs_empty_array_when_no_sessions() {
        let mut out = String::new();
        write_json_list(&mut out, &[], None);
        assert_eq!(out, "[]\n");
    }
}
