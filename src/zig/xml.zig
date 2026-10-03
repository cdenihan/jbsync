//! Small ordered XML DOM for JetBrains' settings dialect. No global allocator.
//! The caller owns the arena holding nodes, decoded strings and projections.
const std = @import("std");
const A = std.mem.Allocator;
const eq = std.mem.eql;
pub const View = std.StringHashMapUnmanaged([]const u8);
const keys = [_][]const u8{ "name", "key", "id", "class", "type", "language", "scheme", "ext", "pattern" };
pub const Node = struct {
    name: []const u8,
    attrs: View = .empty,
    children: std.ArrayList(*Node) = .empty,
    text: ?[]const u8 = null,
    pub fn create(a: A, name: []const u8) !*Node {
        const n = try a.create(Node);
        n.* = .{ .name = name };
        return n;
    }
    pub fn key(self: *const Node) ?[]const u8 {
        for (keys) |k| if (self.attrs.contains(k)) return k;
        return null;
    }
    pub fn shell(self: *const Node, a: A) !*Node {
        const n = try create(a, self.name);
        var it = self.attrs.iterator();
        while (it.next()) |entry| try n.attrs.put(a, entry.key_ptr.*, entry.value_ptr.*);
        return n;
    }
    pub fn clone(self: *const Node, a: A) A.Error!*Node {
        const n = try self.shell(a);
        n.text = self.text;
        for (self.children.items) |child| try n.children.append(a, try child.clone(a));
        return n;
    }
    pub fn empty(self: *const Node) bool {
        if (self.children.items.len != 0 or self.text != null) return false;
        if (self.attrs.count() == 0) return true;
        if (eq(u8, self.name, "component")) {
            var it = self.attrs.keyIterator();
            while (it.next()) |k| if (!eq(u8, k.*, "name") and !eq(u8, k.*, "version")) return false;
            return true;
        }
        for ([_][]const u8{ "option", "entry", "map", "list", "set" }) |wrapper| {
            if (!eq(u8, wrapper, self.name)) continue;
            var it = self.attrs.keyIterator();
            while (it.next()) |k| {
                var is_key = false;
                for (keys) |v| {
                    if (eq(u8, v, k.*)) is_key = true;
                }
                if (!is_key) return false;
            }
            return true;
        }
        return false;
    }
};

