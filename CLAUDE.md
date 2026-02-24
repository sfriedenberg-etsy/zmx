# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

zmx is a terminal session persistence tool (alternative to tmux) written in Zig. It allows attaching and detaching from terminal sessions without killing underlying processes, delegating window management to the OS window manager. Uses a daemon-per-session architecture with Unix socket IPC.

## Build & Test Commands

```sh
just build          # Build zmx-libvterm via Nix (default target)
just test           # Run unit tests via Nix
just check          # Zig compilation check (for IDE integration)
just zig2nix        # Update zig2nix dependency hashes from build.zig.zon
```

Direct zig commands (via nix devshell):
```sh
nix run .#build -- check              # Compilation check
nix run .#test                        # Run tests
nix run .#test -- -Dtest-filter=<name> # Run specific test
zig fmt .                             # Format code
```

Two build variants:
- `zmx` (default): Uses ghostty-vt backend
- `zmx-libvterm`: Uses libvterm-neovim backend, wrapped with library paths

## Architecture

### Daemon-Client Model

Each session runs a dedicated daemon process that manages a PTY and connected clients. Communication uses a custom binary protocol over Unix sockets (`src/ipc.zig`). Message types: Input, Output, Resize, Detach, DetachAll, Kill, Info, Init, History, Run, Ack.

### Backend System

Compile-time polymorphic terminal backends via `src/terminal.zig`:
- **ghostty** (`src/backends/ghostty.zig`): Default. Full feature set including HTML serialization, palette management, keyboard state restoration.
- **libvterm** (`src/backends/libvterm.zig`): C FFI to libvterm-neovim. Plain text and VT format only.

Selected at build time with `-Dbackend=ghostty|libvterm`.

### Key Source Files

- `src/main.zig`: Entry point, CLI parsing, daemon/client event loops, PTY management (~2000 lines)
- `src/terminal.zig`: Generic `Terminal(Impl)` and `VtStream(Impl)` interfaces
- `src/ipc.zig`: Binary message protocol (5-byte header + payload)
- `src/log.zig`: File-based logging with 5MB rotation
- `src/completions.zig`: Embedded shell completion scripts (bash/zsh/fish)

### Session Organization

Sessions are organized into groups (`-g`/`--group` flag, `ZMX_GROUP` env var). Socket paths use URL percent-encoding for session names. Socket directory resolution: `ZMX_DIR` > `XDG_RUNTIME_DIR/zmx` > `TMPDIR/zmx-{uid}` > `/tmp/zmx-{uid}`.

### PTY Management

Platform-specific: `forkpty()` on macOS/FreeBSD, `openpty()` on Linux. Uses `poll()` for non-blocking multiplexed I/O between PTY and clients. Uses `std.heap.c_allocator` (not DebugAllocator) for fork compatibility.

## Finding APIs

Use `zigdoc` to look up library APIs before grepping:
```sh
zigdoc ghostty-vt
zigdoc std.ArrayList
```

Source inspection directories: `zig_std_src/` (stdlib), `ghostty_src/` (ghostty-vt).

## Issue Tracking

Uses bd (beads) for issue tracking. Run `bd quickstart` to learn usage.

## Nix Flake

Follows the stable-first nixpkgs convention: `nixpkgs` (stable) and `nixpkgs-master` (unstable). Uses `zig2nix` for Zig build integration.
