# zmx session groups — Design

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement
> this plan task-by-task.

**Goal:** Add session groups to zmx so users can organize sessions into isolated
namespaces. Each group has its own socket directory and log directory. All
existing commands respect a `-g` / `--group` flag.

**Architecture:** Groups are filesystem directories — no central process. Each
session remains its own independent daemon. A group is implicitly created when a
session is first attached in it, and empty group directories are left in place
(harmless, cleaned on reboot or manually).

**Tech Stack:** Zig 0.15.2, existing zmx IPC and session machinery.

**Breaking change:** Existing sessions created before this change are not
migrated. Users must kill old sessions and re-create them.

---

## Design

### Directory Layout

Sockets and logs are separated into distinct directory trees:

```
Sockets: $ZMX_DIR | $XDG_STATE_HOME/zmx | ~/.local/state/zmx
Logs:    $ZMX_LOG_DIR | $XDG_LOG_HOME/zmx | ~/.local/logs/zmx
```

```
~/.local/state/zmx/          # socket_dir
  default/                   # default group
    session-a                # unix socket
    session-b
  work/                      # named group
    project-x

~/.local/logs/zmx/           # log_dir
  zmx.log                   # global log
  default/
    session-a.log
  work/
    project-x.log
```

### Socket Directory Fallback Chain

Priority order (unchanged logic, new default):

1. `$ZMX_DIR` — explicit override
2. `$XDG_STATE_HOME/zmx` — if `XDG_STATE_HOME` is set
3. `~/.local/state/zmx` — XDG default

### Log Directory Fallback Chain

New, separate from sockets:

1. `$ZMX_LOG_DIR` — explicit override
2. `$XDG_LOG_HOME/zmx` — if `XDG_LOG_HOME` is set (custom convention)
3. `~/.local/logs/zmx` — default

### CLI Changes

**New global flag:** `-g <name>` / `--group <name>`

- Parsed before the subcommand
- Default value: `"default"`
- All existing commands respect it

```
zmx -g work attach foo
zmx -g work list
zmx -g work kill bar
zmx -g work detach-all
zmx -g work fork
zmx -g work run foo echo hello
zmx -g work history foo
```

**New command:** `groups`

Lists group directories that contain at least one active socket:

```
$ zmx groups
default
work
```

### Environment Variables

Two environment variables are set in the spawned shell:

- `ZMX_SESSION` — session name (unchanged)
- `ZMX_GROUP` — group name (new)

The `fork` command reads `ZMX_GROUP` to inherit the group. An explicit `-g` flag
overrides the inherited value.

### Behavioral Scoping

All commands without `-g` operate on the `default` group only:

- `zmx list` — lists default group sessions only
- `zmx detach-all` — detaches all clients in default group only
- `zmx -g work list` — lists work group sessions only
- `zmx -g work detach-all` — detaches all clients in work group only

There are no cross-group operations.

### Group Name Validation

Invalid group names are rejected at parse time:

- Empty string
- Contains `/` or `..`
- Names are percent-encoded the same way session names are

### Implementation Approach

Extend the existing `Daemon` struct with a `group` field. The `Cfg` struct gains
separate `socket_dir` and `log_dir` fields with the new fallback chains.

```zig
const Cfg = struct {
    socket_base: []const u8,  // e.g., ~/.local/state/zmx
    log_base: []const u8,     // e.g., ~/.local/logs/zmx
};
```

Socket paths become `{socket_base}/{group}/{session_name}`.
Log paths become `{log_base}/{group}/{session_name}.log`.

The group directory is created (`mkdir`) on first `attach` or `run` in that
group.

### Shell Completions

Updates needed for bash, zsh, and fish:

- Add `-g` / `--group` flag completion
- Session name completion respects the current `-g` value (or defaults to
  `default/`)
- Add `groups` to completable commands

### Error Cases

| Condition                    | Behavior                         |
| ---------------------------- | -------------------------------- |
| Invalid group name           | Error: "invalid group name: ..." |
| Group directory doesn't exist| No sessions found (not an error) |
| `$ZMX_GROUP` not set in fork | Uses default group               |

### Not In Scope

- No explicit group management commands (create/delete)
- No cross-group operations
- No auto-cleanup of empty group directories
- No migration of pre-existing sessions
