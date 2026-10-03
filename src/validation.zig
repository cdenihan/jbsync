//! Batch protocol used only by the Rust/Zig differential test suite.
const std = @import("std");
const lib = @import("jbsync");
const Case = struct {
    op: []const u8,
    base: ?[]const u8 = null,
    local: ?[]const u8 = null,
    remote: ?[]const u8 = null,
    policy: lib.merge.Policy = .local,
    pattern: []const u8 = "",
    plugin: ?lib.plugins.Plugin = null,
    product: []const u8 = "",
    build: []const u8 = "",
    capabilities: []const []const u8 = &.{},
};
pub fn main(init: std.process.Init) !void {
    const a = init.arena.allocator();
    const args = try init.minimal.args.toSlice(a);
    const raw = try std.Io.Dir.cwd().readFileAlloc(init.io, args[1], a, .limited(64 * 1024 * 1024));
    const cases = (try std.json.parseFromSlice([]Case, a, raw, .{ .allocate = .alloc_always })).value;
    var buffer: [8192]u8 = undefined;
    var output = std.Io.File.stdout().writer(init.io, &buffer);
    const w = &output.interface;
    try w.writeByte('[');
    for (cases, 0..) |c, i| {
        var arena = std.heap.ArenaAllocator.init(a);
        defer arena.deinit();
        const scratch = arena.allocator();
        if (i != 0) try w.writeByte(',');
        if (std.mem.eql(u8, c.op, "plugin")) {
            var caps: std.StringHashMapUnmanaged(void) = .empty;
            for (c.capabilities) |cap| try caps.put(scratch, cap, {});
            const ide: lib.settings.Ide = .{ .name = "fixture", .product = c.product, .path = "fixture", .launched = true, .metadata = .{ .build = c.build } };
            try std.json.Stringify.value(.{ .compatible = try lib.plugins.compatible(scratch, c.plugin.?, ide, &caps, .{}) == null }, .{}, w);
        } else if (std.mem.eql(u8, c.op, "glob")) {
            try std.json.Stringify.value(.{ .matches = lib.settings.glob(c.pattern, c.local.?) }, .{}, w);
        } else if (std.mem.eql(u8, c.op, "xml")) {
            const node = lib.xml.parse(scratch, c.local.?) catch {
                try w.writeAll("{\"error\":true}");
                continue;
            };
            try std.json.Stringify.value(.{ .content = try lib.xml.serialize(scratch, node) }, .{}, w);
        } else {
            const result = try lib.merge.file(scratch, c.base, c.local, c.remote, c.policy);
            try std.json.Stringify.value(.{ .content = result.content, .conflicts = result.conflicts.items.len }, .{}, w);
        }
    }
    try w.writeAll("]\n");
    try w.flush();
}
