const std = @import("std");
const Allocator = std.mem.Allocator;
pub const Map = std.StringHashMapUnmanaged(?[]const u8);
pub const max_file_size = 32 * 1024 * 1024;

pub const Fs = struct {
    allocator: Allocator,
    io: std.Io,
    pub fn join(self: Fs, parts: []const []const u8) ![]const u8 {
        return std.fs.path.join(self.allocator, parts);
    }
    pub fn read(self: Fs, path: []const u8) !?[]const u8 {
        const stat = std.Io.Dir.cwd().statFile(self.io, path, .{ .follow_symlinks = false }) catch |err| switch (err) {
            error.FileNotFound => return null,
            else => return err,
        };
        if (stat.kind != .file) return error.NotARegularSettingsFile;
        return std.Io.Dir.cwd().readFileAlloc(self.io, path, self.allocator, .limited(max_file_size)) catch |err| switch (err) {
            error.FileNotFound => null,
            else => return err,
        };
    }
    pub fn exists(self: Fs, path: []const u8) !bool {
        std.Io.Dir.cwd().access(self.io, path, .{}) catch |err| switch (err) {
            error.FileNotFound => return false,
            else => return err,
        };
        return true;
    }
    pub fn mkdir(self: Fs, path: []const u8) !void {
        try std.Io.Dir.cwd().createDirPath(self.io, path);
    }
    /// Write in the destination directory, flush, then replace atomically.
    pub fn write(self: Fs, path: []const u8, content: ?[]const u8) !void {
        const cwd = std.Io.Dir.cwd();
        if (content) |bytes| {
            if (std.fs.path.dirname(path)) |parent| try self.mkdir(parent);
            var random: [8]u8 = undefined;
            self.io.random(&random);
            const temp = try std.fmt.allocPrint(self.allocator, "{s}.{x}.tmp", .{ path, std.mem.readInt(u64, &random, .little) });
            defer cwd.deleteFile(self.io, temp) catch {};
            const previous = cwd.statFile(self.io, path, .{ .follow_symlinks = false }) catch |err| switch (err) {
                error.FileNotFound => null,
                else => return err,
            };
            if (previous) |stat| if (stat.kind != .file) return error.NotARegularSettingsFile;
            {
                const f = try cwd.createFile(self.io, temp, .{ .exclusive = true, .permissions = .fromMode(0o600) });
                defer f.close(self.io);
                try f.writeStreamingAll(self.io, bytes);
                if (previous) |stat| try f.setPermissions(self.io, stat.permissions);
                try f.sync(self.io);
            }
            try cwd.rename(temp, cwd, path, self.io);
        } else {
            cwd.deleteFile(self.io, path) catch |err| switch (err) {
                error.FileNotFound => {},
                else => return err,
            };
        }
    }
    pub fn pruneBackups(self: Fs, root: []const u8, keep: usize) !void {
        const dir = std.Io.Dir.cwd().openDir(self.io, root, .{ .iterate = true }) catch |err| switch (err) {
            error.FileNotFound => return,
            else => return err,
        };
        defer dir.close(self.io);
        var runs: std.ArrayList([]const u8) = .empty;
        var it = dir.iterate();
        while (try it.next(self.io)) |entry| {
            if (entry.kind != .directory or entry.name.len == 0 or !std.ascii.isDigit(entry.name[0])) continue;
            try runs.append(self.allocator, try self.allocator.dupe(u8, entry.name));
        }
        std.mem.sort([]const u8, runs.items, {}, less);
        if (runs.items.len > keep) for (runs.items[0 .. runs.items.len - keep]) |run| try dir.deleteTree(self.io, run);
    }
    /// Check only paths below the trusted root; system aliases such as /var
    /// remain usable. A shared setting must never traverse a local symlink.
    pub fn checkPath(self: Fs, root: []const u8, relative: []const u8) !void {
        if (!@import("settings.zig").safeRelative(relative)) return error.UnsafeStorePath;
        var parts = std.mem.splitScalar(u8, relative, '/');
        var path = root;
        while (parts.next()) |part| {
            path = try self.join(&.{ path, part });
            const stat = std.Io.Dir.cwd().statFile(self.io, path, .{ .follow_symlinks = false }) catch |err| switch (err) {
                error.FileNotFound => return,
                else => return err,
            };
            if (stat.kind == .sym_link) return error.SymlinkInSettingsPath;
        }
    }
    pub fn list(self: Fs, root: []const u8) ![][]const u8 {
        return self.listFiltered(root, false);
    }
    pub fn listFiltered(self: Fs, root: []const u8, ide_settings: bool) ![][]const u8 {
        var found: std.ArrayList([]const u8) = .empty;
        const dir = std.Io.Dir.cwd().openDir(self.io, root, .{ .iterate = true }) catch |err| switch (err) {
            error.FileNotFound => return &.{},
            else => return err,
        };
        defer dir.close(self.io);
        var walker = try dir.walkSelectively(self.allocator);
        defer walker.deinit();
        while (try walker.next(self.io)) |entry| {
            if (internal(entry.path)) continue;
            if (entry.kind == .directory) {
                if (ide_settings and entry.depth() == 1 and ignoredIdeDirectory(entry.basename)) continue;
                try walker.enter(self.io, entry);
                continue;
            }
            if (entry.kind != .file) continue;
            const path = try self.allocator.dupe(u8, entry.path);
            std.mem.replaceScalar(u8, path, '\\', '/');
            try found.append(self.allocator, path);
        }
        std.mem.sort([]const u8, found.items, {}, less);
        return found.toOwnedSlice(self.allocator);
    }
};
fn ignoredIdeDirectory(name: []const u8) bool {
    for ([_][]const u8{ "plugins", "workspace", "system", "log", "event-log-metadata", "settingsSync", "tasks", "extensions" }) |ignored| {
        if (std.mem.eql(u8, name, ignored)) return true;
    }
    return false;
}
pub fn less(_: void, a: []const u8, b: []const u8) bool {
    return std.mem.lessThan(u8, a, b);
}
pub fn internal(path: []const u8) bool {
    var parts = std.mem.tokenizeAny(u8, path, "/\\");
    while (parts.next()) |part| if (std.mem.eql(u8, part, ".git")) return true;
    return false;
}
pub fn equal(a: ?[]const u8, b: ?[]const u8) bool {
    if (a) |av| return if (b) |bv| std.mem.eql(u8, av, bv) else false;
    return b == null;
}
/// Plans all writes in memory, including real runs. Conflicts with `neither`
/// abort before any IDE, baseline, backup or store file is changed.
pub const Staging = struct {
    fs: Fs,
    pending: Map = .empty,
    original: Map = .empty,
    listings: std.StringHashMapUnmanaged([][]const u8) = .empty,
    roots: []const []const u8 = &.{},
    backups: std.ArrayList([]const u8) = .empty,
    fn validate(self: *Staging, path: []const u8) !void {
        for (self.roots) |root| {
            const prefix = try self.fs.join(&.{ root, "_" });
            const directory = prefix[0 .. prefix.len - 1];
            if (!std.mem.startsWith(u8, path, directory)) continue;
            const relative = try self.fs.allocator.dupe(u8, path[directory.len..]);
            std.mem.replaceScalar(u8, relative, '\\', '/');
            try self.fs.checkPath(root, relative);
            return;
        }
        if (self.roots.len != 0) return error.UnsafeStorePath;
    }
    pub fn read(self: *Staging, path: []const u8) !?[]const u8 {
        if (self.pending.getEntry(path)) |entry| return entry.value_ptr.*;
        if (self.original.getEntry(path)) |entry| return entry.value_ptr.*;
        try self.validate(path);
        const bytes = try self.fs.read(path);
        try self.original.put(self.fs.allocator, path, bytes);
        return bytes;
    }
    pub fn write(self: *Staging, path: []const u8, data: ?[]const u8) !void {
        if (!self.original.contains(path)) _ = try self.read(path);
        if (equal(self.original.get(path).?, data)) {
            _ = self.pending.remove(path);
            return;
        }
        try self.validate(path);
        try self.pending.put(self.fs.allocator, path, data);
    }
    pub fn list(self: *Staging, root: []const u8) ![][]const u8 {
        return self.listFiltered(root, false);
    }
    pub fn listIde(self: *Staging, root: []const u8) ![][]const u8 {
        return self.listFiltered(root, true);
    }
    fn listFiltered(self: *Staging, root: []const u8, ide_settings: bool) ![][]const u8 {
        const cached = try self.listings.getOrPut(self.fs.allocator, root);
        if (!cached.found_existing) cached.value_ptr.* = try self.fs.listFiltered(root, ide_settings);
        var paths: std.StringHashMapUnmanaged(void) = .empty;
        for (cached.value_ptr.*) |path| try paths.put(self.fs.allocator, path, {});
        const prefix = try std.fmt.allocPrint(self.fs.allocator, "{s}{s}", .{ root, std.fs.path.sep_str });
        var it = self.pending.iterator();
        while (it.next()) |entry| {
            if (std.mem.startsWith(u8, entry.key_ptr.*, prefix)) {
                const path = entry.key_ptr.*[prefix.len..];
                if (entry.value_ptr.* != null) try paths.put(self.fs.allocator, path, {}) else _ = paths.remove(path);
            }
        }
        var result = try self.fs.allocator.alloc([]const u8, paths.count());
        var keys = paths.keyIterator();
        var i: usize = 0;
        while (keys.next()) |key| {
            result[i] = key.*;
            i += 1;
        }
        std.mem.sort([]const u8, result, {}, less);
        return result;
    }
    pub fn backup(self: *Staging, path: []const u8, data: ?[]const u8) !void {
        // Preserve the pre-run bytes if a later convergence pass touches the
        // same file again. A backup is committed before any original changes.
        if (self.pending.contains(path)) return;
        try self.write(path, data);
        try self.backups.append(self.fs.allocator, path);
    }
    pub fn commit(self: *Staging) !void {
        // Validate the complete plan before the first write, including files
        // whose parent may have become a symlink while reconciliation ran.
        var preflight = self.pending.iterator();
        while (preflight.next()) |entry| {
            try self.validate(entry.key_ptr.*);
            if (!equal(try self.fs.read(entry.key_ptr.*), self.original.get(entry.key_ptr.*).?)) return error.SettingsChangedDuringSync;
        }
        for (self.backups.items) |path| try self.fs.write(path, self.pending.get(path).?);
        var it = self.pending.iterator();
        while (it.next()) |entry| {
            var is_backup = false;
            for (self.backups.items) |path| if (std.mem.eql(u8, path, entry.key_ptr.*)) {
                is_backup = true;
                break;
            };
            if (!is_backup and !equal(self.original.get(entry.key_ptr.*).?, entry.value_ptr.*)) try self.fs.write(entry.key_ptr.*, entry.value_ptr.*);
        }
    }
};

