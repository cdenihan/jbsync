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
            {
                const f = try cwd.createFile(self.io, temp, .{ .exclusive = true, .permissions = if (@import("builtin").os.tag == .windows) .default_file else .fromMode(0o600) });
                defer f.close(self.io);
                try f.writeStreamingAll(self.io, bytes);
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
        var found: std.ArrayList([]const u8) = .empty;
        const dir = std.Io.Dir.cwd().openDir(self.io, root, .{ .iterate = true }) catch |err| switch (err) {
            error.FileNotFound => return &.{},
            else => return err,
        };
        defer dir.close(self.io);
        var walker = try dir.walk(self.allocator);
        defer walker.deinit();
        while (try walker.next(self.io)) |entry| {
            if (entry.kind != .file or internal(entry.path)) continue;
            const path = try self.allocator.dupe(u8, entry.path);
            std.mem.replaceScalar(u8, path, '\\', '/');
            try found.append(self.allocator, path);
        }
        std.mem.sort([]const u8, found.items, {}, less);
        return found.toOwnedSlice(self.allocator);
    }
};
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
        try self.validate(path);
        if (self.pending.getEntry(path)) |entry| return entry.value_ptr.*;
        return self.fs.read(path);
    }
    pub fn write(self: *Staging, path: []const u8, data: ?[]const u8) !void {
        try self.validate(path);
        try self.pending.put(self.fs.allocator, path, data);
    }
    pub fn list(self: *Staging, root: []const u8) ![][]const u8 {
        var paths: std.StringHashMapUnmanaged(void) = .empty;
        for (try self.fs.list(root)) |path| try paths.put(self.fs.allocator, path, {});
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
        for (self.backups.items) |path| try self.fs.write(path, self.pending.get(path).?);
        var it = self.pending.iterator();
        while (it.next()) |entry| {
            var is_backup = false;
            for (self.backups.items) |path| if (std.mem.eql(u8, path, entry.key_ptr.*)) {
                is_backup = true;
                break;
            };
            if (!is_backup and !equal(try self.fs.read(entry.key_ptr.*), entry.value_ptr.*)) try self.fs.write(entry.key_ptr.*, entry.value_ptr.*);
        }
    }
};
