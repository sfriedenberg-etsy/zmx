# Session Groups — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement
> this plan task-by-task.

**Goal:** Add `-g`/`--group` flag to zmx so sessions are organized into named
groups (filesystem directories), with separate socket and log directory trees.

**Architecture:** The `Cfg` struct gains separate `socket_base` and `log_base`
fields with new fallback chains. A `group` field (default `"default"`) is parsed
from CLI flags and threaded through all functions that use `cfg.socket_dir`.
Socket paths become `{socket_base}/{group}/{session}`. Log paths become
`{log_base}/{group}/{session}.log`. A new `groups` command lists active groups.

**Tech Stack:** Zig 0.15.2, ghostty-vt, Unix domain sockets.

---

### Task 1: Change `Cfg` to separate socket and log directories

The `Cfg` struct currently has `socket_dir` and `log_dir` where `log_dir` is
derived as `{socket_dir}/logs`. We need to split these into independent base
directories with separate fallback chains, and add a `group` field whose
subdirectory is appended to each base.

**Files:**
- Modify: `src/main.zig:76-121` (Cfg struct, init, deinit, mkdir)

**Step 1: Rewrite the `Cfg` struct and its `init`**

Replace the Cfg struct (lines 76-121) with:

```zig
const Cfg = struct {
    socket_base: []const u8,
    log_base: []const u8,
    group: []const u8,
    socket_dir: []const u8,
    log_dir: []const u8,
    max_scrollback: usize = 10_000_000,

    pub fn init(alloc: std.mem.Allocator, group: []const u8) !Cfg {
        const home = posix.getenv("HOME") orelse "/tmp";

        const socket_base: []const u8 = if (posix.getenv("ZMX_DIR")) |zmxdir|
            try alloc.dupe(u8, zmxdir)
        else if (posix.getenv("XDG_STATE_HOME")) |xdg_state|
            try std.fmt.allocPrint(alloc, "{s}/zmx", .{xdg_state})
        else
            try std.fmt.allocPrint(alloc, "{s}/.local/state/zmx", .{home});
        errdefer alloc.free(socket_base);

        const log_base: []const u8 = if (posix.getenv("ZMX_LOG_DIR")) |logdir|
            try alloc.dupe(u8, logdir)
        else if (posix.getenv("XDG_LOG_HOME")) |xdg_log|
            try std.fmt.allocPrint(alloc, "{s}/zmx", .{xdg_log})
        else
            try std.fmt.allocPrint(alloc, "{s}/.local/logs/zmx", .{home});
        errdefer alloc.free(log_base);

        const owned_group = try alloc.dupe(u8, group);
        errdefer alloc.free(owned_group);

        const socket_dir = try std.fs.path.join(alloc, &.{ socket_base, owned_group });
        errdefer alloc.free(socket_dir);

        const log_dir = try std.fs.path.join(alloc, &.{ log_base, owned_group });
        errdefer alloc.free(log_dir);

        var cfg = Cfg{
            .socket_base = socket_base,
            .log_base = log_base,
            .group = owned_group,
            .socket_dir = socket_dir,
            .log_dir = log_dir,
        };

        try cfg.mkdirAll();

        return cfg;
    }

    pub fn deinit(self: *Cfg, alloc: std.mem.Allocator) void {
        if (self.socket_base.len > 0) alloc.free(self.socket_base);
        if (self.log_base.len > 0) alloc.free(self.log_base);
        if (self.group.len > 0) alloc.free(self.group);
        if (self.socket_dir.len > 0) alloc.free(self.socket_dir);
        if (self.log_dir.len > 0) alloc.free(self.log_dir);
    }

    fn mkdirAll(self: *Cfg) !void {
        // Create base directories
        try mkdirRecursive(self.socket_base);
        try mkdirRecursive(self.log_base);
        // Create group subdirectories
        try mkdirRecursive(self.socket_dir);
        try mkdirRecursive(self.log_dir);
    }
};

fn mkdirRecursive(path: []const u8) !void {
    std.fs.makeDirAbsolute(path) catch |err| switch (err) {
        error.PathAlreadyExists => {},
        error.FileNotFound => {
            // Parent doesn't exist, try creating it
            if (std.fs.path.dirname(path)) |parent| {
                try mkdirRecursive(parent);
                std.fs.makeDirAbsolute(path) catch |e| switch (e) {
                    error.PathAlreadyExists => {},
                    else => return e,
                };
            } else {
                return err;
            }
        },
        else => return err,
    };
}
```

