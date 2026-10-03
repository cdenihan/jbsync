//! Git is a transport. Settings are reconciled structurally before recording
//! the remote tip as a parent with an `ours` merge.
const std = @import("std");
const files = @import("files.zig");
const config = @import("config.zig");
const settings = @import("settings.zig");
pub const Tree = @import("xml.zig").View;
pub const Git = struct {
    fs: files.Fs,
    root: []const u8,
    remote: ?[]const u8,
    branch: []const u8,
    pub fn command(self: Git, args: []const []const u8) !std.process.RunResult {
        var argv: std.ArrayList([]const u8) = .empty;
        try argv.appendSlice(self.fs.allocator, &.{ "git", "-C", self.root });
        try argv.appendSlice(self.fs.allocator, args);
        return std.process.run(self.fs.allocator, self.fs.io, .{ .argv = argv.items, .stdout_limit = .limited(files.max_file_size), .stderr_limit = .limited(1024 * 1024) });
    }
    pub fn success(result: std.process.RunResult) bool {
        return result.term == .exited and result.term.exited == 0;
    }
    pub fn run(self: Git, args: []const []const u8) ![]const u8 {
        const result = try self.command(args);
        if (!success(result)) {
            std.Io.File.stderr().writeStreamingAll(self.fs.io, result.stderr) catch {};
            return error.GitCommandFailed;
        }
        return result.stdout;
    }
    pub fn optional(self: Git, args: []const []const u8) !?[]const u8 {
        const result = try self.command(args);
        if (!success(result)) return null;
        const trimmed = std.mem.trim(u8, result.stdout, " \r\n\t");
        return if (trimmed.len == 0) null else trimmed;
    }
    pub fn initialize(self: Git, adopt_remote: bool) !void {
        if (self.branch.len == 0 or self.branch[0] == '-') return error.InvalidBranch;
        if (self.remote) |remote| if (remote.len == 0 or remote[0] == '-') return error.InvalidRemote;
        try self.fs.mkdir(self.root);
        _ = try self.run(&.{ "check-ref-format", "--branch", self.branch });
        if (!try self.fs.exists(try self.fs.join(&.{ self.root, ".git" }))) {
            _ = try self.run(&.{ "init", "-b", self.branch });
        }
        if (try self.optional(&.{ "config", "user.email" }) == null) _ = try self.run(&.{ "config", "user.email", "jbsync@localhost" });
        if (try self.optional(&.{ "config", "user.name" }) == null) _ = try self.run(&.{ "config", "user.name", "jbsync" });
        const origin = try self.optional(&.{ "remote", "get-url", "origin" });
        if (self.remote) |remote| {
            if (origin == null) _ = try self.run(&.{ "remote", "add", "origin", remote }) else if (!std.mem.eql(u8, origin.?, remote)) _ = try self.run(&.{ "remote", "set-url", "origin", remote });
            if (adopt_remote and try self.optional(&.{ "rev-parse", "--verify", "HEAD" }) == null) {
                if (try self.fetch()) |tip| _ = try self.run(&.{ "reset", "--hard", tip });
            }
        } else if (origin != null) _ = try self.run(&.{ "remote", "remove", "origin" });
        const attributes = try self.fs.join(&.{ self.root, ".gitattributes" });
        const current = try self.fs.read(attributes);
        if (current == null or files.equal(current, "* -text -diff=auto\n")) try self.fs.write(attributes, "* -text\n");
    }
    fn fetch(self: Git) !?[]const u8 {
        if (self.remote == null) return null;
        const result = try self.command(&.{ "fetch", "origin", self.branch });
        if (!success(result)) {
            // A genuinely empty remote is valid. Authentication/network errors
            // must surface rather than silently syncing stale local state.
            const listing = try self.run(&.{ "ls-remote", "--heads", "origin", self.branch });
            if (std.mem.trim(u8, listing, " \r\n").len == 0) return null;
            return error.GitFetchFailed;
        }
        const reference = try std.fmt.allocPrint(self.fs.allocator, "refs/remotes/origin/{s}", .{self.branch});
        return self.optional(&.{ "rev-parse", "--verify", reference });
    }
    pub const Incoming = struct { base: Tree, remote: Tree, cursor: []const u8 };
    pub fn incoming(self: Git) !?Incoming {
        const tip = try self.fetch() orelse return null;
        if (success(try self.command(&.{ "merge-base", "--is-ancestor", tip, "HEAD" }))) return null;
        const base = try self.optional(&.{ "merge-base", "HEAD", tip });
        return .{ .base = if (base) |b| try self.tree(b) else .empty, .remote = try self.tree(tip), .cursor = tip };
    }
    pub fn tree(self: Git, reference: []const u8) !Tree {
        var result: Tree = .empty;
        const raw = try self.run(&.{ "ls-tree", "-r", "-z", reference });
        var it = std.mem.splitScalar(u8, raw, 0);
        while (it.next()) |line| {
            if (line.len == 0) continue;
            const tab = std.mem.indexOfScalar(u8, line, '\t') orelse return error.InvalidGitTree;
            const path = line[tab + 1 ..];
            if (!settings.safeRelative(path)) return error.UnsafeStorePath;
            // Refuse symlinks/submodules; store entries are ordinary files.
            if (!std.mem.startsWith(u8, line, "100644 blob ") and !std.mem.startsWith(u8, line, "100755 blob ")) return error.UnsafeStorePath;
            const blob = try std.fmt.allocPrint(self.fs.allocator, "{s}:{s}", .{ reference, path });
            try result.put(self.fs.allocator, path, try self.run(&.{ "show", blob }));
        }
        return result;
    }
    fn commitIfChanged(self: Git, message: []const u8) !bool {
        _ = try self.run(&.{ "add", "-A" });
        const changes = try self.run(&.{ "diff", "--cached", "--name-only" });
        if (changes.len == 0) return false;
        _ = try self.run(&.{ "commit", "--no-verify", "-m", message });
        return true;
    }
    pub fn reconcile(self: Git, cursor: []const u8, message: []const u8) !void {
        _ = try self.commitIfChanged(message);
        if (success(try self.command(&.{ "merge-base", "--is-ancestor", cursor, "HEAD" }))) return;
        _ = try self.run(&.{ "merge", "-s", "ours", "--allow-unrelated-histories", "--no-verify", "-m", message, cursor });
    }
    pub fn publish(self: Git, message: []const u8) !?[]const u8 {
        const changed = try self.commitIfChanged(message);
        if (self.remote != null and try self.optional(&.{ "rev-parse", "--verify", "HEAD" }) != null) _ = try self.run(&.{ "push", "-u", "origin", self.branch });
        return if (changed) try self.optional(&.{ "rev-parse", "--short=8", "HEAD" }) else null;
    }
};
