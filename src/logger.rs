//! File-based logging with 5MB rotation.
//!
//! Messages are written as `[<millis>] [<level>] (default): <msg>` to a log
//! file; before the logger is initialized they fall back to stderr.

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_SIZE: u64 = 5 * 1024 * 1024;

pub struct LogSystem {
    file: Option<File>,
    current_size: u64,
    path: String,
}

static LOG: Mutex<LogSystem> = Mutex::new(LogSystem {
    file: None,
    current_size: 0,
    path: String::new(),
});

#[derive(Clone, Copy)]
pub enum Level {
    Debug,
    Info,
    Warning,
    Error,
}

impl Level {
    fn as_text(self) -> &'static str {
        match self {
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warning => "warning",
            Level::Error => "err",
        }
    }
}

pub fn init(path: &str) -> std::io::Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(path)?;
    let end_pos = file.seek(SeekFrom::End(0))?;
    let mut log = LOG.lock().unwrap();
    log.file = Some(file);
    log.current_size = end_pos;
    log.path = path.to_string();
    Ok(())
}

/// Close the current log file (used by the daemon before re-initializing to
/// its per-session log path).
pub fn deinit() {
    let mut log = LOG.lock().unwrap();
    log.file = None;
    log.current_size = 0;
    log.path.clear();
}

pub fn log(level: Level, msg: &str) {
    let mut log = LOG.lock().unwrap();
    if log.file.is_none() {
        eprintln!("[{}] {}", level.as_text(), msg);
        return;
    }

    if log.current_size >= MAX_SIZE {
        if let Err(err) = rotate(&mut log) {
            eprintln!("Log rotation failed: {err}");
        }
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let line = format!("[{}] [{}] (default): {}\n", now, level.as_text(), msg);
    log.current_size += line.len() as u64;
    if let Some(f) = log.file.as_mut() {
        let _ = f.write_all(line.as_bytes());
    }
}

fn rotate(log: &mut LogSystem) -> std::io::Result<()> {
    log.file = None;
    let old_path = format!("{}.old", log.path);
    match std::fs::rename(&log.path, &old_path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    log.file = Some(
        OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&log.path)?,
    );
    log.current_size = 0;
    Ok(())
}

macro_rules! log_debug {
    ($($arg:tt)*) => { $crate::logger::log($crate::logger::Level::Debug, &format!($($arg)*)) };
}
macro_rules! log_info {
    ($($arg:tt)*) => { $crate::logger::log($crate::logger::Level::Info, &format!($($arg)*)) };
}
macro_rules! log_warn {
    ($($arg:tt)*) => { $crate::logger::log($crate::logger::Level::Warning, &format!($($arg)*)) };
}
macro_rules! log_err {
    ($($arg:tt)*) => { $crate::logger::log($crate::logger::Level::Error, &format!($($arg)*)) };
}

pub(crate) use {log_debug, log_err, log_info, log_warn};
