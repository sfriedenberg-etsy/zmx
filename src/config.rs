//! Directory configuration: socket and log directory resolution, grouped by
//! session group.

use std::fs;
use std::io;

pub struct Cfg {
    pub socket_base: String,
    pub log_base: String,
    pub group: String,
    pub socket_dir: String,
    pub log_dir: String,
    pub max_scrollback: usize,
}

impl Cfg {
    pub fn new(group: &str) -> io::Result<Cfg> {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());

        let socket_base = if let Ok(zmxdir) = std::env::var("ZMX_DIR") {
            zmxdir
        } else if let Ok(xdg_state) = std::env::var("XDG_STATE_HOME") {
            format!("{xdg_state}/zmx")
        } else {
            format!("{home}/.local/state/zmx")
        };

        let log_base = if let Ok(logdir) = std::env::var("ZMX_LOG_DIR") {
            logdir
        } else if let Ok(xdg_log) = std::env::var("XDG_LOG_HOME") {
            format!("{xdg_log}/zmx")
        } else {
            format!("{home}/.local/log/zmx")
        };

        let socket_dir = format!("{socket_base}/{group}");
        let log_dir = format!("{log_base}/{group}");

        let cfg = Cfg {
            socket_base,
            log_base,
            group: group.to_string(),
            socket_dir,
            log_dir,
            max_scrollback: 10_000_000,
        };

        fs::create_dir_all(&cfg.socket_dir)?;
        fs::create_dir_all(&cfg.log_dir)?;

        Ok(cfg)
    }
}
