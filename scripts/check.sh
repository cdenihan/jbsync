#!/bin/sh
# Complete disposable validation. The binary never sees real IDE settings.
set -eu
mode=${1:-ReleaseSafe}
zig fmt --check build.zig build.zig.zon src
zig build test --summary all -Dtarget=aarch64-macos -Doptimize="$mode"
zig build -Dtarget=aarch64-macos -Doptimize="$mode"
zig build validation -Dtarget=aarch64-macos -Doptimize="$mode"
python3 tests/zig/integration.py zig-out/bin/jbsync
python3 tests/zig/regression.py zig-out/bin/jbsync-validation
python3 tests/zig/corpus.py zig-out/bin/jbsync-validation tests/corpus
python3 tests/zig/migration.py zig-out/bin/jbsync
python3 tests/zig/installer.py zig-out/bin/jbsync
python3 tests/zig/release.py
sh tests/zig/platform.sh