const Parser = struct {
    a: A,
    input: []const u8,
    pos: usize = 0,
    fn starts(p: *Parser, s: []const u8) bool {
        return std.mem.startsWith(u8, p.input[p.pos..], s);
    }
    fn ws(p: *Parser) void {
        while (p.pos < p.input.len and std.ascii.isWhitespace(p.input[p.pos])) p.pos += 1;
    }
    fn take(p: *Parser, s: []const u8) !void {
        if (!p.starts(s)) return error.InvalidXml;
        p.pos += s.len;
    }
    fn until(p: *Parser, end: []const u8) ![]const u8 {
        const offset = std.mem.indexOf(u8, p.input[p.pos..], end) orelse return error.InvalidXml;
        const value = p.input[p.pos..][0..offset];
        p.pos += offset + end.len;
        return value;
    }
    fn name(p: *Parser) ![]const u8 {
        const start = p.pos;
        while (p.pos < p.input.len) : (p.pos += 1) {
            const ch = p.input[p.pos];
            if (!std.ascii.isAlphanumeric(ch) and ch != '_' and ch != '-' and ch != '.' and ch != ':' and ch < 0x80) break;
        }
        if (p.pos == start or std.ascii.isDigit(p.input[start])) return error.InvalidXml;
        return p.input[start..p.pos];
    }
    fn misc(p: *Parser) !void {
        while (true) {
            p.ws();
            if (p.starts("<!--")) {
                p.pos += 4;
                const comment = try p.until("-->");
                if (std.mem.indexOf(u8, comment, "--") != null) return error.InvalidXml;
            } else if (p.starts("<?")) {
                p.pos += 2;
                _ = try p.until("?>");
            } else break;
        }
    }
    fn node(p: *Parser, depth: usize) anyerror!*Node {
        if (depth > 128) return error.XmlDepthExceeded;
        try p.take("<");
        const n = try Node.create(p.a, try p.name());
        while (true) {
            const before_ws = p.pos;
            p.ws();
            if (p.starts("/>")) {
                p.pos += 2;
                return n;
            }
            if (p.starts(">")) {
                p.pos += 1;
                break;
            }
            if (before_ws == p.pos) return error.InvalidXml;
            const k = try p.name();
            p.ws();
            try p.take("=");
            p.ws();
            if (p.pos >= p.input.len or (p.input[p.pos] != '\'' and p.input[p.pos] != '"')) return error.InvalidXml;
            const quote = p.input[p.pos];
            p.pos += 1;
            const start = p.pos;
            while (p.pos < p.input.len and p.input[p.pos] != quote) : (p.pos += 1) {}
            if (p.pos == p.input.len) return error.InvalidXml;
            const raw = p.input[start..p.pos];
            p.pos += 1;
            if (std.mem.indexOfScalar(u8, raw, '<') != null or n.attrs.contains(k)) return error.InvalidXml;
            try n.attrs.put(p.a, k, try decode(p.a, raw, true));
        }
        var text: std.ArrayList(u8) = .empty;
        while (true) {
            if (p.pos >= p.input.len) return error.InvalidXml;
            if (p.starts("</")) {
                p.pos += 2;
                const end = try p.name();
                p.ws();
                try p.take(">");
                if (!eq(u8, end, n.name)) return error.InvalidXml;
                break;
            }
            if (p.starts("<!--") or p.starts("<?")) {
                try p.misc();
                continue;
            }
            if (p.starts("<![CDATA[")) {
                p.pos += 9;
                try text.appendSlice(p.a, try p.until("]]>"));
                continue;
            }
            if (p.starts("<")) {
                try n.children.append(p.a, try p.node(depth + 1));
                continue;
            }
            const start = p.pos;
            while (p.pos < p.input.len and p.input[p.pos] != '<') : (p.pos += 1) {}
            try text.appendSlice(p.a, try decode(p.a, p.input[start..p.pos], false));
        }
        const trimmed = std.mem.trim(u8, text.items, " \r\n\t");
        if (trimmed.len != 0) {
            // Mixed content cannot be represented losslessly by this dialect.
            if (n.children.items.len != 0) return error.UnsupportedMixedXml;
            n.text = trimmed;
        }
        return n;
    }
};
pub fn parse(a: A, input: []const u8) !*Node {
    if (!std.unicode.utf8ValidateSlice(input)) return error.InvalidXml;
    for (input) |ch| if (ch < 0x20 and ch != '\t' and ch != '\n' and ch != '\r') return error.InvalidXml;
    var p: Parser = .{ .a = a, .input = input };
    if (p.starts("\xef\xbb\xbf")) p.pos += 3;
    try p.misc();
    const root = try p.node(0);
    try p.misc();
    if (p.pos != input.len) return error.InvalidXml;
    return root;
}
fn decode(a: A, raw: []const u8, attribute: bool) ![]const u8 {
    var out: std.ArrayList(u8) = .empty;
    var i: usize = 0;
    while (i < raw.len) : (i += 1) {
        const ch = raw[i];
        if (ch != '&') {
            try out.append(a, if (attribute and (ch == '\n' or ch == '\t' or ch == '\r')) ' ' else ch);
            continue;
        }
        const end = std.mem.indexOfScalarPos(u8, raw, i, ';') orelse return error.InvalidXml;
        const entity = raw[i + 1 .. end];
        const named = [_][]const u8{ "amp", "lt", "gt", "quot", "apos" };
        const values = "&<>\"'";
        var found = false;
        for (named, values) |name, value| if (eq(u8, name, entity)) {
            try out.append(a, value);
            found = true;
            break;
        };
        if (!found) {
            if (!std.mem.startsWith(u8, entity, "#")) return error.InvalidXml;
            const hex = std.mem.startsWith(u8, entity, "#x");
            const cp = std.fmt.parseInt(u21, entity[if (hex) @as(usize, 2) else 1..], if (hex) 16 else 10) catch return error.InvalidXml;
            if (cp == 0 or (cp < 0x20 and cp != 9 and cp != 10 and cp != 13) or cp == 0xfffe or cp == 0xffff) return error.InvalidXml;
            var encoded: [4]u8 = undefined;
            const len = std.unicode.utf8Encode(cp, &encoded) catch return error.InvalidXml;
            try out.appendSlice(a, encoded[0..len]);
        }
        i = end;
    }
    return out.toOwnedSlice(a);
}
fn escape(out: *std.Io.Writer, value: []const u8, attr: bool) !void {
    for (value) |ch| switch (ch) {
        '&' => try out.writeAll("&amp;"),
        '<' => try out.writeAll("&lt;"),
        '>' => try out.writeAll("&gt;"),
        '"' => if (attr) try out.writeAll("&quot;") else try out.writeByte(ch),
        '\n' => if (attr) try out.writeAll("&#10;") else try out.writeByte(ch),
        '\r' => if (attr) try out.writeAll("&#13;") else try out.writeByte(ch),
        '\t' => if (attr) try out.writeAll("&#9;") else try out.writeByte(ch),
        else => try out.writeByte(ch),
    };
}
fn writeNode(a: A, out: *std.Io.Writer, n: *const Node, depth: usize) anyerror!void {
    try out.splatByteAll(' ', depth * 2);
    try out.print("<{s}", .{n.name});
    const attrs = try a.alloc([]const u8, n.attrs.count());
    var it = n.attrs.keyIterator();
    var i: usize = 0;
    while (it.next()) |k| {
        attrs[i] = k.*;
        i += 1;
    }
    std.mem.sort([]const u8, attrs, {}, @import("files.zig").less);
    for (attrs) |k| {
        try out.print(" {s}=\"", .{k});
        try escape(out, n.attrs.get(k).?, true);
        try out.writeByte('"');
    }
    if (n.children.items.len == 0 and n.text == null) {
        try out.writeAll(" />\n");
        return;
    }
    try out.writeByte('>');
    if (n.text) |text| {
        try escape(out, text, false);
    } else {
        try out.writeByte('\n');
        for (n.children.items) |child| try writeNode(a, out, child, depth + 1);
        try out.splatByteAll(' ', depth * 2);
    }
    try out.print("</{s}>\n", .{n.name});
}
pub fn serialize(a: A, n: *const Node) ![]const u8 {
    var out: std.Io.Writer.Allocating = .init(a);
    try out.writer.writeAll("<?xml version='1.0' encoding='utf-8'?>\n");
    try writeNode(a, &out.writer, n, 0);
    return out.toOwnedSlice();
}
// Escape address delimiters so names containing '/' cannot alias another leaf.
fn addressValue(a: A, s: []const u8) A.Error![]const u8 {
    var out: std.ArrayList(u8) = .empty;
    const hex = "0123456789ABCDEF";
    for (s) |ch| {
        if (ch == '/' or ch == '[' or ch == ']' or ch == '%') {
            try out.appendSlice(a, &.{ '%', hex[ch >> 4], hex[ch & 15] });
        } else try out.append(a, ch);
    }
    return out.toOwnedSlice(a);
}

