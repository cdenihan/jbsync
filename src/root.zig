//! Experimental Zig implementation. Allocations belong to the caller's arena.
pub const version = @import("build_options").version;
pub const xml = @import("zig/xml.zig");
pub const merge = @import("zig/merge.zig");
pub const config = @import("zig/config.zig");
pub const files = @import("zig/files.zig");
pub const settings = @import("zig/settings.zig");
pub const engine = @import("zig/engine.zig");
pub const cli = @import("zig/cli.zig");
test {
    _ = files;
    _ = engine;
    _ = @import("zig/plugins.zig");
    _ = @import("zig/git.zig");
    _ = xml;
    _ = merge;
    _ = config;
    _ = settings;
    _ = cli;
}
