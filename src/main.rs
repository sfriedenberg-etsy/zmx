//! zmx - session persistence for terminal processes.
//!
//! Entry point: global flag parsing and subcommand dispatch. The heavy
//! lifting lives in `daemon` (session daemons), `client` (attach loop), and
//! `vt` (the built-in terminal emulator).

mod logger;

mod client;
mod commands;
mod completions;
mod config;
mod daemon;
mod ipc;
mod names;
mod pty;
mod session;
mod title;
mod vt;

use commands::ListFormat;
use config::Cfg;
use daemon::Daemon;

pub const VERSION: &str = env!("ZMX_BUILD_VERSION");
pub const COMMIT: &str = env!("ZMX_BUILD_COMMIT");

fn main() {
    match run() {
        Ok(()) => {}
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1).peekable();

    // Parse global flags before the subcommand.
    let mut group = std::env::var("ZMX_GROUP").unwrap_or_else(|_| "default".to_string());
    let mut cmd: Option<String> = None;

    while let Some(arg) = args.next() {
        if arg == "-g" || arg == "--group" {
            let value = args.next().ok_or("--group requires a value")?;
            // Validate group name.
            if value.is_empty() {
                return Err("group name cannot be empty".to_string());
            }
            if value.contains('/') || value.contains("..") {
                return Err(format!("invalid group name: {value}"));
            }
            group = value;
        } else {
            cmd = Some(arg);
            break;
        }
    }

    let cfg = Cfg::new(&group).map_err(|err| err.to_string())?;

    let log_path = format!("{}/zmx.log", cfg.log_base);
    logger::init(&log_path).map_err(|err| err.to_string())?;

    let Some(command) = cmd else {
        return commands::list(&cfg, ListFormat::Default);
    };

    match command.as_str() {
        "groups" | "gs" => commands::list_groups(&cfg),
        "version" | "v" | "-v" | "--version" => {
            print_version();
            Ok(())
        }
        "help" | "h" | "-h" => {
            print_help();
            Ok(())
        }
        "list" | "l" => {
            let mut list_format = ListFormat::Default;
            for arg in args {
                if arg == "--short" {
                    list_format = ListFormat::Short;
                } else if arg == "--json" || arg == "-j" {
                    list_format = ListFormat::Json;
                }
            }
            commands::list(&cfg, list_format)
        }
        "completions" | "c" => {
            let Some(arg) = args.next() else {
                return Ok(());
            };
            if let Some(shell) = completions::Shell::from_str(&arg) {
                println!("{}", shell.completion_script());
            }
            Ok(())
        }
        "fork" | "f" => {
            let target_name = args.next();
            commands::fork_session(&cfg, target_name.as_deref())
        }
        "detach-all" | "da" => commands::detach_all_sessions(&cfg),
        "detach" | "d" => {
            if let Some(session_name) = args.next() {
                commands::detach_session(&cfg, &session_name)
            } else {
                commands::detach_current(&cfg)
            }
        }
        "kill" | "k" => {
            let session_name = args.next().ok_or("session name required")?;
            commands::kill(&cfg, &session_name)
        }
        "history" | "hi" => {
            let mut session_name: Option<String> = None;
            let mut format = vt::Format::Plain;
            for arg in args {
                if arg == "--vt" {
                    format = vt::Format::Vt;
                } else if arg == "--html" {
                    format = vt::Format::Html;
                } else if session_name.is_none() {
                    session_name = Some(arg);
                }
            }
            let session_name = session_name.ok_or("session name required")?;
            commands::history(&cfg, &session_name, format)
        }
        "attach" | "a" => {
            let mut detach_flag = false;
            let mut session_name: Option<String> = None;
            let mut command_args: Vec<String> = Vec::new();
            for arg in args {
                if arg == "--detach" {
                    detach_flag = true;
                } else if session_name.is_none() {
                    session_name = Some(arg);
                } else {
                    command_args.push(arg);
                }
            }
            let name = session_name.ok_or("session name required")?;

            let spawn_command = if command_args.is_empty() {
                None
            } else {
                Some(command_args)
            };
            let cwd = std::env::current_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();

            let mut daemon = Daemon::new(&cfg, &name, spawn_command, cwd)?;
            client::attach(&mut daemon, detach_flag)
        }
        "run" | "r" => {
            let session_name = args.next().ok_or("session name required")?;
            let command_args: Vec<String> = args.collect();
            let cwd = std::env::current_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();

            let mut daemon = Daemon::new(&cfg, &session_name, None, cwd)?;
            commands::run(&mut daemon, &command_args)
        }
        _ => {
            print_help();
            Ok(())
        }
    }
}

fn print_version() {
    let mut out = String::new();
    out.push_str(&format!(
        "{:<20} {:<12} {}\n",
        "COMPONENT", "VERSION", "REV"
    ));
    out.push_str(&format!("{:<20} {:<12} {}\n", "zmx", VERSION, COMMIT));
    out.push_str(&format!("{:<20} {:<12} {}\n", "zmx-vt", "(built-in)", ""));
    ipc::print_stdout(&out);
}

fn print_help() {
    let help_text = r#"zmx - session persistence for terminal processes

Usage: zmx [-g <group>] <command> [args]

Global flags:
  -g, --group <name>            Session group (default: "default", or $ZMX_GROUP)

Commands:
  [a]ttach <name> [command...] [--detach]
                                Attach to session, creating session if needed
                                (--detach: ensure session exists, print status, and exit
                                without attaching; flag is accepted in any position)
  [f]ork [<name>]               Fork current session (same cmd + cwd) into a new session
  [r]un <name> [command...]     Send command without attaching, creating session if needed
  [d]etach [<name>]              Detach all clients from current or named session
  [da] detach-all               Detach all clients from all sessions in group
  [gs] groups                   List active session groups
  [l]ist [--short] [-j|--json]  List active sessions in group
  [c]ompletions <shell>         Completion scripts for shell integration (bash, zsh, or fish)
  [k]ill <name>                 Kill a session and all attached clients
  [hi]story <name> [--vt|--html] Output session scrollback (--vt or --html for escape sequences)
  [v]ersion                     Show version information
  [h]elp                        Show this help message
"#;
    ipc::print_stdout(help_text);
}
