const std = @import("std");
const files = @import("files.zig");
const config = @import("config.zig");
const Engine = @import("engine.zig").Engine;
const Options = @import("engine.zig").Options;
const plugins = @import("plugins.zig");
const A = std.mem.Allocator;
const eq = std.mem.eql;
const usage =
    \\jbsync — settings and plugin sync for JetBrains IDEs
    \\
    \\Usage: jbsync [--config-dir PATH] [--verbose] COMMAND
    \\Commands:
    \\  init [--remote URL] [--machine ID]
    \\  status
    \\  sync [--dry-run] [--prefer local|remote|neither] [--ide GLOB]
    \\       [--collect-only] [--no-install-plugins] [--message TEXT]
    \\  ides
    \\  repo show|set URL|unset
    \\  plugins [only|allow|deny ID [--ide GLOB]]
    \\  disable-builtin-sync [--dry-run]
    \\  completions bash|zsh|fish|powershell|elvish
    \\  update [--version VERSION] [--json]
    \\
;
const Args = struct {
    words: std.ArrayList([]const u8) = .empty,
    flags: std.ArrayList([]const u8) = .empty,
    root: ?[]const u8 = null,
    remote: ?[]const u8 = null,
    machine: ?[]const u8 = null,
    verbose: bool = false,
    help: bool = false,
    version: bool = false,
    update_version: []const u8 = "latest",
    json: bool = false,
    options: Options = .{},
};
fn parseArgs(a: A, argv: []const []const u8) !Args {
    var result: Args = .{};
    var only: std.ArrayList([]const u8) = .empty;
    var i: usize = 1;
    while (i < argv.len) : (i += 1) {
        const s = argv[i];
        const update_command = result.words.items.len != 0 and eq(u8, result.words.items[0], "update");
        if (s.len != 0 and s[0] == '-') {
            const end = std.mem.indexOfScalar(u8, s, '=') orelse s.len;
            try result.flags.append(a, s[0..end]);
        }
        if (eq(u8, s, "--json")) result.json = true else if (eq(u8, s, "--help") or eq(u8, s, "-h")) result.help = true else if ((eq(u8, s, "--version") and !update_command) or eq(u8, s, "-V")) result.version = true else if (eq(u8, s, "--verbose") or eq(u8, s, "-v")) result.verbose = true else if (eq(u8, s, "--dry-run")) result.options.dry_run = true else if (eq(u8, s, "--collect-only")) result.options.collect_only = true else if (eq(u8, s, "--no-install-plugins")) result.options.install_plugins = false else if (eq(u8, s, "--install-plugins")) {} else if (s.len != 0 and s[0] == '-') {
            const sep = std.mem.indexOfScalar(u8, s, '=');
            const flag = if (sep) |n| s[0..n] else s;
            const recognized = eq(u8, flag, "--config-dir") or eq(u8, flag, "--remote") or eq(u8, flag, "--machine") or eq(u8, flag, "--ide") or eq(u8, flag, "--message") or eq(u8, flag, "-m") or eq(u8, flag, "--prefer") or (update_command and eq(u8, flag, "--version"));
            if (!recognized) return error.UnknownOption;
            const value = if (sep) |n| s[n + 1 ..] else blk: {
                i += 1;
                if (i >= argv.len) return error.MissingOptionValue;
                break :blk argv[i];
            };
            if (value.len == 0) return error.MissingOptionValue;
            if (eq(u8, flag, "--version")) result.update_version = value else if (eq(u8, flag, "--config-dir")) result.root = value else if (eq(u8, flag, "--remote")) result.remote = value else if (eq(u8, flag, "--machine")) result.machine = value else if (eq(u8, flag, "--ide")) try only.append(a, value) else if (eq(u8, flag, "--prefer")) result.options.policy = std.meta.stringToEnum(@import("merge.zig").Policy, value) orelse return error.InvalidConflictPolicy else result.options.message = value;
        } else try result.words.append(a, s);
    }
    result.options.only = only.items;
    return result;
}
pub fn run(a: A, io: std.Io, env: *const std.process.Environ.Map, argv: []const []const u8, out: *std.Io.Writer) !void {
    const args = try parseArgs(a, argv);
    const words = args.words.items;
    if (args.version) {
        try out.print("jbsync {s}\n", .{@import("build_options").version});
        return;
    }
    if (args.help or words.len == 0) {
        try out.writeAll(usage);
        return;
    }
    const command = words[0];
    for (args.flags.items) |flag| {
        if (eq(u8, flag, "--config-dir") or eq(u8, flag, "--verbose") or eq(u8, flag, "-v")) continue;
        const allowed = if (eq(u8, command, "sync")) !eq(u8, flag, "--remote") and !eq(u8, flag, "--machine") else if (eq(u8, command, "update")) eq(u8, flag, "--version") or eq(u8, flag, "--json") else if (eq(u8, command, "init")) eq(u8, flag, "--remote") or eq(u8, flag, "--machine") else if (eq(u8, command, "disable-builtin-sync")) eq(u8, flag, "--dry-run") else if (eq(u8, command, "plugins")) eq(u8, flag, "--ide") else false;
        if (!allowed) return error.OptionNotValidForCommand;
    }
    if (eq(u8, command, "completions")) {
        if (words.len != 2) return error.InvalidArguments;
        return completions(words[1], out);
    }
    if (eq(u8, command, "update")) {
        if (words.len != 1) return error.InvalidArguments;
        return @import("update.zig").run(a, io, args.update_version, args.json, out);
    }
    const known = [_][]const u8{ "init", "status", "sync", "ides", "repo", "plugins", "disable-builtin-sync" };
    var valid = false;
    for (known) |s| if (eq(u8, s, command)) {
        valid = true;
    };
    if (!valid) return error.UnknownCommand;
    if (!eq(u8, command, "repo") and !eq(u8, command, "plugins") and words.len != 1) return error.InvalidArguments;
    const fs: files.Fs = .{ .allocator = a, .io = io };
    const home = env.get("HOME") orelse return error.HomeNotFound;
    const root = args.root orelse env.get("JBSYNC_CONFIG_DIR") orelse try fs.join(&.{ home, ".jbsync" });
    if (eq(u8, command, "init")) {
        const path = try fs.join(&.{ root, "config.toml" });
        var local = try Engine.loadLocal(fs, (try fs.read(path)) orelse "", env, args.machine);
        if (args.remote) |s| local.remote = s;
        if (args.machine) |s| local.machine = try config.sanitize(a, s);
        const guard = try Engine.lockAt(fs, root);
        defer guard.close(io);
        try fs.write(path, try local.encode(a));
        var engine = try Engine.open(fs, root, env);
        try engine.git.initialize(true);
        try out.print("config  {s}\nstore   {s}\nmachine {s}\nfound   {d} IDE(s)\n\nNext: jbsync status\n", .{ path, engine.git.root, engine.local.machine, engine.ides.len });
        return;
    }
    var engine = try Engine.open(fs, root, env);
    if (eq(u8, command, "sync")) return engine.sync(args.options, out, args.verbose);
    if (eq(u8, command, "status")) {
        var options = args.options;
        options.dry_run = true;
        return engine.sync(options, out, args.verbose);
    }
    if (eq(u8, command, "ides")) {
        if (engine.ides.len == 0) try out.writeAll("No JetBrains IDEs found.\n");
        for (engine.ides) |ide| if (@import("settings.zig").any(engine.policy.ides, ide.name)) try out.print("{s}\t{s}\tbuild {s}{s}\n", .{ ide.name, ide.product, ide.metadata.build, if (ide.launched) "" else " (never launched)" });
        return;
    }
    if (eq(u8, command, "disable-builtin-sync")) return engine.disable(args.options.dry_run, out);
    if (eq(u8, command, "repo")) {
        if (words.len < 2) return error.InvalidArguments;
        if (eq(u8, words[1], "show") and words.len == 2) {
            try out.print("store   {s}\nbackend git\nremote  {s}\nbranch  {s}\n", .{ engine.git.root, engine.local.remote orelse "(local only)", engine.local.branch });
            return;
        }
        if (eq(u8, words[1], "set") and words.len == 3) engine.local.remote = words[2] else if (eq(u8, words[1], "unset") and words.len == 2) engine.local.remote = null else return error.InvalidArguments;
        const guard = try engine.lock();
        defer guard.close(io);
        engine.git.remote = engine.local.remote;
        try engine.git.initialize(true);
        try fs.write(try fs.join(&.{ root, "config.toml" }), try engine.local.encode(a));
        try out.print("Store remote: {s}\n", .{engine.local.remote orelse "(local only)"});
        return;
    }
    if (eq(u8, command, "plugins")) {
        const path = try fs.join(&.{ engine.git.root, "sync.toml" });
        if (words.len == 1) {
            var stage: files.Staging = .{ .fs = fs };
            const actions = try plugins.reconcile(fs, &stage, engine.git.root, engine.ides, engine.policy, .{ .dry_run = true }, out);
            _ = actions;
            const raw = (try fs.read(try fs.join(&.{ engine.git.root, "plugins.json" }))) orelse "{\"plugins\":[]}";
            const manifest = (try std.json.parseFromSlice(plugins.Manifest, a, raw, .{ .ignore_unknown_fields = true })).value;
            if (manifest.plugins.len == 0) try out.writeAll("No plugins recorded yet. Run `jbsync sync` first.\n");
            for (manifest.plugins) |p| try out.print("{s}\t{s}\n", .{ p.id, p.version });
            return;
        }
        if (words.len != 3 or (!eq(u8, words[1], "only") and !eq(u8, words[1], "allow") and !eq(u8, words[1], "deny"))) return error.InvalidArguments;
        if (args.options.only.len > 1 or (eq(u8, words[1], "only") and args.options.only.len == 0)) return error.InvalidArguments;
        const pattern = if (args.options.only.len != 0) args.options.only[0] else "*";
        const guard = try engine.lock();
        defer guard.close(io);
        const raw = (try fs.read(path)) orelse "";
        const doc = try config.parse(a, raw);
        const policy = try config.table(doc, "plugins");
        for (try config.array(policy, "rule")) |r| {
            if (eq(u8, try config.string(r, "id", ""), words[2]) and eq(u8, try config.string(r, "ide", "*"), pattern) and eq(u8, try config.string(r, "action", ""), words[1])) {
                try out.writeAll("Already set.\n");
                return;
            }
        }
        const updated = try std.fmt.allocPrint(a, "{s}\n[[plugins.rule]]\nid = {s}\nide = {s}\naction = {s}\n", .{ raw, try config.quote(a, words[2]), try config.quote(a, pattern), try config.quote(a, words[1]) });
        try fs.write(path, updated);
        try out.print("Written to {s}; run `jbsync sync` to commit it.\n", .{path});
        return;
    }
}
fn completions(shell: []const u8, out: *std.Io.Writer) !void {
    const commands = "init status sync ides repo plugins disable-builtin-sync update completions";
    if (eq(u8, shell, "bash")) try out.print("complete -W '{s}' jbsync\n", .{commands}) else if (eq(u8, shell, "zsh")) try out.print("#compdef jbsync\n_arguments '1:command:({s})'\n", .{commands}) else if (eq(u8, shell, "fish")) try out.print("complete -c jbsync -f -a '{s}'\n", .{commands}) else if (eq(u8, shell, "powershell")) try out.print("Register-ArgumentCompleter -Native -CommandName jbsync -ScriptBlock {{ param($word) '{s}'.Split(' ') | Where-Object {{ $_ -like \"$word*\" }} }}\n", .{commands}) else if (eq(u8, shell, "elvish")) try out.print("set edit:completion:arg-completer[jbsync] = {{|@args| put {s} }}\n", .{commands}) else return error.UnsupportedShell;
}
test "CLI policies, repeated IDE selectors and invalid arguments" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const args = try parseArgs(a, &.{ "jbsync", "sync", "--ide", "CLion*", "--ide=PyCharm*", "--prefer", "neither", "--dry-run" });
    try std.testing.expectEqual(@as(usize, 2), args.options.only.len);
    try std.testing.expect(args.options.dry_run);
    try std.testing.expectEqual(@import("merge.zig").Policy.neither, args.options.policy);
    try std.testing.expectError(error.UnknownOption, parseArgs(a, &.{ "jbsync", "sync", "--dr-run" }));
    try std.testing.expectError(error.MissingOptionValue, parseArgs(a, &.{ "jbsync", "sync", "--prefer" }));
}
