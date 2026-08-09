# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with
code in this repository.

## Overview

zmx is a terminal session persistence tool (alternative to tmux) written in
Rust. It allows attaching and detaching from terminal sessions without killing
underlying processes, delegating window management to the OS window manager.
Uses a daemon-per-session architecture with Unix socket IPC. Terminal state
tracking is handled by a built-in VT emulator (`src/vt/`) with no external
terminal-emulation dependencies.

## Build & Test Commands

``` sh
just                # build + test (rust + bats), the CI-equivalent target
just build          # aggregate: build-nix
just test           # aggregate: test-rust test-bats
just test-rust      # Rust unit tests via cargo
just test-bats      # bats integration suite in the nix sandbox
just validate       # cargo check (for IDE integration)
```

Direct cargo commands:

``` sh
cargo build                  # Debug build
cargo build --release        # Release build
cargo test                   # Run unit tests
cargo test <name>            # Run a specific test
cargo fmt                    # Format code
cargo clippy                 # Lints
```

## Architecture

### Daemon-Client Model

Each session runs a dedicated daemon process that manages a PTY and connected
clients. Communication uses a custom binary protocol over Unix sockets
(`src/ipc.rs`): an 8-byte header { tag: u8, len: u32 LE, 3 pad bytes } plus
payload. Message types: Input, Output, Resize, Detach, DetachAll, Kill, Info,
Init, History, Run, Ack. The bats suite hand-encodes this framing — do not
change the wire format without updating `zz-tests_bats/common.bash`.

### Built-in Terminal Emulator

`src/vt/` is a from-scratch VT emulator that replaced the previous
ghostty-vt/libvterm backends:

- `src/vt/parser.rs`: vt500-style byte state machine (Ground/Escape/CSI/OSC/
  string states) with inline UTF-8 decoding.
- `src/vt/mod.rs`: terminal model — grid, scrollback (cell-count budget),
  alt screen, scroll regions, tab stops, SGR pen, modes (DECAWM, DECTCEM,
  bracketed paste, mouse, app cursor keys, ...).
- `src/vt/serialize.rs`: plain/VT/HTML serialization plus full state
  restoration (`serialize_state`) used to rehydrate re-attaching clients.

The emulator is record-only: query sequences that would need a response
written back to the application (DSR, DA, ...) are ignored, because zmx
passes PTY bytes straight through and the real terminal answers them.

### Key Source Files

- `src/main.rs`: Entry point, CLI parsing, dispatch
- `src/daemon.rs`: Daemon event loop, client bookkeeping, min-size PTY policy
- `src/client.rs`: Attach client loop (raw mode, detach key, IO relay)
- `src/ipc.rs`: Binary message protocol (8-byte header + payload)
- `src/pty.rs`: forkpty, winsize, raw-mode guard, poll helpers
- `src/session.rs`: Socket create/connect/probe/cleanup
- `src/title.rs`: OSC 0/2 window-title tracker (replayed on re-attach)
- `src/logger.rs`: File-based logging with 5MB rotation
- `src/completions.rs`: Embedded shell completion scripts (bash/zsh/fish)

### Session Organization

Sessions are organized into groups (`-g`/`--group` flag, `ZMX_GROUP` env var).
Socket paths use URL percent-encoding for session names. Socket directory
resolution: `ZMX_DIR` \> `XDG_STATE_HOME/zmx` \> `~/.local/state/zmx`, with a
per-group subdirectory.

### PTY Management

`forkpty()` for spawning the session process; `poll()` for non-blocking
multiplexed I/O between the PTY and clients. The daemon sizes the PTY to the
elementwise minimum across all attached clients (tmux `window-size smallest`).

## Issue Tracking

Uses bd (beads) for issue tracking. Run `bd quickstart` to learn usage.

## Nix Flake

Follows the stable-first nixpkgs convention: `nixpkgs` (stable) and
`nixpkgs-master` (unstable). Uses `rustPlatform.buildRustPackage` with
`cargoLock.lockFile`, so `Cargo.lock` must stay committed. Build logic is in
`package.nix`; the version is injected via `ZMX_VERSION`/`ZMX_COMMIT`
(see `build.rs`).
