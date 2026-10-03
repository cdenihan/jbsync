const std = @import("std");

pub fn build(b: *std.Build) void {
    if (@import("builtin").zig_version.major != 0 or @import("builtin").zig_version.minor != 17)
        @compileError("This experiment requires Zig 0.17.x");
    const target = b.standardTargetOptions(.{});
    if (target.result.os.tag != .macos or target.result.cpu.arch != .aarch64)
        @panic("jbsync supports only Apple Silicon Macs (aarch64-macos)");
    const optimize = b.standardOptimizeOption(.{});

    const mod = b.addModule("jbsync", .{
        .root_source_file = b.path("src/root.zig"),
        .target = target,
        .optimize = optimize,
    });

    const exe = b.addExecutable(.{
        .name = "jbsync",
        .root_module = b.createModule(.{
            .root_source_file = b.path("src/main.zig"),
            .target = target,
            .optimize = optimize,
            .imports = &.{
                .{ .name = "jbsync", .module = mod },
            },
        }),
    });

    const options = b.addOptions();
    options.addOption([]const u8, "version", std.mem.trim(u8, @embedFile("VERSION"), "\r\n "));
    mod.addOptions("build_options", options);
    b.installArtifact(exe);

    const validator = b.addExecutable(.{
        .name = "jbsync-validation",
        .root_module = b.createModule(.{
            .root_source_file = b.path("src/validation.zig"),
            .target = target,
            .optimize = optimize,
            .imports = &.{.{ .name = "jbsync", .module = mod }},
        }),
    });
    const validation = b.step("validation", "Build the Rust differential test helper");
    validation.dependOn(&b.addInstallArtifact(validator, .{}).step);

    const run_step = b.step("run", "Run the app");

    const run_cmd = b.addRunArtifact(exe);
    run_step.dependOn(&run_cmd.step);

    run_cmd.step.dependOn(b.getInstallStep());

    run_cmd.addPassthruArgs();

    const mod_tests = b.addTest(.{
        .root_module = mod,
    });

    const run_mod_tests = b.addRunArtifact(mod_tests);

    const exe_tests = b.addTest(.{
        .root_module = exe.root_module,
    });

    const run_exe_tests = b.addRunArtifact(exe_tests);

    const test_step = b.step("test", "Run tests");
    test_step.dependOn(&run_mod_tests.step);
    test_step.dependOn(&run_exe_tests.step);
}