**Step 2: Build and verify it compiles**

Run: `just check`
Expected: Compile errors in `main()` because `Cfg.init` signature changed (now
requires `group` argument). This is expected — we fix it in step 3.

**Step 3: Update `main()` to pass group to `Cfg.init`**

In `main()` (line 343), change:
```zig
var cfg = try Cfg.init(alloc);
```
to:
```zig
var cfg = try Cfg.init(alloc, "default");
```

Also update the log path line (346) — the global log now lives in `log_base`, not
`log_dir`:
```zig
const log_path = try std.fs.path.join(alloc, &.{ cfg.log_base, "zmx.log" });
```

**Step 4: Build and verify it compiles**

Run: `just check`
Expected: Compiles clean.

**Step 5: Commit**

```
refactor: separate socket and log base directories in Cfg
```

---

### Task 2: Parse `-g`/`--group` global flag

Add CLI argument parsing for the group flag before the subcommand.

**Files:**
- Modify: `src/main.zig:335-353` (main function, argument parsing)

**Step 1: Add group flag parsing before subcommand dispatch**

Replace the argument parsing section in `main()` (lines 340-353) with:

```zig
    defer args.deinit();
    _ = args.skip(); // skip program name

    // Parse global flags before subcommand
    var group: []const u8 = "default";
    var cmd: ?[]const u8 = null;

    while (args.next()) |arg| {
        if (std.mem.eql(u8, arg, "-g") or std.mem.eql(u8, arg, "--group")) {
            group = args.next() orelse {
                std.log.err("--group requires a value", .{});
                return error.MissingGroupValue;
            };
            // Validate group name
            if (group.len == 0) {
                std.log.err("group name cannot be empty", .{});
                return error.InvalidGroupName;
            }
            if (std.mem.indexOf(u8, group, "/") != null or std.mem.indexOf(u8, group, "..") != null) {
                std.log.err("invalid group name: {s}", .{group});
                return error.InvalidGroupName;
            }
        } else {
            cmd = arg;
            break;
        }
    }

    var cfg = try Cfg.init(alloc, group);
    defer cfg.deinit(alloc);

    const log_path = try std.fs.path.join(alloc, &.{ cfg.log_base, "zmx.log" });
    defer alloc.free(log_path);
    try log_system.init(alloc, log_path);
    defer log_system.deinit();

    const command = cmd orelse {
        return list(&cfg, false);
    };
```

**Step 2: Update the dispatch block to use `command` instead of `cmd`**

The dispatch block (starting at line 355) currently matches on `cmd`. Update all
occurrences of `cmd` to `command` in the `if/else if` chain. The variable name
changes from `cmd` to `command` since we now use `cmd` as a mutable variable
during flag parsing.

Specifically, replace every `std.mem.eql(u8, cmd, ...)` with
`std.mem.eql(u8, command, ...)` in the dispatch chain (lines 355-463).

**Step 3: Build and verify**

Run: `just check`
Expected: Compiles clean.

**Step 4: Commit**

```
feat: parse -g/--group global flag for session groups
```

---

### Task 3: Set `ZMX_GROUP` in spawned shell

The daemon needs to set `ZMX_GROUP` alongside `ZMX_SESSION` so child processes
know which group they're in.

**Files:**
- Modify: `src/main.zig:1504-1555` (spawnPty function)

**Step 1: Add ZMX_GROUP to the spawned process environment**

In `spawnPty()`, after the `ZMX_SESSION` putenv (line 1520-1521), add:

```zig
        const group_env = try std.fmt.allocPrint(daemon.alloc, "ZMX_GROUP={s}\x00", .{daemon.cfg.group});
        _ = c.putenv(@ptrCast(group_env.ptr));
```

**Step 2: Build and verify**

Run: `just check`
Expected: Compiles clean.

**Step 3: Commit**

```
feat: set ZMX_GROUP env var in spawned shell sessions
```

---

### Task 4: Update `fork` to inherit group from `ZMX_GROUP`

The `fork` command needs to read `ZMX_GROUP` to inherit the source session's
group. An explicit `-g` flag overrides this.

