const std = @import("std");
const config = @import("config.zig");
const xml = @import("xml.zig");
const files = @import("files.zig");
const policy = @import("policy.zig");
const A = std.mem.Allocator;
const eq = std.mem.eql;
pub fn glob(pattern: []const u8, text: []const u8) bool {
    return expandedGlob(pattern, text, 0);
}
fn expandedGlob(pattern: []const u8, text: []const u8, depth: usize) bool {
    if (depth > 16 or pattern.len > 4096) return false;
    if (std.mem.indexOfScalar(u8, pattern, '{')) |start| {
        var nesting: usize = 1;
        var end = start + 1;
        while (end < pattern.len) : (end += 1) {
            if (pattern[end] == '{') nesting += 1;
            if (pattern[end] == '}') nesting -= 1;
            if (nesting == 0) break;
        }
        if (end == pattern.len) return false;
        var item = start + 1;
        var index = item;
        nesting = 0;
        while (index <= end) : (index += 1) {
            if (index < end and pattern[index] == '{') nesting += 1;
            if (index < end and pattern[index] == '}') nesting -= 1;
            if (index == end or (pattern[index] == ',' and nesting == 0)) {
                var buffer: [4096]u8 = undefined;
                const expanded = std.fmt.bufPrint(&buffer, "{s}{s}{s}", .{ pattern[0..start], pattern[item..index], pattern[end + 1 ..] }) catch return false;
                if (expandedGlob(expanded, text, depth + 1)) return true;
                item = index + 1;
            }
        }
        return false;
    }
    // globset's recursive directory wildcard also matches zero directories.
    if (std.mem.indexOf(u8, pattern, "**/")) |start| {
        if (start == 0 or pattern[start - 1] == '/') {
            var buffer: [4096]u8 = undefined;
            const zero = std.fmt.bufPrint(&buffer, "{s}{s}", .{ pattern[0..start], pattern[start + 3 ..] }) catch return false;
            if (expandedGlob(zero, text, depth + 1)) return true;
        }
    }
    return simpleGlob(pattern, text);
}
fn simpleGlob(pattern: []const u8, text: []const u8) bool {
    var p: usize = 0;
    var t: usize = 0;
    var star: ?usize = null;
    var retry: usize = 0;
    while (t < text.len) {
        if (p < pattern.len and pattern[p] == '*') {
            star = p;
            p += 1;
            retry = t;
            continue;
        }
        if (p < pattern.len) {
            if (pattern[p] == '?' or pattern[p] == text[t]) {
                p += 1;
                t += 1;
                continue;
            }
            if (pattern[p] == '\\' and p + 1 < pattern.len and pattern[p + 1] == text[t]) {
                p += 2;
                t += 1;
                continue;
            }
            if (pattern[p] == '[') {
                if (std.mem.indexOfScalarPos(u8, pattern, p + 1, ']')) |end| {
                    var idx = p + 1;
                    const negate = idx < end and (pattern[idx] == '!' or pattern[idx] == '^');
                    if (negate) idx += 1;
                    var matches = false;
                    while (idx < end) {
                        if (idx + 2 < end and pattern[idx + 1] == '-') {
                            if (text[t] >= pattern[idx] and text[t] <= pattern[idx + 2]) matches = true;
                            idx += 3;
                        } else {
                            if (pattern[idx] == text[t]) matches = true;
                            idx += 1;
                        }
                    }
                    if (matches != negate) {
                        p = end + 1;
                        t += 1;
                        continue;
                    }
                }
            }
        }
        if (star) |s| {
            retry += 1;
            t = retry;
            p = s + 1;
            continue;
        }
        return false;
    }
    while (p < pattern.len and pattern[p] == '*') p += 1;
    return p == pattern.len;
}
pub fn any(patterns: []const []const u8, text: []const u8) bool {
    for (patterns) |p| if (glob(p, text)) return true;
    return false;
}
pub fn safeRelative(path: []const u8) bool {
    if (path.len == 0 or std.fs.path.isAbsolute(path) or std.mem.indexOfScalar(u8, path, ':') != null or std.mem.indexOfScalar(u8, path, '\\') != null) return false;
    var it = std.mem.splitScalar(u8, path, '/');
    while (it.next()) |part| if (part.len == 0 or eq(u8, part, ".") or eq(u8, part, "..") or eq(u8, part, ".git")) return false;
    return true;
}
pub const Ide = struct {
    name: []const u8,
    product: []const u8,
    path: []const u8,
    launched: bool,
    metadata: Metadata = .{},
    pub fn selected(self: Ide, only: []const []const u8) bool {
        if (only.len == 0) return true;
        for (only) |s| if (glob(s, self.name) or glob(s, self.product) or eq(u8, s, self.path)) return true;
        return false;
    }
};
pub const Metadata = struct {
    name: []const u8 = "",
    build: []const u8 = "",
    launcher: []const u8 = "",
    vm_options: []const u8 = "idea.vmoptions",
    modules: []const []const u8 = &.{},
    bundled_plugins: []const []const u8 = &.{},
};
pub fn product(name: []const u8) []const u8 {
    var end: usize = 0;
    while (end < name.len and std.ascii.isAlphabetic(name[end])) end += 1;
    return if (end == 0) name else name[0..end];
}
pub fn discover(fs: files.Fs, root: []const u8, patterns: []const []const u8, installations: []const []const u8) ![]Ide {
    const dir = std.Io.Dir.cwd().openDir(fs.io, root, .{ .iterate = true }) catch |err| switch (err) {
        error.FileNotFound => return &.{},
        else => return err,
    };
    defer dir.close(fs.io);
    var metadata: std.StringHashMapUnmanaged(Metadata) = .empty;
    for (installations) |installation| {
        const candidates = if (std.mem.endsWith(u8, installation, "product-info.json")) &[_][]const u8{""} else try fs.list(installation);
        for (candidates) |candidate| {
            if (candidate.len != 0 and !std.mem.endsWith(u8, candidate, "product-info.json")) continue;
            const path = if (candidate.len == 0) installation else try fs.join(&.{ installation, candidate });
            const raw = try fs.read(path) orelse continue;
            const parsed = try std.json.parseFromSlice(std.json.Value, fs.allocator, raw, .{ .allocate = .alloc_always });
            const info = parsed.value;
            if (info != .object) return error.InvalidProductInfo;
            const name = jsonString(info, "dataDirectoryName") orelse continue;
            var m: Metadata = .{ .name = jsonString(info, "name") orelse name, .build = jsonString(info, "buildNumber") orelse "" };
            m.modules = try jsonStrings(fs.allocator, info, "modules");
            m.bundled_plugins = try jsonStrings(fs.allocator, info, "bundledPlugins");
            if (info.object.get("launch")) |launches| {
                if (launches == .array) for (launches.array.items) |launch| {
                    const tag = jsonString(launch, "os") orelse osTag();
                    if (!eq(u8, tag, osTag())) continue;
                    if (jsonString(launch, "launcherPath")) |s| m.launcher = try fs.join(&.{ std.fs.path.dirname(path) orelse ".", s });
                    if (jsonString(launch, "vmOptionsFilePath")) |s| m.vm_options = std.fs.path.basename(s);
                    break;
                };
            }
            if (metadata.get(name)) |previous| {
                if (!newer(m.build, previous.build)) continue;
            }
            try metadata.put(fs.allocator, name, m);
        }
    }
    var result: std.ArrayList(Ide) = .empty;
    var it = dir.iterate();
    while (try it.next(fs.io)) |entry| {
        if (entry.kind != .directory or !any(patterns, entry.name)) continue;
        const name = try fs.allocator.dupe(u8, entry.name);
        const path = try fs.join(&.{ root, name });
        var launched = false;
        for ([_][]const u8{ "options/other.xml", "options/ide.general.xml", "options/options.xml" }) |marker| if (try fs.exists(try fs.join(&.{ path, marker }))) {
            launched = true;
        };
        try result.append(fs.allocator, .{ .name = name, .product = product(name), .path = path, .launched = launched, .metadata = metadata.get(name) orelse .{} });
    }
    sortIdes(result.items, patterns);
    return result.toOwnedSlice(fs.allocator);
}
pub fn sortIdes(ides: []Ide, patterns: []const []const u8) void {
    std.mem.sort(Ide, ides, patterns, struct {
        fn less(p: []const []const u8, left: Ide, right: Ide) bool {
            var li: usize = p.len;
            var ri: usize = p.len;
            for (p, 0..) |pattern, i| {
                if (li == p.len and glob(pattern, left.name)) li = i;
                if (ri == p.len and glob(pattern, right.name)) ri = i;
            }
            if (li != ri) return li < ri;
            if (newer(left.name, right.name)) return true;
            if (newer(right.name, left.name)) return false;
            return std.mem.lessThan(u8, left.name, right.name);
        }
    }.less);
}
pub fn osTag() []const u8 {
    return "macOS";
}
pub fn jsonString(value: std.json.Value, key: []const u8) ?[]const u8 {
    if (value != .object) return null;
    const v = value.object.get(key) orelse return null;
    return if (v == .string) v.string else null;
}
pub fn jsonStrings(a: A, value: std.json.Value, key: []const u8) ![]const []const u8 {
    if (value != .object) return &.{};
    const v = value.object.get(key) orelse return &.{};
    if (v != .array) return error.InvalidProductInfo;
    const out = try a.alloc([]const u8, v.array.items.len);
    for (v.array.items, out) |item, *s| {
        if (item != .string) return error.InvalidProductInfo;
        s.* = item.string;
    }
    return out;
}
/// Numeric runs compare build numbers without lexicographic 9 > 10 mistakes.
pub fn newer(left: []const u8, right: []const u8) bool {
    var li: usize = 0;
    var ri: usize = 0;
    while (true) {
        while (li < left.len and !std.ascii.isDigit(left[li])) li += 1;
        while (ri < right.len and !std.ascii.isDigit(right[ri])) ri += 1;
        if (li == left.len or ri == right.len) return li != left.len;
        var lv: u64 = 0;
        var rv: u64 = 0;
        while (li < left.len and std.ascii.isDigit(left[li])) : (li += 1) lv = lv *| 10 +| @as(u64, left[li] - '0');
        while (ri < right.len and std.ascii.isDigit(right[ri])) : (ri += 1) rv = rv *| 10 +| @as(u64, right[ri] - '0');
        if (lv != rv) return lv > rv;
    }
}
pub fn vmName(ide: Ide, c: config.Sync) ![]const u8 {
    return config.string(c.vm_names, ide.product, ide.metadata.vm_options);
}
pub fn targetPath(ide: Ide, c: config.Sync, relative: []const u8) ![]const u8 {
    return if (eq(u8, relative, "idea.vmoptions")) vmName(ide, c) else relative;
}
pub fn excluded(c: config.Sync, relative: []const u8) bool {
    if (!safeRelative(relative)) return true;
    if (any(c.explicit, relative)) return false;
    return any(c.exclude, relative) or (c.default_excludes and any(&policy.excludes, relative));
}
pub fn roamable(c: config.Sync, manifest: []const []const u8, relative: []const u8) bool {
    return !excluded(c, relative) and (any(c.explicit, relative) or any(&policy.manifest, relative) or any(manifest, relative) or any(c.include, relative));
}
pub const Pruned = struct { content: ?[]const u8, removed: usize = 0 };
pub fn view(a: A, relative: []const u8, raw: ?[]const u8, c: config.Sync, defaults: config.Value) !Pruned {
    const bytes = raw orelse return .{ .content = null };
    if (eq(u8, std.mem.trim(u8, bytes, " \r\n\t"), "DELETED")) return .{ .content = null };
    const n = xml.parse(a, bytes) catch |err| switch (err) {
        error.OutOfMemory => return err,
        else => return .{ .content = bytes },
    };
    var removed: usize = 0;
    prune(relative, n, null, c, &removed);
    if (c.use_defaults) {
        const d: config.Value = defaults.get("files") orelse .{ .table = .empty };
        if (d.get(relative)) |values| defaults_block: {
            const projection = xml.project(a, n) catch |err| switch (err) {
                // Legacy Rust defaults may contain collapsed duplicate keys.
                // Do not suppress values that cannot be addressed uniquely.
                error.AmbiguousXmlAddress => break :defaults_block,
                else => return err,
            };
            var it = projection.iterator();
            while (it.next()) |entry| if (values.get(entry.key_ptr.*)) |v| {
                if (eq(u8, try v.str(), entry.value_ptr.*)) {
                    try xml.remove(a, n, entry.key_ptr.*);
                    removed += 1;
                }
            };
        }
    }
    xml.pruneEmpty(n);
    return .{ .content = if (n.children.items.len == 0 and n.attrs.count() == 0 and n.text == null) null else try xml.serialize(a, n), .removed = removed };
}
fn attr(n: *const xml.Node, k: []const u8, v: []const u8) bool {
    return if (n.attrs.get(k)) |s| eq(u8, s, v) else false;
}
fn prune(relative: []const u8, n: *xml.Node, inherited_scope: ?[]const u8, c: config.Sync, removed: *usize) void {
    const scope = if (eq(u8, n.name, "component")) n.attrs.get("name") else inherited_scope;
    var i: usize = 0;
    while (i < n.children.items.len) {
        const child = n.children.items[i];
        var drop = false;
        if (eq(u8, relative, "options/project.default.xml") and eq(u8, child.name, "component")) {
            for ([_][]const u8{ "WindowStateProjectService", "masterDetails", "ProjectInspectionProfilesVisibleTreeState", "PropertiesComponent" }) |name| if (attr(child, "name", name)) {
                drop = true;
            };
        }
        if (c.use_defaults) {
            if (eq(u8, relative, "options/ide.general.xml") and files.equal(scope, "Registry") and eq(u8, child.name, "entry") and (attr(child, "source", "SYSTEM") or attr(child, "source", "MANAGER"))) drop = true;
            if (eq(u8, relative, "options/ide-features-trainer.xml") and files.equal(scope, "LessonStateBase") and eq(u8, child.name, "entry") and attr(child, "value", "NOT_PASSED")) drop = true;
            if (glob("options/*.xml", relative) and eq(u8, child.name, "option") and attr(child, "name", "MIGRATE_OLD_SETTINGS")) drop = true;
            if (eq(u8, relative, "options/other.xml") and files.equal(scope, "LangManager") and eq(u8, child.name, "entry") and attr(child, "key", "JAVA")) drop = true;
        }
        for (c.rules) |rule| {
            if (!glob(rule.file, relative) or !eq(u8, rule.element, child.name)) continue;
            if (rule.component != null and !files.equal(rule.component, scope)) continue;
            if (rule.option) |option| {
                if (!attr(child, "name", option)) continue;
            }
            if (rule.attribute) |attribute| {
                if (!attr(child, attribute, rule.equals)) continue;
            }
            drop = true;
        }
        if (drop) {
            _ = n.children.orderedRemove(i);
            removed.* += 1;
        } else {
            prune(relative, child, scope, c, removed);
            i += 1;
        }
    }
}
test "pruning preserves USER choices; privacy rules survive use_defaults=false" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const v = try view(a, "options/ide.general.xml", "<application><component name='Registry'><entry key='a' value='1' source='SYSTEM'/><entry key='b' value='2' source='USER'/></component></application>", .{}, .{ .table = .empty });
    try std.testing.expectEqual(@as(usize, 1), v.removed);
    try std.testing.expect(std.mem.indexOf(u8, v.content.?, "USER") != null);
    const p = try view(a, "options/project.default.xml", "<application><component name='ProjectManager'><defaultProject><component name='PropertiesComponent'>secret</component></defaultProject></component></application>", .{ .use_defaults = false }, .{ .table = .empty });
    try std.testing.expect(p.content == null);
    try std.testing.expect(!safeRelative("../secret"));
    try std.testing.expect(glob("*20??.*", "CLion2026.2"));
    try std.testing.expect(glob("options/[a-z]*.xml", "options/editor.xml"));
}

