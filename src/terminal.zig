const std = @import("std");

/// Terminal backend abstraction layer.
/// This module provides a unified interface for terminal emulators,
/// allowing different backends (ghostty-vt, libvterm, etc.) to be used interchangeably.

/// Cursor position and state
pub const Cursor = struct {
    x: usize,
    y: usize,
    pending_wrap: bool,
};

/// Output format for terminal serialization
pub const Format = enum(u8) {
    plain = 0,
    vt = 1,
    html = 2,
};

/// VT stream for processing terminal input data.
/// Generic over the underlying implementation type.
pub fn VtStream(comptime Impl: type) type {
    return struct {
        impl: Impl,

        const Self = @This();

        /// Process the next slice of input data through the terminal emulator
        pub fn nextSlice(self: *Self, data: []const u8) !void {
            return self.impl.nextSlice(data);
        }

        /// Clean up any resources held by the stream
        pub fn deinit(self: *Self) void {
            self.impl.deinit();
        }
    };
}

/// Terminal interface using comptime polymorphism.
/// Generic over the underlying implementation type.
pub fn Terminal(comptime Impl: type) type {
    return struct {
        impl: Impl,
        alloc: std.mem.Allocator,

        const Self = @This();
        pub const Stream = VtStream(Impl.StreamImpl);

        /// Initialize a new terminal with the given dimensions
        pub fn init(alloc: std.mem.Allocator, cols: u16, rows: u16, max_scrollback: usize) !Self {
            const impl = try Impl.init(alloc, cols, rows, max_scrollback);
            return .{
                .impl = impl,
                .alloc = alloc,
            };
        }

        /// Clean up terminal resources
        pub fn deinit(self: *Self) void {
            self.impl.deinit(self.alloc);
        }

        /// Resize the terminal to new dimensions
        pub fn resize(self: *Self, cols: u16, rows: u16) !void {
            return self.impl.resize(self.alloc, cols, rows);
        }

        /// Get the current cursor position and state
        pub fn getCursor(self: *Self) Cursor {
            return self.impl.getCursor();
        }

        /// Get a VT stream for processing input data
        pub fn vtStream(self: *Self) Stream {
            return .{ .impl = self.impl.vtStream() };
        }

        /// Serialize terminal state for session restoration (VT format with modes/screen).
        /// Returns null if the terminal has no content to serialize.
        pub fn serializeState(self: *Self) error{OutOfMemory}!?[]const u8 {
            return self.impl.serializeState(self.alloc);
        }

        /// Serialize terminal content in the specified format.
        /// Returns null if the format is unsupported or terminal has no content.
        pub fn serialize(self: *Self, format: Format) error{OutOfMemory}!?[]const u8 {
            return self.impl.serialize(self.alloc, format);
        }
    };
}

// Re-export the default backend
pub const backends = @import("backends/mod.zig");
pub const DefaultTerminal = backends.DefaultTerminal;
