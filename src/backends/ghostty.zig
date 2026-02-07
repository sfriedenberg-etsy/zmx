const std = @import("std");
const ghostty_vt = @import("ghostty-vt");
const terminal = @import("../terminal.zig");

/// Ghostty-vt backend implementation for the terminal interface
pub const GhosttyBackend = struct {
    term: ghostty_vt.Terminal,

    pub const StreamImpl = struct {
        stream: @TypeOf(@as(*ghostty_vt.Terminal, undefined).vtStream()),

        pub fn nextSlice(self: *StreamImpl, data: []const u8) !void {
            return self.stream.nextSlice(data);
        }

        pub fn deinit(self: *StreamImpl) void {
            self.stream.deinit();
        }
    };

    pub fn init(alloc: std.mem.Allocator, cols: u16, rows: u16, max_scrollback: usize) !GhosttyBackend {
        return .{
            .term = try ghostty_vt.Terminal.init(alloc, .{
                .cols = cols,
                .rows = rows,
                .max_scrollback = max_scrollback,
            }),
        };
    }

    pub fn deinit(self: *GhosttyBackend, alloc: std.mem.Allocator) void {
        self.term.deinit(alloc);
    }

    pub fn resize(self: *GhosttyBackend, alloc: std.mem.Allocator, cols: u16, rows: u16) !void {
        return self.term.resize(alloc, cols, rows);
    }

    pub fn getCursor(self: *GhosttyBackend) terminal.Cursor {
        const cursor = &self.term.screens.active.cursor;
        return .{
            .x = cursor.x,
            .y = cursor.y,
            .pending_wrap = cursor.pending_wrap,
        };
    }

    pub fn vtStream(self: *GhosttyBackend) StreamImpl {
        return .{ .stream = self.term.vtStream() };
    }

    /// Serialize terminal state for session restoration.
    /// Includes modes, scrolling region, pwd, and keyboard state but excludes
    /// palette (terminal has its own) and tabstops (shell will restore).
    pub fn serializeState(self: *GhosttyBackend, alloc: std.mem.Allocator) error{OutOfMemory}!?[]const u8 {
        var builder: std.Io.Writer.Allocating = .init(alloc);
        defer builder.deinit();

        var term_formatter = ghostty_vt.formatter.TerminalFormatter.init(&self.term, .vt);
        term_formatter.content = .{ .selection = null };
        term_formatter.extra = .{
            .palette = false, // Terminal emulator has its own palette
            .modes = true,
            .scrolling_region = true,
            .tabstops = false, // Shell restores its own tabstops
            .pwd = true,
            .keyboard = true,
            .screen = .all,
        };

        term_formatter.format(&builder.writer) catch |err| {
            // Format errors are typically I/O errors on the writer, which for
            // an allocating writer means OOM
            @branchHint(.unlikely);
            std.log.warn("failed to format terminal state err={s}", .{@errorName(err)});
            return error.OutOfMemory;
        };

        const output = builder.writer.buffered();
        if (output.len == 0) return null;

        return try alloc.dupe(u8, output);
    }

    /// Serialize terminal content in the specified format.
    /// Plain format returns text only, VT includes escape sequences for full state,
    /// HTML wraps content with inline styles.
    pub fn serialize(self: *GhosttyBackend, alloc: std.mem.Allocator, format: terminal.Format) error{OutOfMemory}!?[]const u8 {
        var builder: std.Io.Writer.Allocating = .init(alloc);
        defer builder.deinit();

        const opts: ghostty_vt.formatter.Options = switch (format) {
            .plain => .plain,
            .vt => .vt,
            .html => .html,
        };
        var term_formatter = ghostty_vt.formatter.TerminalFormatter.init(&self.term, opts);
        term_formatter.content = .{ .selection = null };
        term_formatter.extra = switch (format) {
            .plain => .none,
            .vt => .{
                .palette = false, // Terminal emulator has its own palette
                .modes = true,
                .scrolling_region = true,
                .tabstops = false, // Shell restores its own tabstops
                .pwd = true,
                .keyboard = true,
                .screen = .all,
            },
            .html => .styles,
        };

        term_formatter.format(&builder.writer) catch |err| {
            @branchHint(.unlikely);
            std.log.warn("failed to format terminal err={s}", .{@errorName(err)});
            return error.OutOfMemory;
        };

        const output = builder.writer.buffered();
        if (output.len == 0) return null;

        return try alloc.dupe(u8, output);
    }
};
