//! Configuration TOML subset used by jbsync: tables, arrays of tables,
//! strings, booleans, integers, arrays and inline tables. Unsupported value
//! types fail explicitly rather than silently changing sync policy.
const std = @import("std");
const A = std.mem.Allocator;
const eq = std.mem.eql;
pub const Table = std.StringHashMapUnmanaged(Value);
pub const Value = union(enum) {
    string: []const u8,
    boolean: bool,
    integer: i64,
    array: std.ArrayList(Value),
    table: Table,
    pub fn get(self: Value, key: []const u8) ?Value {
        return if (self == .table) self.table.get(key) else null;
    }
    pub fn str(self: Value) ![]const u8 {
        return if (self == .string) self.string else error.InvalidConfigType;
    }
    pub fn booleanValue(self: Value) !bool {
        return if (self == .boolean) self.boolean else error.InvalidConfigType;
    }
};
pub fn string(t: Value, k: []const u8, default: []const u8) ![]const u8 {
    return if (t.get(k)) |v| v.str() else default;
}
pub fn boolean(t: Value, k: []const u8, default: bool) !bool {
    return if (t.get(k)) |v| v.booleanValue() else default;
}
pub fn strings(a: A, t: Value, k: []const u8, default: []const []const u8) ![]const []const u8 {
    const v = t.get(k) orelse return default;
    if (v != .array) return error.InvalidConfigType;
    const result = try a.alloc([]const u8, v.array.items.len);
    for (v.array.items, result) |item, *out| out.* = try item.str();
    return result;
}
pub fn table(t: Value, k: []const u8) !Value {
    const v = t.get(k) orelse return .{ .table = .empty };
    if (v != .table) return error.InvalidConfigType;
    return v;
}
pub fn array(t: Value, k: []const u8) ![]Value {
    const v = t.get(k) orelse return &.{};
    if (v != .array) return error.InvalidConfigType;
    return v.array.items;
}
const Parser = struct {
    a: A,
    source: []const u8,
    pos: usize = 0,
    fn ws(p: *Parser, multiline: bool) void {
        while (p.pos < p.source.len) {
            const c = p.source[p.pos];
            if (c == ' ' or c == '\t' or c == '\r' or (multiline and c == '\n')) {
                p.pos += 1;
                continue;
            }
            if (c == '#') {
                while (p.pos < p.source.len and p.source[p.pos] != '\n') p.pos += 1;
                continue;
            }
            break;
        }
    }
    fn take(p: *Parser, c: u8) !void {
        if (p.pos >= p.source.len or p.source[p.pos] != c) return error.InvalidToml;
        p.pos += 1;
    }
    fn quoted(p: *Parser) ![]const u8 {
        const delimiter = p.source[p.pos];
        p.pos += 1;
        var out: std.ArrayList(u8) = .empty;
        while (p.pos < p.source.len) {
            const c = p.source[p.pos];
            p.pos += 1;
            if (c == delimiter) return out.toOwnedSlice(p.a);
            if (c < 0x20 and c != '\t') return error.InvalidToml;
            if (c != '\\' or delimiter == '\'') {
                try out.append(p.a, c);
                continue;
            }
            if (p.pos >= p.source.len) return error.InvalidToml;
            const escaped = p.source[p.pos];
            p.pos += 1;
            switch (escaped) {
                'n' => try out.append(p.a, '\n'),
                'r' => try out.append(p.a, '\r'),
                't' => try out.append(p.a, '\t'),
                'b' => try out.append(p.a, 8),
                'f' => try out.append(p.a, 12),
                '"', '\\' => try out.append(p.a, escaped),
                'u', 'U' => {
                    const len: usize = if (escaped == 'u') 4 else 8;
                    if (p.pos + len > p.source.len) return error.InvalidToml;
                    const cp = std.fmt.parseInt(u21, p.source[p.pos..][0..len], 16) catch return error.InvalidToml;
                    p.pos += len;
                    var encoded: [4]u8 = undefined;
                    const n = std.unicode.utf8Encode(cp, &encoded) catch return error.InvalidToml;
                    try out.appendSlice(p.a, encoded[0..n]);
                },
                else => return error.InvalidToml,
            }
        }
        return error.InvalidToml;
    }
    fn key(p: *Parser) ![]const u8 {
        if (p.pos >= p.source.len) return error.InvalidToml;
        if (p.source[p.pos] == '"' or p.source[p.pos] == '\'') return p.quoted();
        const start = p.pos;
        while (p.pos < p.source.len and (std.ascii.isAlphanumeric(p.source[p.pos]) or p.source[p.pos] == '_' or p.source[p.pos] == '-')) p.pos += 1;
        if (p.pos == start) return error.InvalidToml;
        return p.source[start..p.pos];
    }
    fn path(p: *Parser) ![][]const u8 {
        var result: std.ArrayList([]const u8) = .empty;
        while (true) {
            p.ws(false);
            try result.append(p.a, try p.key());
            p.ws(false);
            if (p.pos >= p.source.len or p.source[p.pos] != '.') break;
            p.pos += 1;
        }
        return result.toOwnedSlice(p.a);
    }
    fn value(p: *Parser, depth: usize) anyerror!Value {
        if (depth > 64 or p.pos >= p.source.len) return error.InvalidToml;
        switch (p.source[p.pos]) {
            '"', '\'' => return .{ .string = try p.quoted() },
            '[' => {
                p.pos += 1;
                var values: std.ArrayList(Value) = .empty;
                while (true) {
                    p.ws(true);
                    if (p.pos >= p.source.len) return error.InvalidToml;
                    if (p.source[p.pos] == ']') {
                        p.pos += 1;
                        break;
                    }
                    try values.append(p.a, try p.value(depth + 1));
                    p.ws(true);
                    if (p.pos < p.source.len and p.source[p.pos] == ']') {
                        p.pos += 1;
                        break;
                    }
                    try p.take(',');
                }
                return .{ .array = values };
            },
            '{' => {
                p.pos += 1;
                var values: Value = .{ .table = .empty };
                while (true) {
                    p.ws(false);
                    if (p.pos >= p.source.len) return error.InvalidToml;
                    if (p.source[p.pos] == '}') {
                        p.pos += 1;
                        break;
                    }
                    const path_parts = try p.path();
                    try p.take('=');
                    p.ws(false);
                    try put(p.a, &values, path_parts, try p.value(depth + 1));
                    p.ws(false);
                    if (p.pos < p.source.len and p.source[p.pos] == '}') {
                        p.pos += 1;
                        break;
                    }
                    try p.take(',');
                }
                return values;
            },
            else => {
                const start = p.pos;
                while (p.pos < p.source.len and std.mem.indexOfScalar(u8, " \t\r\n,]}#", p.source[p.pos]) == null) p.pos += 1;
                const token = p.source[start..p.pos];
                if (eq(u8, token, "true")) return .{ .boolean = true };
                if (eq(u8, token, "false")) return .{ .boolean = false };
                return .{ .integer = std.fmt.parseInt(i64, token, 0) catch return error.UnsupportedTomlValue };
            },
        }
    }
};
fn child(a: A, current: *Value, key: []const u8) !*Value {
    if (current.* != .table) return error.InvalidToml;
    const entry = try current.table.getOrPut(a, key);
    if (!entry.found_existing) entry.value_ptr.* = .{ .table = .empty };
    if (entry.value_ptr.* == .array) {
        const items = entry.value_ptr.array.items;
        if (items.len == 0) return error.InvalidToml;
        return &items[items.len - 1];
    }
    if (entry.value_ptr.* != .table) return error.InvalidToml;
    return entry.value_ptr;
}
fn put(a: A, current: *Value, path: []const []const u8, v: Value) !void {
    var cursor = current;
    for (path[0 .. path.len - 1]) |part| cursor = try child(a, cursor, part);
    if (cursor.* != .table) return error.InvalidToml;
    const entry = try cursor.table.getOrPut(a, path[path.len - 1]);
    if (entry.found_existing) return error.DuplicateTomlKey;
    entry.value_ptr.* = v;
}
pub fn parse(a: A, source: []const u8) !Value {
    if (!std.unicode.utf8ValidateSlice(source)) return error.InvalidToml;
    var root: Value = .{ .table = .empty };
    var current = &root;
    var p: Parser = .{ .a = a, .source = source };
    while (true) {
        p.ws(true);
        if (p.pos == source.len) break;
        if (source[p.pos] == '[') {
            p.pos += 1;
            const many = p.pos < source.len and source[p.pos] == '[';
            if (many) p.pos += 1;
            const parts = try p.path();
            try p.take(']');
            if (many) try p.take(']');
            current = &root;
            for (parts[0 .. parts.len - 1]) |part| current = try child(a, current, part);
            const last = parts[parts.len - 1];
            if (many) {
                if (current.* != .table) return error.InvalidToml;
                const entry = try current.table.getOrPut(a, last);
                if (!entry.found_existing) entry.value_ptr.* = .{ .array = .empty };
                if (entry.value_ptr.* != .array) return error.InvalidToml;
                try entry.value_ptr.array.append(a, .{ .table = .empty });
                current = &entry.value_ptr.array.items[entry.value_ptr.array.items.len - 1];
            } else current = try child(a, current, last);
        } else {
            const parts = try p.path();
            try p.take('=');
            p.ws(false);
            try put(a, current, parts, try p.value(0));
        }
        p.ws(false);
        if (p.pos < source.len) try p.take('\n');
    }
    return root;
}
pub fn quote(a: A, text: []const u8) ![]const u8 {
    var out: std.Io.Writer.Allocating = .init(a);
    try out.writer.writeByte('"');
    for (text) |ch| switch (ch) {
        '\\', '"' => {
            try out.writer.writeByte('\\');
            try out.writer.writeByte(ch);
        },
        '\n' => try out.writer.writeAll("\\n"),
        '\r' => try out.writer.writeAll("\\r"),
        '\t' => try out.writer.writeAll("\\t"),
        else => if (ch < 0x20) try out.writer.print("\\u{X:0>4}", .{@as(u16, ch)}) else try out.writer.writeByte(ch),
    };
    try out.writer.writeByte('"');
    return out.toOwnedSlice();
}
pub fn sanitize(a: A, input: []const u8) ![]const u8 {
    const s = try a.dupe(u8, input);
    for (s) |*ch| if (!std.ascii.isAlphanumeric(ch.*) and ch.* != '.' and ch.* != '_' and ch.* != '-') {
        ch.* = '-';
    };
    const trimmed = std.mem.trim(u8, s, "-");
    if (trimmed.len == 0 or eq(u8, trimmed, ".") or eq(u8, trimmed, "..")) return "machine";
    return trimmed;
}
pub const Local = struct {
    remote: ?[]const u8 = null,
    branch: []const u8 = "main",
    store: ?[]const u8 = null,
    root: ?[]const u8 = null,
    install_roots: []const []const u8 = &.{},
    machine: []const u8 = "machine",
    pub fn load(a: A, raw: []const u8, hostname: []const u8) !Local {
        const doc = try parse(a, raw);
        const repo = try table(doc, "repo");
        const jb = try table(doc, "jetbrains");
        const m = try table(doc, "machine");
        if (!eq(u8, try string(repo, "backend", "git"), "git")) return error.UnsupportedBackend;
        return .{
            .remote = if (repo.get("remote")) |v| try v.str() else null,
            .branch = try string(repo, "branch", "main"),
            .store = if (repo.get("path")) |v| try v.str() else null,
            .root = if (jb.get("root")) |v| try v.str() else null,
            .install_roots = try strings(a, jb, "install_roots", &.{}),
            .machine = try sanitize(a, try string(m, "id", hostname)),
        };
    }
    pub fn encode(self: Local, a: A) ![]const u8 {
        var out: std.Io.Writer.Allocating = .init(a);
        try out.writer.print("[repo]\nbackend = \"git\"\nbranch = {s}\n", .{try quote(a, self.branch)});
        if (self.remote) |s| try out.writer.print("remote = {s}\n", .{try quote(a, s)});
        if (self.store) |s| try out.writer.print("path = {s}\n", .{try quote(a, s)});
        try out.writer.writeAll("\n[jetbrains]\n");
        if (self.root) |s| try out.writer.print("root = {s}\n", .{try quote(a, s)});
        try out.writer.writeAll("install_roots = [");
        for (self.install_roots, 0..) |s, i| {
            if (i != 0) try out.writer.writeAll(", ");
            try out.writer.writeAll(try quote(a, s));
        }
        try out.writer.print("]\n\n[machine]\nid = {s}\n", .{try quote(a, self.machine)});
        return out.toOwnedSlice();
    }
};
pub const Rule = struct { file: []const u8, component: ?[]const u8 = null, element: []const u8 = "option", option: ?[]const u8 = null, attribute: ?[]const u8 = null, equals: []const u8 = "" };
pub const Sync = struct {
    ides: []const []const u8 = &.{"*20??.*"},
    backups: bool = true,
    default_excludes: bool = true,
    use_defaults: bool = true,
    plugins: bool = true,
    include: []const []const u8 = &.{},
    exclude: []const []const u8 = &.{},
    explicit: []const []const u8 = &.{},
    rules: []const Rule = &.{},
    plugin_config: Value = .{ .table = .empty },
    vm_names: Value = .{ .table = .empty },
    pub fn load(a: A, raw: []const u8, machine_raw: []const u8) !Sync {
        const doc = try parse(a, raw);
        if (doc.get("version")) |v| if (v != .integer or v.integer != 1) return error.UnsupportedConfigVersion;
        const machine = try parse(a, machine_raw);
        const jb = try table(doc, "jetbrains");
        const x = try table(doc, "xml");
        const p = try table(doc, "plugins");
        var rules: std.ArrayList(Rule) = .empty;
        for ([_]Value{ x, try table(machine, "xml") }) |layer| for (try array(layer, "omit")) |r| {
            try rules.append(a, .{ .file = try string(r, "file", "*"), .element = try string(r, "element", "option"), .component = if (r.get("component")) |v| try v.str() else null, .option = if (r.get("option")) |v| try v.str() else null, .attribute = if (r.get("attribute")) |v| try v.str() else null, .equals = try string(r, "equals", "") });
        };
        var excludes: std.ArrayList([]const u8) = .empty;
        try excludes.appendSlice(a, try strings(a, jb, "exclude", &.{}));
        try excludes.appendSlice(a, try strings(a, try table(machine, "jetbrains"), "exclude", &.{}));
        return .{ .ides = try strings(a, jb, "ides", &.{"*20??.*"}), .backups = try boolean(jb, "backups", true), .default_excludes = try boolean(jb, "use_default_excludes", true), .use_defaults = try boolean(x, "use_defaults", true), .plugins = try boolean(p, "enabled", true), .include = try strings(a, jb, "include", &.{}), .exclude = excludes.items, .explicit = try strings(a, jb, "explicit_include", &.{}), .rules = rules.items, .plugin_config = p, .vm_names = try table(jb, "vmoptions_names") };
    }
};
test "comments, multiline arrays, inline maps and omit tables" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const s = try Sync.load(a, "[jetbrains]\nides = [\n'CLion*', # comment\n'PyCharm*',\n]\nvmoptions_names = { CLion = 'clion.vmoptions' }\n[[xml.omit]]\nfile = 'options/editor.xml'\noption = 'x'\n", "[jetbrains]\nexclude=['options/laf.xml']\n");
    try std.testing.expectEqual(@as(usize, 2), s.ides.len);
    try std.testing.expectEqualStrings("clion.vmoptions", try s.vm_names.get("CLion").?.str());
    try std.testing.expectEqual(@as(usize, 1), s.rules.len);
    try std.testing.expectEqualStrings("options/laf.xml", s.exclude[0]);
    try std.testing.expectError(error.DuplicateTomlKey, parse(a, "x=1\nx=2\n"));
    try std.testing.expectError(error.InvalidConfigType, Sync.load(a, "[jetbrains]\nbackups='false'", ""));
}