**Files:**
- Modify: `src/main.zig:707-732` (forkSession function)
- Modify: `src/main.zig:735-826` (fork function)

**Step 1: Read `ZMX_GROUP` in `forkSession` for auto-naming**

In `forkSession()` (line 726), the `cfg.socket_dir` is used to open the
directory for `nextForkName`. Since `cfg` already has the group baked into
`socket_dir` (from `-g` flag or default), this should work correctly without
changes if the user passed `-g`.

However, when `-g` is NOT passed and we're inside a session, we should inherit
`ZMX_GROUP`. This requires reading `ZMX_GROUP` before `Cfg.init`. The cleanest
approach: in `main()`, before calling `Cfg.init`, check if the fork command is
being used and `ZMX_GROUP` is set, and use that as the default group.

Update the group defaulting logic in `main()` (from Task 2) to read from
`ZMX_GROUP` if no explicit `-g` flag was given:

```zig
    // Parse global flags before subcommand
    var group: []const u8 = posix.getenv("ZMX_GROUP") orelse "default";
    var explicit_group = false;
    var cmd: ?[]const u8 = null;

    while (args.next()) |arg| {
        if (std.mem.eql(u8, arg, "-g") or std.mem.eql(u8, arg, "--group")) {
            group = args.next() orelse {
                std.log.err("--group requires a value", .{});
                return error.MissingGroupValue;
            };
            explicit_group = true;
            // Validate group name
            if (group.len == 0) {
                std.log.err("group name cannot be empty", .{});
                return error.InvalidGroupName;
            }
            if (std.mem.indexOf(u8, group, "/") != null or std.mem.indexOf(u8, group, "..") != null) {
                std.log.err("invalid group name: {s}", .{group});
                return error.InvalidGroupName;
            }
        } else {
            cmd = arg;
            break;
        }
    }
```

Wait — actually this is simpler. If `ZMX_GROUP` is the default when `-g` isn't
given, then ALL commands (not just fork) automatically inherit the group context
when run from inside a session. This is the correct behavior: if you're inside a
`work` group session, `zmx list` should show `work` sessions, `zmx attach foo`
should attach in the `work` group, etc.

But the design says "no -g means default group". Let's reconsider: the design
says fork reads ZMX_GROUP, but other commands default to "default". However,
it actually makes more sense for ALL commands to inherit ZMX_GROUP when inside a
session — otherwise `zmx list` from inside a `work` session would show the wrong
sessions.

**Resolution:** Default to `ZMX_GROUP` if set, otherwise `"default"`. This is
consistent and intuitive. The `-g` flag always overrides.

**Step 2: Build and verify**

Run: `just check`
Expected: Compiles clean.

**Step 3: Commit**

```
feat: inherit ZMX_GROUP from environment when -g not specified
```

---

### Task 5: Add `groups` command

New command that lists group directories containing active sockets.

**Files:**
- Modify: `src/main.zig` (add `listGroups` function and dispatch entry)

**Step 1: Write the `listGroups` function**

Add after the `list` function:

```zig
fn listGroups(cfg: *Cfg) !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const alloc = gpa.allocator();

    var base_dir = std.fs.openDirAbsolute(cfg.socket_base, .{ .iterate = true }) catch |err| switch (err) {
        error.FileNotFound => {
            // No socket base dir yet, no groups
            return;
        },
        else => return err,
    };
    defer base_dir.close();

    var groups = try std.ArrayList([]const u8).initCapacity(alloc, 8);
    defer {
        for (groups.items) |name| alloc.free(name);
        groups.deinit(alloc);
    }

    var iter = base_dir.iterate();
    while (try iter.next()) |entry| {
        if (entry.kind != .directory) continue;

        // Check if this group directory has any socket files
        var group_dir = base_dir.openDir(entry.name, .{ .iterate = true }) catch continue;
        defer group_dir.close();

        var group_iter = group_dir.iterate();
        var has_sockets = false;
        while (try group_iter.next()) |sub_entry| {
            if (sub_entry.kind == .unix_domain_socket or sub_entry.kind == .file) {
                has_sockets = true;
                break;
            }
        }

        if (has_sockets) {
            try groups.append(alloc, try alloc.dupe(u8, entry.name));
        }
    }

    // Sort alphabetically
    const S = struct {
        fn lessThan(_: void, a: []const u8, b: []const u8) bool {
            return std.mem.order(u8, a, b) == .lt;
        }
    };
    std.mem.sort([]const u8, groups.items, {}, S.lessThan);

    var buf: [4096]u8 = undefined;
    var w = std.fs.File.stdout().writer(&buf);
    for (groups.items) |name| {
        try w.interface.print("{s}\n", .{name});
    }
    try w.interface.flush();
}
```

