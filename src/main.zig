const std = @import("std");
const jbsync = @import("jbsync");

pub fn main(init: std.process.Init) void {
    const arena = init.arena.allocator();
    const args = init.minimal.args.toSlice(arena) catch {
        std.process.exit(1);
    };
    var buffer: [4096]u8 = undefined;
    var output = std.Io.File.stdout().writer(init.io, &buffer);
    jbsync.cli.run(arena, init.io, init.environ_map, args, &output.interface) catch |err| {
        output.interface.flush() catch {};
        var error_buffer: [1024]u8 = undefined;
        var errors = std.Io.File.stderr().writer(init.io, &error_buffer);
        errors.interface.print("jbsync: {s}\n", .{@errorName(err)}) catch {};
        errors.interface.flush() catch {};
        std.process.exit(1);
    };
    output.interface.flush() catch std.process.exit(1);
}
