#!/bin/sh
# Build guards and installer dispatch must agree with the support policy.
set -eu
export JBSYNC_INSTALLER_SOURCE_ONLY=1
. ./scripts/install.sh
test "$(artifact_for Darwin aarch64)" = jbsync-macos-aarch64
for platform in 'Darwin x86_64' 'Linux aarch64' 'Linux x86_64' 'Windows aarch64'; do
    if artifact_for $platform >/dev/null; then
        echo "installer accepted unsupported platform: $platform" >&2
        exit 1
    fi
done
for target in x86_64-macos aarch64-linux-musl x86_64-windows; do
    log=$(mktemp)
    if zig build -Dtarget="$target" >"$log" 2>&1; then
        rm "$log"
        echo "build accepted unsupported target: $target" >&2
        exit 1
    fi
    grep -q 'only Apple Silicon Macs' "$log"
    rm "$log"
done
sh -n scripts/install.sh