**Step 2: Add dispatch entry**

In the command dispatch chain in `main()`, add before the `version` check:

```zig
    if (std.mem.eql(u8, command, "groups") or std.mem.eql(u8, command, "gs")) {
        return listGroups(&cfg);
    } else if ...
```

**Step 3: Build and verify**

Run: `just check`
Expected: Compiles clean.

**Step 4: Commit**

```
feat: add groups command to list active session groups
```

---

### Task 6: Update help text

**Files:**
- Modify: `src/main.zig:485-509` (help function)

**Step 1: Update the help text**

Replace the help text string with:

```zig
fn help() !void {
    const help_text =
        \\zmx - session persistence for terminal processes
        \\
        \\Usage: zmx [-g <group>] <command> [args]
        \\
        \\Global flags:
        \\  -g, --group <name>            Session group (default: "default", or $ZMX_GROUP)
        \\
        \\Commands:
        \\  [a]ttach <name> [command...]  Attach to session, creating session if needed
        \\  [f]ork [<name>]               Fork current session (same cmd + cwd) into a new session
        \\  [r]un <name> [command...]     Send command without attaching, creating session if needed
        \\  [d]etach [<name>]              Detach all clients from current or named session
        \\  [da] detach-all               Detach all clients from all sessions in group
        \\  [gs] groups                   List active session groups
        \\  [l]ist [--short]              List active sessions in group
        \\  [c]ompletions <shell>         Completion scripts for shell integration (bash, zsh, or fish)
        \\  [k]ill <name>                 Kill a session and all attached clients
        \\  [hi]story <name> [--vt|--html] Output session scrollback (--vt or --html for escape sequences)
        \\  [v]ersion                     Show version information
        \\  [h]elp                        Show this help message
        \\
    ;
    var buf: [4096]u8 = undefined;
    var w = std.fs.File.stdout().writer(&buf);
    try w.interface.print(help_text, .{});
    try w.interface.flush();
}
```

**Step 2: Build and verify**

Run: `just check`
Expected: Compiles clean.

**Step 3: Commit**

```
docs: update help text with group flag and groups command
```

---

### Task 7: Update shell completions

**Files:**
- Modify: `src/completions.zig`

**Step 1: Update bash completions**

Key changes:
- Add `groups` to the commands list
- Add `-g`/`--group` flag completion
- Session completion calls `zmx -g <group> list --short` when a group flag is
  present (but for simplicity, just use `zmx list --short` which respects
  `ZMX_GROUP`)

Update the `bash_completions` constant to:

```zig
const bash_completions =
    \\_zmx_completions() {
    \\  local cur prev words cword
    \\  COMPREPLY=()
    \\  cur="${COMP_WORDS[COMP_CWORD]}"
    \\  prev="${COMP_WORDS[COMP_CWORD-1]}"
    \\
    \\  local commands="attach run detach detach-all fork groups list completions kill history version help"
    \\
    \\  # Handle -g/--group flag
    \\  if [[ "$prev" == "-g" || "$prev" == "--group" ]]; then
    \\    local groups=$(zmx groups 2>/dev/null | tr '\n' ' ')
    \\    COMPREPLY=($(compgen -W "$groups" -- "$cur"))
    \\    return 0
    \\  fi
    \\
    \\  if [[ "$cur" == -* ]]; then
    \\    COMPREPLY=($(compgen -W "-g --group" -- "$cur"))
    \\    return 0
    \\  fi
    \\
    \\  # Find the subcommand (skip -g <group>)
    \\  local subcmd=""
    \\  local i=1
    \\  while [[ $i -lt $COMP_CWORD ]]; do
    \\    local word="${COMP_WORDS[$i]}"
    \\    if [[ "$word" == "-g" || "$word" == "--group" ]]; then
    \\      ((i+=2))
    \\      continue
    \\    fi
    \\    if [[ "$word" != -* ]]; then
    \\      subcmd="$word"
    \\      break
    \\    fi
    \\    ((i++))
    \\  done
    \\
    \\  if [[ -z "$subcmd" ]]; then
    \\    COMPREPLY=($(compgen -W "$commands" -- "$cur"))
    \\    return 0
    \\  fi
    \\
    \\  case "$subcmd" in
    \\    attach|run|detach|kill|history)
    \\      local sessions=$(zmx list --short 2>/dev/null | tr '\n' ' ')
    \\      COMPREPLY=($(compgen -W "$sessions" -- "$cur"))
    \\      ;;
    \\    completions)
    \\      COMPREPLY=($(compgen -W "bash zsh fish" -- "$cur"))
    \\      ;;
    \\    list)
    \\      COMPREPLY=($(compgen -W "--short" -- "$cur"))
    \\      ;;
    \\    *)
    \\      ;;
    \\  esac
    \\}
    \\
    \\complete -o bashdefault -o default -F _zmx_completions zmx
;
```

