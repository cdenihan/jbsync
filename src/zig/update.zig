//! Self-update shares the checksum-verified, atomic release installer.
const std = @import("std");
const installer = @import("build_options").installer;
pub fn run(a: std.mem.Allocator, io: std.Io, requested: []const u8, json: bool, out: *std.Io.Writer) !void {
    const executable = try std.process.executablePathAlloc(io, a);
    if (!std.mem.eql(u8, std.fs.path.basename(executable), "jbsync")) return error.UpdateRequiresExecutableNamedJbsync;
    const result = try std.process.run(a, io, .{ .argv = &.{ "/bin/sh", "-c", "unset JBSYNC_INSTALLER_SOURCE_ONLY\n" ++ installer, "jbsync-update", "--version", requested, "--install-dir", std.fs.path.dirname(executable) orelse return error.InvalidExecutablePath }, .stdout_limit = .limited(1024 * 1024), .stderr_limit = .limited(1024 * 1024) });
    if (result.term != .exited or result.term.exited != 0) {
        try std.Io.File.stderr().writeStreamingAll(io, result.stderr);
        return error.UpdateFailed;
    }
    const installed = try std.process.run(a, io, .{ .argv = &.{ executable, "--version" }, .stdout_limit = .limited(1024), .stderr_limit = .limited(1024) });
    if (installed.term != .exited or installed.term.exited != 0) return error.UpdatedExecutableFailed;
    const reported = std.mem.trim(u8, installed.stdout, "\r\n ");
    if (!std.mem.startsWith(u8, reported, "jbsync ")) return error.InvalidReleaseVersion;
    const version = reported[7..];
    const previous = @import("build_options").version;
    const status = if (std.mem.eql(u8, version, previous)) "current" else "updated";
    if (json) {
        try std.json.Stringify.value(.{ .previous_version = previous, .installed_version = version, .executable = executable, .status = status }, .{}, out);
        try out.writeByte('\n');
    } else try out.print("jbsync {s} -> {s} at {s} ({s})\n", .{ previous, version, executable, status });
}