test "staged reads are stable and concurrent edits abort before any write" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var temp = std.testing.tmpDir(.{});
    defer temp.cleanup();
    const root = try temp.dir.realPathFileAlloc(std.testing.io, ".", a);
    const fs: Fs = .{ .allocator = a, .io = std.testing.io };
    const first = try fs.join(&.{ root, "first.xml" });
    const second = try fs.join(&.{ root, "second.xml" });
    try fs.write(first, "old");
    try fs.write(second, "original");
    var staging: Staging = .{ .fs = fs, .roots = &.{root} };
    try std.testing.expectEqualStrings("old", (try staging.read(first)).?);
    try staging.write(first, "planned");
    try staging.write(second, "new");
    try fs.write(first, "concurrent");
    try std.testing.expectEqualStrings("planned", (try staging.read(first)).?);
    try std.testing.expectError(error.SettingsChangedDuringSync, staging.commit());
    try std.testing.expectEqualStrings("concurrent", (try fs.read(first)).?);
    try std.testing.expectEqualStrings("original", (try fs.read(second)).?);
}

test "settings traversal skips private IDE directories and Git internals" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var temp = std.testing.tmpDir(.{});
    defer temp.cleanup();
    const root = try temp.dir.realPathFileAlloc(std.testing.io, ".", a);
    const fs: Fs = .{ .allocator = a, .io = std.testing.io };
    for ([_][]const u8{ "options/editor.xml", "workspace/huge.xml", "plugins/plugin.xml", ".git/objects/blob" }) |path| try fs.write(try fs.join(&.{ root, path }), "x");
    const list = try fs.listFiltered(root, true);
    try std.testing.expectEqual(@as(usize, 1), list.len);
    try std.testing.expectEqualStrings("options/editor.xml", list[0]);
    try std.testing.expectEqual(@as(usize, 3), (try fs.list(root)).len);
}

test "a reverted staged write leaves no work" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var temp = std.testing.tmpDir(.{});
    defer temp.cleanup();
    const root = try temp.dir.realPathFileAlloc(std.testing.io, ".", a);
    const fs: Fs = .{ .allocator = a, .io = std.testing.io };
    const path = try fs.join(&.{ root, "settings" });
    try fs.write(path, "old");
    var staging: Staging = .{ .fs = fs, .roots = &.{root} };
    try staging.write(path, "new");
    try staging.write(path, "old");
    try std.testing.expectEqual(@as(u32, 0), staging.pending.count());
    try staging.commit();
    try std.testing.expectEqualStrings("old", (try fs.read(path)).?);
}