**Step 2: Update zsh completions**

Update the `zsh_completions` constant to add `groups` command and `-g` flag:

```zig
const zsh_completions =
    \\_zmx() {
    \\  local context state state_descr line
    \\  typeset -A opt_args
    \\
    \\  _arguments -C \
    \\    '(-g --group)'{-g,--group}'[Session group]:group:_zmx_groups' \
    \\    '1: :->commands' \
    \\    '2: :->args' \
    \\    '*: :->trailing' \
    \\    && return 0
    \\
    \\  case $state in
    \\    commands)
    \\      local -a commands
    \\      commands=(
    \\        'attach:Attach to session, creating if needed'
    \\        'run:Send command without attaching'
    \\        'detach:Detach all clients from current or named session'
    \\        'detach-all:Detach all clients from all sessions in group'
    \\        'fork:Fork current session with same command'
    \\        'groups:List active session groups'
    \\        'list:List active sessions in group'
    \\        'completions:Shell completion scripts'
    \\        'kill:Kill a session'
    \\        'history:Output session scrollback'
    \\        'version:Show version'
    \\        'help:Show help message'
    \\      )
    \\      _describe 'command' commands
    \\      ;;
    \\    args)
    \\      case $words[2] in
    \\        attach|a|detach|d|kill|k|run|r|history|hi)
    \\          _zmx_sessions
    \\          ;;
    \\        completions|c)
    \\          _values 'shell' 'bash' 'zsh' 'fish'
    \\          ;;
    \\        list|l)
    \\          _values 'options' '--short'
    \\          ;;
    \\      esac
    \\      ;;
    \\    trailing)
    \\      # Additional args for commands like 'attach' or 'run'
    \\      ;;
    \\  esac
    \\}
    \\
    \\_zmx_groups() {
    \\  local -a groups
    \\  local output=$(zmx groups 2>/dev/null)
    \\  if [[ -n "$output" ]]; then
    \\    groups+=(${(f)output})
    \\  fi
    \\  _describe 'group' groups
    \\}
    \\
    \\_zmx_sessions() {
    \\  local -a sessions
    \\
    \\  local local_sessions=$(zmx list --short 2>/dev/null)
    \\  if [[ -n "$local_sessions" ]]; then
    \\    sessions+=(${(f)local_sessions})
    \\  fi
    \\
    \\  _describe 'local session' sessions
    \\}
    \\
    \\compdef _zmx zmx
;
```

**Step 3: Update fish completions**

Update the `fish_completions` constant:

