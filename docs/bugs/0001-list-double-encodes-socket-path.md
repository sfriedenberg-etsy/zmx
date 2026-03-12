# 0001: list double-encodes socket path, deleting live sessions

## Summary

`zmx list` destroys every live session it touches. It double-encodes the
socket filename when probing, fails to connect, concludes the session is
stale, and deletes the real socket file.

## Root Cause

`list()` at `src/main.zig:630` passes a directory entry name to
`getSocketPath()`:

```zig
const socket_path = try getSocketPath(alloc, cfg.socket_dir, entry.name);
```

`entry.name` is the **already-encoded** filename from the directory listing
(e.g. `purse-first%2Fsc-zmx-fixes`). `getSocketPath()` calls
`encodeSessionName()` internally, which encodes it again — turning `%2F` into
`%252F`.

The probe connects to the double-encoded path, which doesn't exist. The
connection fails, so `list` calls `cleanupStaleSocket(dir, entry.name)` —
which deletes the **correctly-named** socket file that the live daemon is
listening on.

## Affected Code

- `list()` — `src/main.zig:630`: passes encoded name to `getSocketPath()`
- `getSocketPath()` — `src/main.zig:1806-1816`: always calls
  `encodeSessionName()`, assumes raw input
- `cleanupStaleSocket()` — `src/main.zig:1739-1744`: deletes the real socket

## Contrast with Correct Usage

At `src/main.zig:493`, `getSocketPath()` is called with a raw
user-provided session name (e.g. `purse-first/sc-zmx-fixes`), which is
correctly encoded once.

## Impact

- Every `zmx list` or `zmx -g sc list` call deletes sockets for all live
  sessions whose names contain characters that get percent-encoded (i.e. any
  name with `/`, `%`, or `\`)
- The daemon processes keep running but become unreachable — no new clients
  can connect
- `spinclass status` calls `zmx -g sc list` to detect active sessions,
  so merely checking status destroys the sessions it's checking

## Fix

In `list()`, build the socket path by joining `cfg.socket_dir` and
`entry.name` directly, without re-encoding:

```zig
const socket_path = try std.fs.path.join(alloc, &.{ cfg.socket_dir, entry.name });
```

Audit all other callers of `getSocketPath()` that may pass pre-encoded names.