test "numeric build order and brace globs" {
    try std.testing.expect(newer("262.10", "262.9"));
    try std.testing.expect(!newer("262.9", "262.10"));
    try std.testing.expect(glob("options/{editor,laf}.xml", "options/laf.xml"));
    try std.testing.expect(glob("{CLion,PyCharm}20??.*", "CLion2026.2"));
}

test "legacy collapsed defaults cannot remove ambiguous XML values" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const defaults = try config.parse(a, "[files.\"options/ambiguous.xml\"]\n\"component[name=Injection]/injection[language=RegExp]/@value\"='one'\n");
    const raw = "<application><component name='Injection'><injection language='RegExp' value='one'/><injection language='RegExp' value='two'/></component></application>";
    const pruned = try view(a, "options/ambiguous.xml", raw, .{}, defaults);
    try std.testing.expectEqual(@as(usize, 0), pruned.removed);
    const root = try xml.parse(a, pruned.content.?);
    try std.testing.expectEqual(@as(usize, 2), root.children.items[0].children.items.len);
    try std.testing.expectEqualStrings("one", root.children.items[0].children.items[0].attrs.get("value").?);
    try std.testing.expectEqualStrings("two", root.children.items[0].children.items[1].attrs.get("value").?);
}

test "missing settings root returns before scanning unrelated applications" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var temp = std.testing.tmpDir(.{});
    defer temp.cleanup();
    const root = try temp.dir.realPathFileAlloc(std.testing.io, ".", a);
    const fs: files.Fs = .{ .allocator = a, .io = std.testing.io };
    const metadata = try fs.join(&.{ root, "product-info.json" });
    try fs.write(metadata, "not valid JSON");
    const missing = try fs.join(&.{ root, "missing-JetBrains" });
    try std.testing.expectEqual(@as(usize, 0), (try discover(fs, missing, &.{"*"}, &.{metadata})).len);
}