```zig
const fish_completions =
    \\complete -c zmx -f
    \\
    \\set -l subcommands attach run detach detach-all fork groups list completions kill history version help
    \\set -l no_subcmd "not __fish_seen_subcommand_from $subcommands"
    \\
    \\complete -c zmx -n $no_subcmd -s g -l group -d 'Session group' -r -a '(zmx groups 2>/dev/null)'
    \\
    \\complete -c zmx -n $no_subcmd -a attach -d 'Attach to session, creating if needed'
    \\complete -c zmx -n $no_subcmd -a run -d 'Send command without attaching'
    \\complete -c zmx -n $no_subcmd -a detach -d 'Detach all clients from current or named session'
    \\complete -c zmx -n $no_subcmd -a detach-all -d 'Detach all clients from all sessions in group'
    \\complete -c zmx -n $no_subcmd -a fork -d 'Fork current session with same command'
    \\complete -c zmx -n $no_subcmd -a groups -d 'List active session groups'
    \\complete -c zmx -n $no_subcmd -a list -d 'List active sessions in group'
    \\complete -c zmx -n $no_subcmd -a completions -d 'Shell completion scripts'
    \\complete -c zmx -n $no_subcmd -a kill -d 'Kill a session'
    \\complete -c zmx -n $no_subcmd -a history -d 'Output session scrollback'
    \\complete -c zmx -n $no_subcmd -a version -d 'Show version'
    \\complete -c zmx -n $no_subcmd -a help -d 'Show help message'
    \\
    \\complete -c zmx -n "__fish_seen_subcommand_from attach run detach kill history" -a '(zmx list --short 2>/dev/null)' -d 'Session name'
    \\
    \\complete -c zmx -n "__fish_seen_subcommand_from completions" -a 'bash zsh fish' -d 'Shell'
    \\
    \\complete -c zmx -n "__fish_seen_subcommand_from list" -l short -d 'Short output'
;
```

**Step 4: Build and verify**

Run: `just check`
Expected: Compiles clean.

**Step 5: Commit**

```
feat: update shell completions for -g/--group flag and groups command
```

---

### Task 8: Update session log path in `ensureSession`

The session-specific log path in `ensureSession` currently uses the encoded
session name. With groups, the log directory is already scoped to the group
(via `cfg.log_dir`), so the session log filename stays the same — just the
directory changes. Verify this is correct.

**Files:**
- Modify: `src/main.zig:976-979` (session log path in ensureSession)

**Step 1: Verify the log path construction**

The current code (lines 976-979):
```zig
const session_log_name = try std.fmt.allocPrint(daemon.alloc, "{s}.log", .{encoded_name});
defer daemon.alloc.free(session_log_name);
const session_log_path = try std.fs.path.join(daemon.alloc, &.{ daemon.cfg.log_dir, session_log_name });
```

This already uses `daemon.cfg.log_dir` which now includes the group path
(`{log_base}/{group}/`). The session log filename is `{encoded_name}.log`.
This produces `{log_base}/{group}/{encoded_name}.log` which is correct.

No code changes needed — just verify this compiles and the paths are correct.

**Step 2: Build the full project**

Run: `just build`
Expected: Full build succeeds.

**Step 3: Commit (if any fixes were needed)**

```
fix: ensure session log paths use group-scoped log directory
```

---

### Task 9: Manual integration test

**Step 1: Build zmx**

Run: `just build`

**Step 2: Test default group behavior**

```bash
./result/bin/zmx attach test-session
# Inside session:
echo $ZMX_GROUP   # should print "default"
echo $ZMX_SESSION # should print "test-session"
# Detach with Ctrl+\
```

**Step 3: Verify session is in default group directory**

```bash
ls ~/.local/state/zmx/default/
# should show: test-session
```

**Step 4: Test named group**

```bash
./result/bin/zmx -g work attach project-x
# Inside session:
echo $ZMX_GROUP   # should print "work"
# Detach with Ctrl+\
```

**Step 5: Verify group isolation**

```bash
./result/bin/zmx list
# should show only: test-session (default group)

./result/bin/zmx -g work list
# should show only: project-x

./result/bin/zmx groups
# should show: default, work
```

**Step 6: Test fork inherits group**

```bash
./result/bin/zmx -g work attach project-x
# Inside session:
zmx fork
# Detach with Ctrl+\

./result/bin/zmx -g work list
# should show: project-x, project-x-1
```

**Step 7: Test group flag with other commands**

```bash
./result/bin/zmx -g work kill project-x-1
./result/bin/zmx -g work list
# should show only: project-x
```

**Step 8: Clean up**

```bash
./result/bin/zmx kill test-session
./result/bin/zmx -g work kill project-x
```

**Step 9: Verify logs are in separate directory**

```bash
ls ~/.local/logs/zmx/
# should show: zmx.log, default/, work/
ls ~/.local/logs/zmx/default/
# should show log files for default sessions
```
