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

        const screen = c.vterm_obtain_screen(vt);
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

    pub fn serializeState(self: *LibvtermBackend, alloc: std.mem.Allocator) ?[]const u8 {
        return self.serializeScreen(alloc, true);
    }

    pub fn serialize(self: *LibvtermBackend, alloc: std.mem.Allocator, format: terminal.Format) ?[]const u8 {
        return switch (format) {
            .plain => self.serializePlain(alloc),
            .vt => self.serializeScreen(alloc, false),
            .html => null, // HTML not implemented for libvterm
        };
    }

    fn serializePlain(self: *LibvtermBackend, alloc: std.mem.Allocator) ?[]const u8 {
        var buf = std.ArrayList(u8).init(alloc);
        errdefer buf.deinit();

        var row: c_int = 0;
        while (row < self.rows) : (row += 1) {
            var col: c_int = 0;
            var last_non_space: usize = 0;
            const row_start = buf.items.len;

            while (col < self.cols) : (col += 1) {
                var cell: c.VTermScreenCell = undefined;
                const pos = c.VTermPos{ .row = row, .col = col };
                _ = c.vterm_screen_get_cell(self.screen, pos, &cell);

                // Get the character (handle wide chars)
                if (cell.chars[0] != 0) {
                    var char_buf: [4]u8 = undefined;
                    const len = std.unicode.utf8Encode(@intCast(cell.chars[0]), &char_buf) catch 1;
                    buf.appendSlice(char_buf[0..len]) catch return null;
                    if (cell.chars[0] != ' ') {
                        last_non_space = buf.items.len;
                    }
                } else {
                    buf.append(' ') catch return null;
                }
            }

            // Trim trailing spaces
            buf.shrinkRetainingCapacity(if (last_non_space > row_start) last_non_space else row_start);
            buf.append('\n') catch return null;
        }

        return buf.toOwnedSlice() catch null;
    }

    fn serializeScreen(self: *LibvtermBackend, alloc: std.mem.Allocator, include_cursor: bool) ?[]const u8 {
        var buf = std.ArrayList(u8).init(alloc);
        errdefer buf.deinit();

        var last_fg: c.VTermColor = undefined;
        var last_bg: c.VTermColor = undefined;
        var last_attrs: c.VTermScreenCellAttrs = undefined;
        var has_style = false;

        // Clear screen and home cursor
        buf.appendSlice("\x1b[2J\x1b[H") catch return null;

        var row: c_int = 0;
        while (row < self.rows) : (row += 1) {
            if (row > 0) {
                buf.appendSlice("\r\n") catch return null;
            }

            var col: c_int = 0;
            while (col < self.cols) : (col += 1) {
                var cell: c.VTermScreenCell = undefined;
                const pos = c.VTermPos{ .row = row, .col = col };
                _ = c.vterm_screen_get_cell(self.screen, pos, &cell);

                // Check if style changed
                const style_changed = !has_style or
                    !colorsEqual(cell.fg, last_fg) or
                    !colorsEqual(cell.bg, last_bg) or
                    !attrsEqual(cell.attrs, last_attrs);

                if (style_changed) {
                    self.emitSGR(&buf, &cell) catch return null;
                    last_fg = cell.fg;
                    last_bg = cell.bg;
                    last_attrs = cell.attrs;
                    has_style = true;
                }

                // Output character
                if (cell.chars[0] != 0) {
                    var char_buf: [4]u8 = undefined;
                    const len = std.unicode.utf8Encode(@intCast(cell.chars[0]), &char_buf) catch 1;
                    buf.appendSlice(char_buf[0..len]) catch return null;
                } else {
                    buf.append(' ') catch return null;
                }
            }
        }

        // Reset attributes
        buf.appendSlice("\x1b[0m") catch return null;

        // Position cursor
        if (include_cursor) {
            const cursor = self.getCursor();
            var cursor_buf: [32]u8 = undefined;
            const cursor_seq = std.fmt.bufPrint(&cursor_buf, "\x1b[{d};{d}H", .{
                cursor.y + 1,
                cursor.x + 1,
            }) catch return null;
            buf.appendSlice(cursor_seq) catch return null;
        }

        return buf.toOwnedSlice() catch null;
    }

    fn emitSGR(self: *LibvtermBackend, buf: *std.ArrayList(u8), cell: *c.VTermScreenCell) !void {
        _ = self;
        // Reset and build new style
        try buf.appendSlice("\x1b[0");

        // Attributes
        if (cell.attrs.bold != 0) try buf.appendSlice(";1");
        if (cell.attrs.italic != 0) try buf.appendSlice(";3");
        if (cell.attrs.underline != 0) try buf.appendSlice(";4");
        if (cell.attrs.blink != 0) try buf.appendSlice(";5");
        if (cell.attrs.reverse != 0) try buf.appendSlice(";7");
        if (cell.attrs.strike != 0) try buf.appendSlice(";9");

        // Foreground color
        if (c.VTERM_COLOR_IS_RGB(&cell.fg)) {
            var color_buf: [20]u8 = undefined;
            const seq = std.fmt.bufPrint(&color_buf, ";38;2;{d};{d};{d}", .{
                cell.fg.rgb.red,
                cell.fg.rgb.green,
                cell.fg.rgb.blue,
            }) catch return;
            try buf.appendSlice(seq);
        } else if (c.VTERM_COLOR_IS_INDEXED(&cell.fg)) {
            var color_buf: [12]u8 = undefined;
            const idx = cell.fg.indexed.idx;
            if (idx < 8) {
                const seq = std.fmt.bufPrint(&color_buf, ";{d}", .{30 + idx}) catch return;
                try buf.appendSlice(seq);
            } else if (idx < 16) {
                const seq = std.fmt.bufPrint(&color_buf, ";{d}", .{90 + idx - 8}) catch return;
                try buf.appendSlice(seq);
            } else {
                const seq = std.fmt.bufPrint(&color_buf, ";38;5;{d}", .{idx}) catch return;
                try buf.appendSlice(seq);
            }
        }

        // Background color
        if (c.VTERM_COLOR_IS_RGB(&cell.bg)) {
            var color_buf: [20]u8 = undefined;
            const seq = std.fmt.bufPrint(&color_buf, ";48;2;{d};{d};{d}", .{
                cell.bg.rgb.red,
                cell.bg.rgb.green,
                cell.bg.rgb.blue,
            }) catch return;
            try buf.appendSlice(seq);
        } else if (c.VTERM_COLOR_IS_INDEXED(&cell.bg)) {
            var color_buf: [12]u8 = undefined;
            const idx = cell.bg.indexed.idx;
            if (idx < 8) {
                const seq = std.fmt.bufPrint(&color_buf, ";{d}", .{40 + idx}) catch return;
                try buf.appendSlice(seq);
            } else if (idx < 16) {
                const seq = std.fmt.bufPrint(&color_buf, ";{d}", .{100 + idx - 8}) catch return;
                try buf.appendSlice(seq);
            } else {
                const seq = std.fmt.bufPrint(&color_buf, ";48;5;{d}", .{idx}) catch return;
                try buf.appendSlice(seq);
            }
        }

        try buf.append('m');
    }
};

fn colorsEqual(a: c.VTermColor, b: c.VTermColor) bool {
    if (c.VTERM_COLOR_IS_RGB(&a) and c.VTERM_COLOR_IS_RGB(&b)) {
        return a.rgb.red == b.rgb.red and
            a.rgb.green == b.rgb.green and
            a.rgb.blue == b.rgb.blue;
    }
    if (c.VTERM_COLOR_IS_INDEXED(&a) and c.VTERM_COLOR_IS_INDEXED(&b)) {
        return a.indexed.idx == b.indexed.idx;
    }
    return false;
}

fn attrsEqual(a: c.VTermScreenCellAttrs, b: c.VTermScreenCellAttrs) bool {
    return a.bold == b.bold and
        a.italic == b.italic and
        a.underline == b.underline and
        a.blink == b.blink and
        a.reverse == b.reverse and
        a.strike == b.strike;
}
