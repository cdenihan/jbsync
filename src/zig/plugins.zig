//! Plugin descriptors and manifests, using std.zip for JARs. Installation
//! actions are returned to the engine and run only after conflict validation.
const std = @import("std");
const files = @import("files.zig");
const settings = @import("settings.zig");
const config = @import("config.zig");
const xml = @import("xml.zig");
const A = std.mem.Allocator;
const eq = std.mem.eql;
pub const Plugin = struct {
    id: []const u8,
    name: []const u8 = "",
    version: []const u8 = "",
    since_build: []const u8 = "",
    until_build: []const u8 = "",
    required_dependencies: []const []const u8 = &.{},
    incompatible_with: []const []const u8 = &.{},
    provided_modules: []const []const u8 = &.{},
    modular: bool = false,
    source_products: []const []const u8 = &.{},
};
pub const Manifest = struct { version: u32 = 1, plugins: []const Plugin = &.{} };
pub const Action = struct { ide: settings.Ide, id: []const u8 };
fn childText(n: *const xml.Node, name: []const u8) []const u8 {
    for (n.children.items) |child| if (eq(u8, child.name, name)) return child.text orelse "";
    return "";
}
pub fn descriptor(a: A, raw: []const u8, fallback: []const u8) !Plugin {
    const n = try xml.parse(a, raw);
    if (!eq(u8, n.name, "idea-plugin")) return error.InvalidPluginDescriptor;
    const name = childText(n, "name");
    const id = childText(n, "id");
    var p: Plugin = .{ .id = if (id.len != 0) id else if (name.len != 0) name else fallback, .name = name, .version = childText(n, "version") };
    var required: std.ArrayList([]const u8) = .empty;
    var provided: std.ArrayList([]const u8) = .empty;
    var incompatible: std.ArrayList([]const u8) = .empty;
    for (n.children.items) |child| {
        if (eq(u8, child.name, "idea-version")) {
            p.since_build = child.attrs.get("since-build") orelse "";
            p.until_build = child.attrs.get("until-build") orelse "";
        }
        if (eq(u8, child.name, "depends") and !files.equal(child.attrs.get("optional"), "true")) if (child.text) |s| try required.append(a, s);
        if (eq(u8, child.name, "incompatible-with")) if (child.text) |s| try incompatible.append(a, s);
        if (eq(u8, child.name, "module")) if (child.attrs.get("value")) |s| try provided.append(a, s);
        if (eq(u8, child.name, "content")) {
            p.modular = true;
            for (child.children.items) |module| if (module.attrs.get("name")) |s| try provided.append(a, s);
        }
        if (eq(u8, child.name, "dependencies")) {
            for (child.children.items) |dependency| {
                if (files.equal(dependency.attrs.get("optional"), "true") or files.equal(dependency.attrs.get("loading"), "optional")) continue;
                if (dependency.attrs.get("name") orelse dependency.attrs.get("id")) |s| try required.append(a, s);
            }
        }
    }
    p.required_dependencies = required.items;
    p.provided_modules = provided.items;
    p.incompatible_with = incompatible.items;
    return p;
}
fn fromJar(fs: files.Fs, path: []const u8) !?[]const u8 {
    const f = try std.Io.Dir.cwd().openFile(fs.io, path, .{});
    defer f.close(fs.io);
    var buffer: [4096]u8 = undefined;
    var reader = f.reader(fs.io, &buffer);
    var it = try std.zip.Iterator.init(&reader);
    var name_buffer: [4096]u8 = undefined;
    while (try it.next()) |entry| {
        const name = try entry.getFilename(&reader, &name_buffer, .{});
        if (!eq(u8, name, "META-INF/plugin.xml")) continue;
        const output = try fs.allocator.alloc(u8, 1024 * 1024);
        var writer = std.Io.Writer.fixed(output);
        try entry.extractTo(&reader, &writer);
        return writer.buffered();
    }
    return null;
}
pub fn installed(fs: files.Fs, ide: settings.Ide) ![]Plugin {
    var result: std.ArrayList(Plugin) = .empty;
    const root = try fs.join(&.{ ide.path, "plugins" });
    const dir = std.Io.Dir.cwd().openDir(fs.io, root, .{ .iterate = true }) catch |err| switch (err) {
        error.FileNotFound => return &.{},
        else => return err,
    };
    defer dir.close(fs.io);
    var it = dir.iterate();
    const bundled_raw = (try fs.read(try fs.join(&.{ ide.path, "bundled_plugins.txt" }))) orelse "";
    while (try it.next(fs.io)) |entry| {
        if (entry.kind != .directory) continue;
        const fallback = try fs.allocator.dupe(u8, entry.name);
        const plugin_dir = try fs.join(&.{ root, fallback });
        var raw = try fs.read(try fs.join(&.{ plugin_dir, "META-INF/plugin.xml" }));
        if (raw == null) {
            for (try fs.list(plugin_dir)) |relative| {
                if (!std.mem.endsWith(u8, relative, ".jar")) continue;
                raw = try fromJar(fs, try fs.join(&.{ plugin_dir, relative }));
                if (raw != null) break;
            }
        }
        const source = raw orelse continue;
        var p = try descriptor(fs.allocator, source, fallback);
        var bundled = has(ide.metadata.bundled_plugins, p.id);
        var lines = std.mem.splitScalar(u8, bundled_raw, '\n');
        while (lines.next()) |line| {
            const pipe = std.mem.indexOfScalar(u8, line, '|') orelse line.len;
            if (eq(u8, std.mem.trim(u8, line[0..pipe], " \r\t"), p.id)) bundled = true;
        }
        if (bundled) continue;
        p.source_products = try fs.allocator.dupe([]const u8, &.{ide.product});
        try result.append(fs.allocator, p);
    }
    std.mem.sort(Plugin, result.items, {}, struct {
        fn less(_: void, l: Plugin, r: Plugin) bool {
            return std.mem.lessThan(u8, l.id, r.id);
        }
    }.less);
    return result.items;
}
fn has(items: []const []const u8, value: []const u8) bool {
    for (items) |item| if (eq(u8, item, value)) return true;
    return false;
}
pub fn compatible(a: A, plugin: Plugin, ide: settings.Ide, capabilities: *const std.StringHashMapUnmanaged(void), c: config.Sync) !?[]const u8 {
    var manual: ?bool = null;
    for (try config.array(c.plugin_config, "rule")) |rule| {
        if (!settings.glob(try config.string(rule, "id", "*"), plugin.id)) continue;
        const pattern = try config.string(rule, "ide", "*");
        const matches = settings.glob(pattern, ide.name) or settings.glob(pattern, ide.product);
        const action = try config.string(rule, "action", "");
        if (eq(u8, action, "only")) manual = matches else if (matches and eq(u8, action, "allow")) manual = true else if (matches and eq(u8, action, "deny")) manual = false;
    }
    if (manual) |allow| return if (allow) null else "manual plugin rule";
    if (ide.metadata.build.len == 0 and !has(plugin.source_products, ide.product)) return "product build metadata unavailable";
    if (plugin.since_build.len != 0 and settings.newer(plugin.since_build, ide.metadata.build)) return "IDE older than since-build";
    if (plugin.until_build.len != 0) {
        if (std.mem.endsWith(u8, plugin.until_build, ".*")) {
            const prefix = plugin.until_build[0 .. plugin.until_build.len - 1];
            if (!std.mem.startsWith(u8, ide.metadata.build, prefix)) return "IDE outside until-build range";
        } else if (settings.newer(ide.metadata.build, plugin.until_build)) return "IDE newer than until-build";
    }
    for (plugin.incompatible_with) |id| if (capabilities.contains(id)) return try std.fmt.allocPrint(a, "incompatible with {s}", .{id});
    for (plugin.required_dependencies) |id| if (!capabilities.contains(id)) return try std.fmt.allocPrint(a, "missing dependency {s}", .{id});
    return null;
}
pub fn reconcile(fs: files.Fs, staging: *files.Staging, store: []const u8, ides: []const settings.Ide, c: config.Sync, options: @import("engine.zig").Options, out: *std.Io.Writer) ![]Action {
    const a = fs.allocator;
    const path = try fs.join(&.{ store, "plugins.json" });
    const raw = try staging.read(path);
    const stored = if (raw) |s| (try std.json.parseFromSlice(Manifest, a, s, .{ .ignore_unknown_fields = true, .allocate = .alloc_always })).value else Manifest{};
    if (stored.version != 1) return error.UnsupportedPluginManifest;
    var plugins: std.StringHashMapUnmanaged(Plugin) = .empty;
    for (stored.plugins) |p| try plugins.put(a, p.id, p);
    var observations: std.StringHashMapUnmanaged([]Plugin) = .empty;
    for (ides) |ide| {
        if (!ide.launched or !ide.selected(options.only)) continue;
        const found = try installed(fs, ide);
        try observations.put(a, ide.name, found);
        for (found) |p| {
            var combined = p;
            if (plugins.get(p.id)) |prior| {
                var products: std.ArrayList([]const u8) = .empty;
                try products.appendSlice(a, prior.source_products);
                for (p.source_products) |product| if (!has(products.items, product)) try products.append(a, product);
                combined.source_products = products.items;
            }
            try plugins.put(a, p.id, combined);
        }
    }
    var ordered = try a.alloc(Plugin, plugins.count());
    var it = plugins.valueIterator();
    var i: usize = 0;
    while (it.next()) |p| {
        ordered[i] = p.*;
        i += 1;
    }
    std.mem.sort(Plugin, ordered, {}, struct {
        fn less(_: void, l: Plugin, r: Plugin) bool {
            return std.mem.lessThan(u8, l.id, r.id);
        }
    }.less);
    if (ordered.len != 0 or raw != null) {
        var encoded: std.Io.Writer.Allocating = .init(a);
        try std.json.Stringify.value(Manifest{ .plugins = ordered }, .{ .whitespace = .indent_2 }, &encoded.writer);
        try encoded.writer.writeByte('\n');
        try staging.write(path, try encoded.toOwnedSlice());
    }
    var actions: std.ArrayList(Action) = .empty;
    for (ides) |ide| {
        const observed = observations.get(ide.name) orelse continue;
        var caps: std.StringHashMapUnmanaged(void) = .empty;
        for (ide.metadata.modules) |s| try caps.put(a, s, {});
        for (ide.metadata.bundled_plugins) |s| try caps.put(a, s, {});
        for (observed) |p| {
            try caps.put(a, p.id, {});
            for (p.provided_modules) |s| try caps.put(a, s, {});
        }
        for (try config.array(c.plugin_config, "capability")) |rule| {
            const pattern = try config.string(rule, "ide", "*");
            if (!settings.glob(pattern, ide.name) and !settings.glob(pattern, ide.product)) continue;
            for (try config.strings(a, rule, "add", &.{})) |s| try caps.put(a, s, {});
            for (try config.strings(a, rule, "remove", &.{})) |s| _ = caps.remove(s);
        }
        const planned = try a.alloc(bool, ordered.len);
        @memset(planned, false);
        // Resolve managed dependencies before dependents. Capabilities grow
        // only from already installed or compatible planned plugins.
        for (0..ordered.len) |_| {
            var progressed = false;
            for (ordered, 0..) |p, index| {
                if (planned[index] or caps.contains(p.id)) continue;
                if (try compatible(a, p, ide, &caps, c) != null) continue;
                planned[index] = true;
                progressed = true;
                try out.print("{s}: install {s}\n", .{ ide.name, p.id });
                try actions.append(a, .{ .ide = ide, .id = p.id });
                try caps.put(a, p.id, {});
                for (p.provided_modules) |module| try caps.put(a, module, {});
            }
            if (!progressed) break;
        }
        for (ordered, 0..) |p, index| {
            if (planned[index] or caps.contains(p.id)) continue;
            if (try compatible(a, p, ide, &caps, c)) |reason| try out.print("{s}: skip {s} ({s})\n", .{ ide.name, p.id, reason });
        }
        for (observed) |p| if (try compatible(a, p, ide, &caps, c)) |reason| try out.print("{s}: installed {s} cannot load ({s})\n", .{ ide.name, p.id, reason });
    }
    return actions.items;
}
pub fn install(fs: files.Fs, actions: []const Action, c: config.Sync) !void {
    const launchers = try config.table(c.plugin_config, "launchers");
    for (actions) |action| {
        const launcher = try config.string(launchers, action.ide.product, action.ide.metadata.launcher);
        if (launcher.len == 0) return error.PluginLauncherNotFound;
        const result = try std.process.run(fs.allocator, fs.io, .{ .argv = &.{ launcher, "installPlugins", action.id }, .stdout_limit = .limited(1024 * 1024), .stderr_limit = .limited(1024 * 1024) });
        if (result.term != .exited or result.term.exited != 0) {
            std.Io.File.stderr().writeStreamingAll(fs.io, result.stderr) catch {};
            return error.PluginInstallationFailed;
        }
    }
}
test "descriptor dependencies and build ranges" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const p = try descriptor(a, "<idea-plugin><id>x</id><version>1</version><idea-version since-build='262.1' until-build='262.*'/><depends>python</depends><depends optional='true'>optional</depends><module value='provided'/></idea-plugin>", "fallback");
    try std.testing.expectEqualStrings("x", p.id);
    try std.testing.expectEqual(@as(usize, 1), p.required_dependencies.len);
    var caps: std.StringHashMapUnmanaged(void) = .empty;
    try caps.put(a, "python", {});
    const ide: settings.Ide = .{ .name = "PyCharm2026.2", .product = "PyCharm", .path = "", .launched = true, .metadata = .{ .build = "262.3" } };
    try std.testing.expect(try compatible(a, p, ide, &caps, .{}) == null);
    _ = caps.remove("python");
    try std.testing.expect(try compatible(a, p, ide, &caps, .{}) != null);
}
