# TODO

## Validate socket path length before bind

When a session name with slashes is percent-encoded (e.g.,
`eng/purse-first/other-marketplaces` becomes
`eng%2Fpurse-first%2Fother-marketplaces`), the full socket path can exceed
macOS's 104-byte `sockaddr_un.sun_path` limit. `std.net.Address.initUnix`
returns `error.NameTooLong`, which propagates up and exits with code 1 but
no user-visible error message (ReleaseSafe builds don't print error return
traces).

**Fix:** In `getSocketPath()` (main.zig), after building the full path,
validate the length against `std.net.Address.unix_path_max` and print a
clear error to stderr:

```zig
if (fname.len >= comptime std.net.Address.unix_path_max) {
    const w = std.io.getStdErr().writer();
    w.print("error: socket path too long ({d} bytes, max {d}): {s}\n", .{
        fname.len, std.net.Address.unix_path_max - 1, fname,
    }) catch {};
    return error.SocketPathTooLong;
}
```