fn segment(a: A, parent: *const Node, index: usize) ![]const u8 {
    const n = parent.children.items[index];
    var counter: usize = 0;
    for (parent.children.items[0..index]) |previous| if (eq(u8, previous.name, n.name)) {
        counter += 1;
    };
    if (n.key()) |k| return std.fmt.allocPrint(a, "{s}[{s}={s}]", .{ n.name, k, try addressValue(a, n.attrs.get(k).?) });
    return std.fmt.allocPrint(a, "{s}#{d}", .{ n.name, counter });
}
fn joined(a: A, prefix: []const u8, leaf: []const u8) ![]const u8 {
    return if (prefix.len == 0) leaf else std.fmt.allocPrint(a, "{s}/{s}", .{ prefix, leaf });
}
pub const Leaf = struct { path: []const u8, value: []const u8 };
fn collect(a: A, n: *const Node, prefix: []const u8, out: *std.ArrayList(Leaf)) A.Error!void {
    const key = n.key();
    var it = n.attrs.iterator();
    var payload = false;
    while (it.next()) |entry| {
        if (key != null and eq(u8, key.?, entry.key_ptr.*)) continue;
        try out.append(a, .{ .path = try joined(a, prefix, try std.fmt.allocPrint(a, "@{s}", .{entry.key_ptr.*})), .value = entry.value_ptr.* });
        payload = true;
    }
    if (n.text) |text| {
        try out.append(a, .{ .path = try joined(a, prefix, "#text"), .value = text });
        payload = true;
    }
    if (prefix.len != 0 and !payload and n.children.items.len == 0 and !n.empty()) try out.append(a, .{ .path = try joined(a, prefix, "#present"), .value = "" });
    for (n.children.items, 0..) |child, index| try collect(a, child, try joined(a, prefix, try segment(a, n, index)), out);
}
pub fn ordered(a: A, n: *const Node) ![]Leaf {
    var leaves: std.ArrayList(Leaf) = .empty;
    try collect(a, n, "", &leaves);
    return leaves.toOwnedSlice(a);
}
pub fn project(a: A, n: *const Node) !View {
    var out: View = .empty;
    for (try ordered(a, n)) |leaf| {
        if (out.contains(leaf.path)) return error.AmbiguousXmlAddress;
        try out.put(a, leaf.path, leaf.value);
    }
    return out;
}
/// Traverse the donor in document order so a new keyless item #10 is never
/// inserted before #2. Remove in reverse target order to keep indices stable.
pub fn patch(a: A, target: *Node, donor: *const Node, updates: View, removals: []const []const u8) !void {
    for (try ordered(a, donor)) |leaf| if (updates.get(leaf.path)) |value| try set(a, target, donor, leaf.path, value);
    const target_leaves = try ordered(a, target);
    var i = target_leaves.len;
    while (i > 0) {
        i -= 1;
        for (removals) |path| if (eq(u8, path, target_leaves[i].path)) {
            try remove(a, target, path);
            break;
        };
    }
}

