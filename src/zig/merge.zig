//! Pure three-way reconciliation; never performs I/O.
const std = @import("std");
const xml = @import("xml.zig");
const files = @import("files.zig");
const A = std.mem.Allocator;
pub const Policy = enum { local, remote, neither };
pub const Change = struct { path: []const u8, before: ?[]const u8, after: ?[]const u8 };
pub const Conflict = struct { path: []const u8, local: ?[]const u8, remote: ?[]const u8 };
pub const Result = struct {
    content: ?[]const u8,
    incoming: std.ArrayList(Change) = .empty,
    outgoing: std.ArrayList(Change) = .empty,
    conflicts: std.ArrayList(Conflict) = .empty,
    pub fn changed(self: Result) bool {
        return self.incoming.items.len + self.outgoing.items.len + self.conflicts.items.len != 0;
    }
};
fn normalize(s: ?[]const u8) ?[]const u8 {
    if (s) |text| if (std.mem.eql(u8, std.mem.trim(u8, text, " \r\n\t"), "DELETED")) return null;
    return s;
}
pub fn whole(a: A, base: ?[]const u8, local: ?[]const u8, remote: ?[]const u8, policy: Policy) !Result {
    var r: Result = .{ .content = local };
    if (files.equal(local, remote)) return r;
    const take_remote = if (files.equal(local, base)) true else if (files.equal(remote, base)) false else blk: {
        try r.conflicts.append(a, .{ .path = "(whole file)", .local = local, .remote = remote });
        break :blk policy == .remote;
    };
    r.content = if (take_remote) remote else local;
    const change: Change = .{ .path = "", .before = if (take_remote) local else remote, .after = r.content };
    if (take_remote) try r.incoming.append(a, change) else try r.outgoing.append(a, change);
    return r;
}
pub fn file(a: A, base_raw: ?[]const u8, local_raw: ?[]const u8, remote_raw: ?[]const u8, policy: Policy) !Result {
    const base = normalize(base_raw);
    const local = normalize(local_raw);
    const remote = normalize(remote_raw);
    if (files.equal(local, remote)) return .{ .content = local };
    if (local != null and remote != null) {
        const l = xml.parse(a, local.?) catch |err| switch (err) {
            error.OutOfMemory => return err,
            else => null,
        };
        const r = xml.parse(a, remote.?) catch |err| switch (err) {
            error.OutOfMemory => return err,
            else => null,
        };
        if (l != null and r != null and std.mem.eql(u8, l.?.name, r.?.name)) {
            const b = if (base) |text| xml.parse(a, text) catch |err| switch (err) {
                error.OutOfMemory => return err,
                else => null,
            } else null;
            return documents(a, b, l.?, r.?, policy);
        }
        if (std.unicode.utf8ValidateSlice(local.?) and std.unicode.utf8ValidateSlice(remote.?) and !std.mem.startsWith(u8, std.mem.trim(u8, local.?, " \r\n\t"), "<")) return mergeText(a, base, local.?, remote.?, policy);
    }
    return whole(a, base, local, remote, policy);
}
fn documents(a: A, base: ?*const xml.Node, local: *const xml.Node, remote: *const xml.Node, policy: Policy) !Result {
    const bv = if (base) |b| try xml.project(a, b) else xml.View.empty;
    const lv = try xml.project(a, local);
    const rv = try xml.project(a, remote);
    const merged = try local.clone(a);
    var result: Result = .{ .content = null };
    const paths = try unionKeys(a, &.{ bv, lv, rv });
    // Apply removals in reverse document address order before additions. This
    // avoids renumbering keyless siblings while deleting their leaves.
    var removals: std.ArrayList([]const u8) = .empty;
    var updates: xml.View = .empty;
    for (paths) |path| {
        const b = bv.get(path);
        const l = lv.get(path);
        const r = rv.get(path);
        if (files.equal(l, r)) continue;
        const take_remote = if (files.equal(l, b)) true else if (files.equal(r, b)) false else blk: {
            try result.conflicts.append(a, .{ .path = path, .local = l, .remote = r });
            break :blk policy == .remote;
        };
        const change: Change = .{ .path = path, .before = if (take_remote) l else r, .after = if (take_remote) r else l };
        if (take_remote) {
            try result.incoming.append(a, change);
            if (r) |v| try updates.put(a, path, v) else try removals.append(a, path);
        } else try result.outgoing.append(a, change);
    }
    try xml.patch(a, merged, remote, updates, removals.items);
    xml.pruneEmpty(merged);
    result.content = try xml.serialize(a, merged);
    return result;
}
pub fn unionKeys(a: A, views: []const xml.View) ![][]const u8 {
    var set: std.StringHashMapUnmanaged(void) = .empty;
    for (views) |view| {
        var it = view.keyIterator();
        while (it.next()) |k| try set.put(a, k.*, {});
    }
    const keys = try a.alloc([]const u8, set.count());
    var it = set.keyIterator();
    var i: usize = 0;
    while (it.next()) |k| {
        keys[i] = k.*;
        i += 1;
    }
    std.mem.sort([]const u8, keys, {}, files.less);
    return keys;
}
pub fn flagKey(line: []const u8) []const u8 {
    if (std.mem.indexOfScalar(u8, line, '=')) |i| return line[0..i];
    return std.mem.trimEnd(u8, line, "0123456789kKmMgG");
}
fn lines(a: A, raw: ?[]const u8) !xml.View {
    var result: xml.View = .empty;
    var it = std.mem.splitScalar(u8, raw orelse "", '\n');
    while (it.next()) |line| {
        const trimmed = std.mem.trim(u8, line, " \t\r");
        if (trimmed.len != 0) try result.put(a, flagKey(trimmed), trimmed);
    }
    return result;
}
fn mergeText(a: A, base: ?[]const u8, local: []const u8, remote: []const u8, policy: Policy) !Result {
    const bv = try lines(a, base);
    const lv = try lines(a, local);
    const rv = try lines(a, remote);
    var out: std.Io.Writer.Allocating = .init(a);
    var result: Result = .{ .content = null };
    for (try unionKeys(a, &.{ bv, lv, rv })) |path| {
        const b = bv.get(path);
        const l = lv.get(path);
        const r = rv.get(path);
        var value = l;
        if (!files.equal(l, r)) {
            const take_remote = if (files.equal(l, b)) true else if (files.equal(r, b)) false else blk: {
                try result.conflicts.append(a, .{ .path = path, .local = l, .remote = r });
                break :blk policy == .remote;
            };
            value = if (take_remote) r else l;
            const change: Change = .{ .path = "", .before = if (take_remote) l else r, .after = value };
            if (take_remote) try result.incoming.append(a, change) else try result.outgoing.append(a, change);
        }
        if (value) |v| try out.writer.print("{s}\n", .{v});
    }
    result.content = try out.toOwnedSlice();
    return result;
}
test "disjoint XML edits, removals, conflicts and policies" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const b = "<application><component name='Editor'><option name='tabs' value='4'/><option name='wrap' value='true'/></component></application>";
    const l = "<application><component name='Editor'><option name='tabs' value='8'/><option name='wrap' value='true'/></component></application>";
    const r = "<application><component name='Editor'><option name='tabs' value='4'/><option name='wrap' value='false'/></component></application>";
    const result = try file(a, b, l, r, .local);
    const view = try xml.project(a, try xml.parse(a, result.content.?));
    try std.testing.expectEqual(@as(usize, 0), result.conflicts.items.len);
    try std.testing.expectEqualStrings("8", view.get("component[name=Editor]/option[name=tabs]/@value").?);
    try std.testing.expectEqualStrings("false", view.get("component[name=Editor]/option[name=wrap]/@value").?);
    const conflict = try file(a, b, l, "<application><component name='Editor'><option name='tabs' value='2'/></component></application>", .remote);
    try std.testing.expectEqual(@as(usize, 1), conflict.conflicts.items.len);
    try std.testing.expect((try file(a, b, "DELETED", b, .local)).content == null);
}
test "JVM flag values conflict, independent additions survive" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const merged = try file(a, "-Xmx4g\n", "-Xmx8g\n-XX:+UseZGC\n", "-Xmx16g\n-Dfile.encoding=UTF-8\n", .remote);
    try std.testing.expectEqual(@as(usize, 1), merged.conflicts.items.len);
    try std.testing.expect(std.mem.indexOf(u8, merged.content.?, "-Xmx16g") != null);
    try std.testing.expect(std.mem.indexOf(u8, merged.content.?, "-XX:+UseZGC") != null);
    try std.testing.expect(std.mem.indexOf(u8, merged.content.?, "-Dfile.encoding=UTF-8") != null);
}

test "twelve ordered keyless children are grafted in document order" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const base = "<application><component name='Filters'><list/></component></application>";
    var raw: std.Io.Writer.Allocating = .init(a);
    try raw.writer.writeAll("<application><component name='Filters'><list>");
    for (0..12) |i| try raw.writer.print("<item value='{d}'/>", .{i});
    try raw.writer.writeAll("</list></component></application>");
    const result = try file(a, base, base, raw.written(), .local);
    const n = try xml.parse(a, result.content.?);
    for (n.children.items[0].children.items[0].children.items, 0..) |item, i| {
        try std.testing.expectEqualStrings(try std.fmt.allocPrint(a, "{d}", .{i}), item.attrs.get("value").?);
    }
}
