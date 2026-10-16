const std = @import("std");
const config = @import("config.zig");
const files = @import("files.zig");
const settings = @import("settings.zig");
const merge = @import("merge.zig");
const xml = @import("xml.zig");
const Git = @import("git.zig").Git;
const A = std.mem.Allocator;
pub const Options = struct {
    dry_run: bool = false,
    policy: merge.Policy = .local,
    only: []const []const u8 = &.{},
    collect_only: bool = false,
    install_plugins: bool = true,
    message: []const u8 = "Sync JetBrains settings",
};
pub const Report = struct {
    incoming: usize = 0,
    outgoing: usize = 0,
    conflicts: usize = 0,
    pruned: usize = 0,
    details: std.Io.Writer.Allocating,
    pub fn init(a: A) Report {
        return .{ .details = .init(a) };
    }
    fn absorb(self: *Report, label: []const u8, r: merge.Result) !void {
        self.incoming += r.incoming.items.len;
        self.outgoing += r.outgoing.items.len;
        self.conflicts += r.conflicts.items.len;
        for (r.incoming.items) |c| try self.details.writer.print("{s}: to IDE/store {s} {s} -> {s}\n", .{ label, c.path, c.before orelse "(absent)", c.after orelse "(absent)" });
        for (r.outgoing.items) |c| try self.details.writer.print("{s}: to store {s} {s} -> {s}\n", .{ label, c.path, c.before orelse "(absent)", c.after orelse "(absent)" });
        for (r.conflicts.items) |c| try self.details.writer.print("{s}: conflict {s}: local {s}, remote {s}\n", .{ label, c.path, c.local orelse "(absent)", c.remote orelse "(absent)" });
    }
};
pub const Engine = struct {
    fs: files.Fs,
    app: []const u8,
    local: config.Local,
    policy: config.Sync,
    git: Git,
    ides: []settings.Ide,
    pub fn loadLocal(fs: files.Fs, raw: []const u8, env: *const std.process.Environ.Map, fallback: ?[]const u8) !config.Local {
        return config.Local.load(fs.allocator, raw, fallback) catch |err| switch (err) {
            error.MachineIdNotConfigured => config.Local.load(fs.allocator, raw, try hostname(fs, env)),
            else => return err,
        };
    }
    pub fn open(fs: files.Fs, app: []const u8, env: *const std.process.Environ.Map) !Engine {
        const local = try loadLocal(fs, (try fs.read(try fs.join(&.{ app, "config.toml" }))) orelse "", env, null);
        const store = local.store orelse try fs.join(&.{ app, "data" });
        const machine_file = try std.fmt.allocPrint(fs.allocator, "{s}.toml", .{local.machine});
        const policy = try config.Sync.load(fs.allocator, (try fs.read(try fs.join(&.{ store, "sync.toml" }))) orelse "", (try fs.read(try fs.join(&.{ store, "machines", machine_file }))) orelse "");
        const home = env.get("HOME") orelse return error.HomeNotFound;
        const default_root = try fs.join(&.{ home, "Library/Application Support/JetBrains" });
        const root = if (local.root) |r| if (std.mem.eql(u8, r, "auto")) default_root else r else default_root;
        const installations = if (local.install_roots.len != 0) local.install_roots else &[_][]const u8{ "/Applications", try fs.join(&.{ home, "Applications" }), try fs.join(&.{ home, "Library/Application Support/JetBrains/Toolbox/apps" }) };
        return .{ .fs = fs, .app = app, .local = local, .policy = policy, .git = .{ .fs = fs, .root = store, .remote = local.remote, .branch = local.branch }, .ides = try settings.discover(fs, root, &.{"*"}, installations) };
    }
    pub fn lock(self: Engine) !std.Io.File {
        return lockAt(self.fs, self.app);
    }
    pub fn lockAt(fs: files.Fs, app: []const u8) !std.Io.File {
        try fs.mkdir(app);
        {
            const dir = try std.Io.Dir.cwd().openDir(fs.io, app, .{});
            defer dir.close(fs.io);
            try dir.setPermissions(fs.io, .fromMode(0o700));
        }
        const f = try std.Io.Dir.cwd().createFile(fs.io, try fs.join(&.{ app, "sync.lock" }), .{ .truncate = false });
        errdefer f.close(fs.io);
        if (!try f.tryLock(fs.io, .exclusive)) return error.AnotherSyncInProgress;
        return f;
    }
    fn loadPolicy(self: *Engine, staging: *files.Staging) !void {
        const machine = try std.fmt.allocPrint(self.fs.allocator, "{s}.toml", .{self.local.machine});
        self.policy = try config.Sync.load(self.fs.allocator, (try staging.read(try self.fs.join(&.{ self.git.root, "sync.toml" }))) orelse "", (try staging.read(try self.fs.join(&.{ self.git.root, "machines", machine }))) orelse "");
    }
    pub fn sync(self: *Engine, options: Options, out: *std.Io.Writer, verbose: bool) !void {
        const a = self.fs.allocator;
        const guard = try self.lock();
        defer guard.close(self.fs.io);
        // Dry runs use the existing repository; they never create/adopt a store.
        if (!options.dry_run) try self.git.initialize(false);
        var trusted_roots: std.ArrayList([]const u8) = .empty;
        try trusted_roots.appendSlice(a, &.{ self.app, self.git.root });
        for (self.ides) |ide| try trusted_roots.append(a, ide.path);
        var staging: files.Staging = .{ .fs = self.fs, .roots = trusted_roots.items };
        var report = Report.init(a);
        var cursor: ?[]const u8 = null;
        if (try self.fs.exists(try self.fs.join(&.{ self.git.root, ".git" }))) {
            if (try self.git.incoming()) |incoming| {
                cursor = incoming.cursor;
                var local_view: xml.View = .empty;
                for (try staging.list(self.git.root)) |relative| {
                    if (try staging.read(try self.fs.join(&.{ self.git.root, relative }))) |bytes| try local_view.put(a, relative, bytes);
                }
                for (try merge.unionKeys(a, &.{ incoming.base, incoming.remote, local_view })) |relative| {
                    if (std.mem.eql(u8, relative, ".gitattributes")) continue;
                    try self.fs.checkPath(self.git.root, relative);
                    const r = if (std.mem.startsWith(u8, relative, "shared/")) try merge.file(a, incoming.base.get(relative), local_view.get(relative), incoming.remote.get(relative), options.policy) else try merge.whole(a, incoming.base.get(relative), local_view.get(relative), incoming.remote.get(relative), options.policy);
                    try report.absorb(relative, r);
                    try staging.write(try self.fs.join(&.{ self.git.root, relative }), r.content);
                }
            }
        }
        // Remote policy is effective in this very run, including dry runs.
        try self.loadPolicy(&staging);
        var selected_ides: std.ArrayList(settings.Ide) = .empty;
        for (self.ides) |ide| if (@import("settings.zig").any(self.policy.ides, ide.name)) try selected_ides.append(a, ide);
        self.ides = selected_ides.items;
        settings.sortIdes(self.ides, self.policy.ides);
        var learned: std.ArrayList([]const u8) = .empty;
        const manifest_file = try self.fs.join(&.{ self.git.root, "manifest.toml" });
        if (try staging.read(manifest_file)) |raw| try learned.appendSlice(a, try config.strings(a, try config.parse(a, raw), "roamable", &.{}));
        var observed = false;
        for (self.ides) |ide| {
            if (!ide.launched) continue;
            for (try self.fs.list(try self.fs.join(&.{ ide.path, "settingsSync" }))) |relative| {
                var hidden = false;
                var parts = std.mem.splitScalar(u8, relative, '/');
                while (parts.next()) |part| if (std.mem.startsWith(u8, part, ".")) {
                    hidden = true;
                };
                if (hidden or !settings.safeRelative(relative) or std.mem.eql(u8, relative, "plugins.json")) continue;
                if (!contains(learned.items, relative)) {
                    try learned.append(a, relative);
                    observed = true;
                }
            }
        }
        if (observed) {
            std.mem.sort([]const u8, learned.items, {}, files.less);
            var encoded: std.Io.Writer.Allocating = .init(a);
            try encoded.writer.writeAll("version = 1\nroamable = [\n");
            for (learned.items) |path| try encoded.writer.print("  {s},\n", .{try config.quote(a, path)});
            try encoded.writer.writeAll("]\n");
            try staging.write(manifest_file, try encoded.toOwnedSlice());
        }
        for (self.ides) |ide| if (!ide.launched) try self.captureDefaults(&staging, ide, learned.items);
        var random: [8]u8 = undefined;
        self.fs.io.random(&random);
        const stamp = try std.fmt.allocPrint(a, "{d}-{x}", .{ std.Io.Clock.real.now(self.fs.io).toMilliseconds(), std.mem.readInt(u64, &random, .little) });
        for (0..4) |pass| {
            var moved = false;
            for (self.ides) |ide| {
                if (!ide.selected(options.only)) continue;
                if (!ide.launched) {
                    if (pass == 0) try report.details.writer.print("{s}: never launched; recorded factory defaults, skipped\n", .{ide.name});
                    continue;
                }
                var paths: std.StringHashMapUnmanaged([]const u8) = .empty;
                for (try staging.listIde(ide.path)) |relative| {
                    if (!settings.roamable(self.policy, learned.items, relative)) continue;
                    const canonical = if (std.mem.eql(u8, relative, try settings.vmName(ide, self.policy))) "idea.vmoptions" else relative;
                    try paths.put(a, canonical, relative);
                }
                const shared = try self.fs.join(&.{ self.git.root, "shared" });
                const base_root = try self.fs.join(&.{ self.app, "base", ide.name });
                for ([_][]const u8{ shared, base_root }) |dir| for (try staging.list(dir)) |relative| {
                    const target = try settings.targetPath(ide, self.policy, relative);
                    if (!settings.excluded(self.policy, target)) try paths.put(a, relative, target);
                };
                var sort_view: xml.View = .empty;
                var it = paths.iterator();
                while (it.next()) |entry| try sort_view.put(a, entry.key_ptr.*, entry.value_ptr.*);
                const defaults_file = try self.fs.join(&.{ self.git.root, "defaults", try std.fmt.allocPrint(a, "{s}.toml", .{try config.sanitize(a, ide.product)}) });
                const defaults = try config.parse(a, (try staging.read(defaults_file)) orelse "");
                for (try merge.unionKeys(a, &.{sort_view})) |relative| {
                    const target = paths.get(relative).?;
                    if (!settings.safeRelative(relative) or !settings.safeRelative(target)) return error.UnsafeStorePath;
                    const ide_file = try self.fs.join(&.{ ide.path, target });
                    const store_file = try self.fs.join(&.{ shared, relative });
                    const base_file = try self.fs.join(&.{ base_root, relative });
                    const raw = try staging.read(ide_file);
                    const base = try staging.read(base_file);
                    const remote = try staging.read(store_file);
                    const pruned = try settings.view(a, relative, raw, self.policy, defaults);
                    // A file holding only defaults means no opinion, rather
                    // than a deletion that would withdraw another IDE's choices.
                    const local = if (raw != null and pruned.content == null and !std.mem.eql(u8, std.mem.trim(u8, raw.?, " \r\n\t"), "DELETED")) base else pruned.content;
                    const r = try merge.file(a, base, local, remote, options.policy);
                    try staging.write(base_file, if (options.collect_only) local else r.content);
                    if (r.changed()) {
                        moved = true;
                        const label = try std.fmt.allocPrint(a, "{s}/{s}", .{ ide.name, relative });
                        try report.absorb(label, r);
                        try staging.write(store_file, r.content);
                        if (!options.collect_only and r.incoming.items.len != 0) try self.writeBack(&staging, ide, relative, ide_file, raw, pruned.content, r, stamp);
                    }
                    if (pass == 0) {
                        report.pruned += pruned.removed;
                        if (verbose and pruned.removed != 0) try report.details.writer.print("{s}/{s}: pruned {d} defaults/private settings\n", .{ ide.name, relative, pruned.removed });
                    }
                }
            }
            if (!moved) break;
            if (pass == 3) try report.details.writer.writeAll("warning: reconciliation did not settle after four passes\n");
        }
        const plugin_actions = if (self.policy.plugins) try @import("plugins.zig").reconcile(self.fs, &staging, self.git.root, self.ides, self.policy, options, &report.details.writer) else &.{};
        try out.print("{s} · {s}\n\n{d} incoming · {d} outgoing · {d} conflict(s)\n", .{ self.local.machine, self.local.remote orelse "local store, no remote", report.incoming, report.outgoing, report.conflicts });
        try out.writeAll(report.details.written());
        if (options.policy == .neither and report.conflicts != 0) {
            try out.writeAll("Conflicts found; no settings were written.\n");
            return error.UnresolvedConflicts;
        }
        if (options.dry_run) {
            try out.writeAll("Dry run; no settings were written.\n");
            return;
        }
        try staging.commit();
        if (options.install_plugins) try @import("plugins.zig").install(self.fs, plugin_actions, self.policy);
        if (cursor) |tip| try self.git.reconcile(tip, options.message);
        if (try self.git.publish(options.message)) |commit| try out.print("Committed: {s}\n", .{commit});
        self.fs.pruneBackups(try self.fs.join(&.{ self.app, "backups" }), 10) catch {};
    }
    fn captureDefaults(self: Engine, staging: *files.Staging, ide: settings.Ide, manifest: []const []const u8) !void {
        const a = self.fs.allocator;
        const path = try self.fs.join(&.{ self.git.root, "defaults", try std.fmt.allocPrint(a, "{s}.toml", .{try config.sanitize(a, ide.product)}) });
        const stored = try config.parse(a, (try staging.read(path)) orelse "");
        var values: config.Value = try config.table(stored, "files");
        var changed = false;
        for (try staging.listIde(ide.path)) |relative| {
            if (!settings.roamable(self.policy, manifest, relative)) continue;
            const raw = try self.fs.read(try self.fs.join(&.{ ide.path, relative })) orelse continue;
            const n = xml.parse(a, raw) catch |err| switch (err) {
                error.OutOfMemory => return err,
                else => continue,
            };
            const projection = xml.project(a, n) catch |err| switch (err) {
                // A positional default cannot safely describe repeated keys.
                error.AmbiguousXmlAddress => continue,
                else => return err,
            };
            if (projection.count() == 0) continue;
            var table: config.Table = .empty;
            var it = projection.iterator();
            while (it.next()) |entry| try table.put(a, entry.key_ptr.*, .{ .string = entry.value_ptr.* });
            try values.table.put(a, relative, .{ .table = table });
            changed = true;
        }
        if (!changed) return;
        var out: std.Io.Writer.Allocating = .init(a);
        try out.writer.print("version = 1\nproduct = {s}\nbuild = {s}\n", .{ try config.quote(a, ide.product), try config.quote(a, ide.metadata.build) });
        var view: xml.View = .empty;
        var it = values.table.iterator();
        while (it.next()) |entry| try view.put(a, entry.key_ptr.*, "");
        for (try merge.unionKeys(a, &.{view})) |file_path| {
            try out.writer.print("\n[files.{s}]\n", .{try config.quote(a, file_path)});
            var leaves: xml.View = .empty;
            var entries = values.table.get(file_path).?.table.iterator();
            while (entries.next()) |entry| try leaves.put(a, entry.key_ptr.*, try entry.value_ptr.str());
            for (try merge.unionKeys(a, &.{leaves})) |address| try out.writer.print("{s} = {s}\n", .{ try config.quote(a, address), try config.quote(a, leaves.get(address).?) });
        }
        try staging.write(path, try out.toOwnedSlice());
    }
    fn writeBack(self: Engine, staging: *files.Staging, ide: settings.Ide, relative: []const u8, path: []const u8, raw: ?[]const u8, shared_before: ?[]const u8, result: merge.Result, stamp: []const u8) !void {
        const a = self.fs.allocator;
        var updated = result.content;
        if (raw) |bytes| {
            if (xml.parse(a, bytes) catch |err| switch (err) {
                error.OutOfMemory => return err,
                else => null,
            }) |target| {
                const donor = if (result.content) |content| xml.parse(a, content) catch |err| switch (err) {
                    error.OutOfMemory => return err,
                    else => null,
                } else null;
                if (donor != null) replacement: {
                    for (result.incoming.items) |change| {
                        if (change.path.len != 0) continue;
                        _ = xml.project(a, donor.?) catch |err| switch (err) {
                            error.AmbiguousXmlAddress => {
                                // A whole-file fallback must never graft two
                                // repeated addresses onto the same sibling.
                                // Replace only when this contains no private
                                // or default values that would be discarded.
                                if (!files.equal(try xml.serialize(a, target), shared_before)) return error.AmbiguousXmlWithPrivateSettings;
                                updated = result.content;
                                break :replacement;
                            },
                            else => return err,
                        };
                    }
                    var updates: xml.View = .empty;
                    var removals: std.ArrayList([]const u8) = .empty;
                    for (result.incoming.items) |change| {
                        if (change.path.len == 0) try xml.graft(a, target, donor.?) else if (change.after) |value| try updates.put(a, change.path, value) else try removals.append(a, change.path);
                    }
                    try xml.patch(a, target, donor.?, updates, removals.items);
                    xml.pruneEmpty(target);
                    updated = try xml.serialize(a, target);
                } else if (result.content == null) {
                    // Delete only the previously shared leaves, preserving all
                    // private/default content in the real IDE document.
                    const base = try self.fs.read(try self.fs.join(&.{ self.app, "base", ide.name, relative }));
                    if (base) |b| {
                        const n = try xml.parse(a, b);
                        const projection = try xml.project(a, n);
                        var it = projection.keyIterator();
                        while (it.next()) |key| try xml.remove(a, target, key.*);
                        xml.pruneEmpty(target);
                        updated = try xml.serialize(a, target);
                    }
                }
            }
        }
        if (files.equal(raw, updated)) return;
        if (self.policy.backups and raw != null) try staging.backup(try self.fs.join(&.{ self.app, "backups", stamp, ide.name, relative }), raw);
        try staging.write(path, updated);
    }
    pub fn disable(self: Engine, dry: bool, out: *std.Io.Writer) !void {
        const guard = try self.lock();
        defer guard.close(self.fs.io);
        for (self.ides) |ide| {
            if (!ide.launched) continue;
            const switches = [_][3][]const u8{ .{ "settingsSync.xml", "SettingsSyncSettings", "syncEnabled" }, .{ "settingsSyncLocal.xml", "SettingsSyncLocalSettings", "crossIdeSyncEnabled" } };
            for (switches) |s| {
                const path = try self.fs.join(&.{ ide.path, "options", s[0] });
                const raw = try self.fs.read(path);
                const target = if (raw) |r| try xml.parse(self.fs.allocator, r) else try xml.Node.create(self.fs.allocator, "application");
                const source = try std.fmt.allocPrint(self.fs.allocator, "<application><component name='{s}'><option name='{s}' value='false'/></component></application>", .{ s[1], s[2] });
                const donor = try xml.parse(self.fs.allocator, source);
                const projection = try xml.project(self.fs.allocator, donor);
                var it = projection.iterator();
                while (it.next()) |entry| try xml.set(self.fs.allocator, target, donor, entry.key_ptr.*, entry.value_ptr.*);
                const content = try xml.serialize(self.fs.allocator, target);
                if (!files.equal(raw, content)) {
                    try out.print("{s}: disable Backup and Sync ({s})\n", .{ ide.name, s[2] });
                    if (!dry) try self.fs.write(path, content);
                }
            }
        }
    }
};
fn contains(list: []const []const u8, text: []const u8) bool {
    for (list) |s| if (std.mem.eql(u8, s, text)) return true;
    return false;
}

// Zig has no portable hostname I/O operation yet. Use the OS hostname utility
// without a shell, only for identity (Git remains the transport dependency).
pub fn hostname(fs: files.Fs, env: *const std.process.Environ.Map) ![]const u8 {
    if (env.get("HOSTNAME")) |name| return name;
    const result = try std.process.run(fs.allocator, fs.io, .{ .argv = &.{"hostname"}, .stdout_limit = .limited(4096), .stderr_limit = .limited(4096) });
    const name = std.mem.trim(u8, result.stdout, " \r\n\t");
    if (result.term != .exited or result.term.exited != 0 or name.len == 0) return error.HostnameUnavailable;
    return name;
}