fn find(a: A, parent: *const Node, wanted: []const u8) !?usize {
    for (parent.children.items, 0..) |_, i| if (eq(u8, try segment(a, parent, i), wanted)) return i;
    return null;
}
pub fn set(a: A, target: *Node, donor: *const Node, path: []const u8, value: []const u8) !void {
    var parts = std.mem.splitScalar(u8, path, '/');
    var cursor = target;
    var source = donor;
    while (parts.next()) |part| {
        if (parts.peek() == null) {
            if (eq(u8, part, "#text")) cursor.text = value else if (std.mem.startsWith(u8, part, "@")) try cursor.attrs.put(a, part[1..], value) else if (!eq(u8, part, "#present")) return error.InvalidXmlAddress;
            return;
        }
        const si = try find(a, source, part) orelse return error.InvalidXmlAddress;
        source = source.children.items[si];
        if (try find(a, cursor, part)) |ti| cursor = cursor.children.items[ti] else {
            const child = try source.shell(a);
            try cursor.children.append(a, child);
            cursor = child;
        }
    }
}
pub fn remove(a: A, target: *Node, path: []const u8) A.Error!void {
    const slash = std.mem.indexOfScalar(u8, path, '/');
    if (slash) |i| {
        const index = try find(a, target, path[0..i]) orelse return;
        if (eq(u8, path[i + 1 ..], "#present")) {
            _ = target.children.orderedRemove(index);
            return;
        }
        try remove(a, target.children.items[index], path[i + 1 ..]);
    } else if (eq(u8, path, "#text")) target.text = null else if (std.mem.startsWith(u8, path, "@")) {
        _ = target.attrs.remove(path[1..]);
    }
}
pub fn pruneEmpty(n: *Node) void {
    var i: usize = 0;
    while (i < n.children.items.len) {
        pruneEmpty(n.children.items[i]);
        if (n.children.items[i].empty()) _ = n.children.orderedRemove(i) else i += 1;
    }
}
pub fn graft(a: A, target: *Node, donor: *const Node) A.Error!void {
    var it = donor.attrs.iterator();
    while (it.next()) |entry| try target.attrs.put(a, entry.key_ptr.*, entry.value_ptr.*);
    if (donor.text != null) target.text = donor.text;
    for (donor.children.items, 0..) |child, i| {
        if (try find(a, target, try segment(a, donor, i))) |index| try graft(a, target.children.items[index], child) else try target.children.append(a, try child.clone(a));
    }
}
test "canonicalization, entities and document order" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const source = "<root z='a&#10;b' a='&amp;'><list><item value='z'/><item value='a'/></list><text><![CDATA[x<&]]></text></root>";
    const n = try parse(a, source);
    const encoded = try serialize(a, n);
    try std.testing.expect(std.mem.indexOf(u8, encoded, "a=\"&amp;\" z=\"a&#10;b\"") != null);
    try std.testing.expectEqualStrings(encoded, try serialize(a, try parse(a, encoded)));
    try std.testing.expectError(error.InvalidXml, parse(a, "<a><b></a>"));
    try std.testing.expectError(error.InvalidXml, parse(a, "<a x='1' x='2'/>"));
    try std.testing.expectError(error.UnsupportedMixedXml, parse(a, "<a>x<b/>y</a>"));
}
test "projection addresses escape delimiters and preserve domain presence" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const n = try parse(a, "<application><component name='A/B'><option name='x[y]' value='3'/><global_color_scheme name='Dark'/></component></application>");
    const view = try project(a, n);
    try std.testing.expectEqualStrings("3", view.get("component[name=A%2FB]/option[name=x%5By%5D]/@value").?);
    try std.testing.expect(view.contains("component[name=A%2FB]/global_color_scheme[name=Dark]/#present"));
}
