/// Terminal backend module exports.
/// This module provides the default terminal backend selection based on build configuration.

const build_options = @import("build_options");
const terminal = @import("../terminal.zig");

pub const ghostty = switch (build_options.backend) {
    .ghostty => @import("ghostty.zig"),
    .libvterm => struct {
        pub const GhosttyBackend = @import("libvterm.zig").LibvtermBackend;
    },
};

pub const libvterm = switch (build_options.backend) {
    .libvterm => @import("libvterm.zig"),
    .ghostty => struct {
        pub const LibvtermBackend = @import("ghostty.zig").GhosttyBackend;
    },
};

/// The default terminal implementation based on build configuration
pub const DefaultTerminal = switch (build_options.backend) {
    .ghostty => terminal.Terminal(ghostty.GhosttyBackend),
    .libvterm => terminal.Terminal(libvterm.LibvtermBackend),
};
