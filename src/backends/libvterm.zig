const std = @import("std");
const terminal = @import("../terminal.zig");

const c = @cImport({
    @cInclude("vterm.h");
});

/// libvterm backend implementation for the terminal interface
pub const LibvtermBackend = struct {
    vt: *c.VTerm,
    screen: *c.VTermScreen,
    rows: u16,
    cols: u16,

    pub const StreamImpl = struct {
        vt: *c.VTerm,

        pub fn nextSlice(self: *StreamImpl, data: []const u8) !void {
            _ = c.vterm_input_write(self.vt, data.ptr, data.len);
        }

        pub fn deinit(self: *StreamImpl) void {
            _ = self;
            // Nothing to clean up - the VTerm owns this
        }
    };

    pub fn init(_: std.mem.Allocator, cols: u16, rows: u16, _: usize) !LibvtermBackend {
        const vt = c.vterm_new(rows, cols) orelse return error.OutOfMemory;
        errdefer c.vterm_free(vt);

        c.vterm_set_utf8(vt, 1);

        const screen = c.vterm_obtain_screen(vt) orelse return error.OutOfMemory;
        c.vterm_screen_reset(screen, 1);

        return .{
            .vt = vt,
            .screen = screen,
            .rows = rows,
            .cols = cols,
        };
    }

    pub fn deinit(self: *LibvtermBackend, _: std.mem.Allocator) void {
        c.vterm_free(self.vt);
    }

    pub fn resize(self: *LibvtermBackend, _: std.mem.Allocator, cols: u16, rows: u16) !void {
        c.vterm_set_size(self.vt, rows, cols);
        self.rows = rows;
        self.cols = cols;
    }

    pub fn getCursor(self: *LibvtermBackend) terminal.Cursor {
        const state = c.vterm_obtain_state(self.vt);
        var pos: c.VTermPos = undefined;
        c.vterm_state_get_cursorpos(state, &pos);
        return .{
            .x = @intCast(pos.col),
            .y = @intCast(pos.row),
            .pending_wrap = false, // libvterm doesn't expose this directly
        };
    }

    pub fn vtStream(self: *LibvtermBackend) StreamImpl {
        return .{ .vt = self.vt };
    }

    /// Serialize terminal state for session restoration.
    /// Includes screen content and cursor position.
    pub fn serializeState(self: *LibvtermBackend, alloc: std.mem.Allocator) error{OutOfMemory}!?[]const u8 {
        return self.serializeScreen(alloc, true);
    }

    /// Serialize terminal content in the specified format.
    /// Note: HTML format is not supported by libvterm backend.
    pub fn serialize(self: *LibvtermBackend, alloc: std.mem.Allocator, format: terminal.Format) error{OutOfMemory}!?[]const u8 {
        return switch (format) {
            .plain => self.serializePlain(alloc),
            .vt => self.serializeScreen(alloc, false),
            .html => null, // HTML not implemented for libvterm - would require cell attribute access
        };
    }

    fn serializePlain(self: *LibvtermBackend, alloc: std.mem.Allocator) error{OutOfMemory}!?[]const u8 {
        // Use vterm_screen_get_text to extract text row by row
        const max_line_len = self.cols * 4 + 1; // UTF-8 max 4 bytes per char + newline
        const line_buf = try alloc.alloc(u8, max_line_len);
        defer alloc.free(line_buf);

        var buf: std.ArrayList(u8) = .empty;
        errdefer buf.deinit(alloc);

        var row: c_int = 0;
        while (row < self.rows) : (row += 1) {
            const rect = c.VTermRect{
                .start_row = row,
                .end_row = row + 1,
                .start_col = 0,
                .end_col = self.cols,
            };

            const len = c.vterm_screen_get_text(self.screen, line_buf.ptr, max_line_len, rect);
            if (len > 0) {
                // Trim trailing spaces for cleaner output
                var end: usize = @intCast(len);
                while (end > 0 and line_buf[end - 1] == ' ') {
                    end -= 1;
                }
                try buf.appendSlice(alloc, line_buf[0..end]);
            }
            try buf.append(alloc, '\n');
        }

        return @as(?[]const u8, try buf.toOwnedSlice(alloc));
    }

    fn serializeScreen(self: *LibvtermBackend, alloc: std.mem.Allocator, include_cursor: bool) error{OutOfMemory}!?[]const u8 {
        // For VT serialization without cell access, we use a simpler approach:
        // output the text content with cursor positioning.
        // Note: This doesn't preserve colors/attributes because libvterm's
        // VTermScreenCell struct is opaque from Zig's @cImport perspective.

        var buf: std.ArrayList(u8) = .empty;
        errdefer buf.deinit(alloc);

        // Clear screen and home cursor before restoring content
        try buf.appendSlice(alloc, "\x1b[2J\x1b[H");

        const max_line_len = self.cols * 4 + 1;
        const line_buf = try alloc.alloc(u8, max_line_len);
        defer alloc.free(line_buf);

        var row: c_int = 0;
        while (row < self.rows) : (row += 1) {
            if (row > 0) {
                try buf.appendSlice(alloc, "\r\n");
            }

            const rect = c.VTermRect{
                .start_row = row,
                .end_row = row + 1,
                .start_col = 0,
                .end_col = self.cols,
            };

            const len = c.vterm_screen_get_text(self.screen, line_buf.ptr, max_line_len, rect);
            if (len > 0) {
                try buf.appendSlice(alloc, line_buf[0..@intCast(len)]);
            }
        }

        // Position cursor to match original location
        if (include_cursor) {
            const cursor = self.getCursor();
            var cursor_buf: [32]u8 = undefined;
            // bufPrint on a stack buffer cannot fail with OOM
            const cursor_seq = std.fmt.bufPrint(&cursor_buf, "\x1b[{d};{d}H", .{
                cursor.y + 1,
                cursor.x + 1,
            }) catch unreachable;
            try buf.appendSlice(alloc, cursor_seq);
        }

        return @as(?[]const u8, try buf.toOwnedSlice(alloc));
    }
};
