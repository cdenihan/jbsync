# Development and releases

The production implementation requires Zig 0.17.0 and targets generic ARM on macOS 15 or newer (`aarch64-macos.15.0`).
The build began with `zig init`; its module, executable, run and test steps use
the standard build API. There are no external Zig package dependencies.

```sh
sh scripts/check.sh ReleaseSafe
sh scripts/check.sh Debug
sh scripts/check.sh ReleaseFast
```

Every mode runs native tests, 32 real CLI/Git scenarios, 2,688 deterministic
regression cases, legacy store migration, installer and self-update tests,
release version tests, and platform rejection checks. Fixtures use temporary
IDE roots and local bare Git remotes. Plugin tests use a mock launcher.

`tests/zig/reference.json` freezes the legacy implementation's verified merge,
XML, glob and plugin outputs at commit
`490807f8cc28cbf3d678f5906b668c7ffcbd3e26` (seed 1700). XML results compare ordered
structure rather than formatting. `legacy-store.json` contains real persisted
shared settings and IDE baselines from that implementation. The migration test
reconstructs transport history, preserves those bytes, upgrades two machines,
checks disjoint edits, verifies idempotence and runs Git integrity checks.
Neither fixture requires a Rust compiler or executable.

For a read-only check against your own IDE XML:

```sh
zig build validation -Doptimize=ReleaseSafe
python3 tests/zig/corpus.py zig-out/bin/jbsync-validation \
  "$HOME/Library/Application Support/JetBrains"
```

This checks semantic round trips and byte-idempotent serialization. Unsupported
XML is counted separately: sync preserves malformed or unsupported files as
opaque content. Ambiguous repeated XML addresses use whole-file merging;
writeback refuses replacement if it would discard private settings.

## Release process

CI validates Debug, ReleaseSafe and ReleaseFast on native Apple Silicon runners,
with a generic ARM target. Successful CI for the current `main` commit starts
release preparation. It advances the UTC calendar version in `VERSION` and the
package metadata, runs the full ReleaseSafe gate again, and atomically pushes
the version commit and reserved tag. A concurrent main update rejects that push.
The bot dispatches publishing explicitly, because token-authored pushes do not
trigger another workflow run.

Publishing verifies the dispatch ref, calendar version, tag, commit and main
ancestry; builds and tests with Zig 0.17.0/ReleaseSafe; verifies the binary's ARM
architecture and embedded version; and publishes `jbsync-macos-aarch64`,
`install.sh`, `VERSION` and SHA-256 sidecars. Failed publishing removes only this
run's unused reservation. No private dependency token or Rust release workflow
is required.

Installation and `jbsync update [--version YYYY.MM.DD.N] [--json]` share the
embedded installer. Downloads use HTTPS, checksum verification, ARM architecture
and executable-version checks, followed by a same-directory atomic replacement.
A failed verification preserves the installed executable. Mirror testing uses
`JBSYNC_RELEASE_BASE_URL=file:///...` exclusively in disposable directories.
Same-version identical binaries leave the existing executable untouched.

The release automation is prepared here; merging or publishing a live release
is a separate action. Real Marketplace downloads and a running IDE's settings
reload require manual release smoke testing. Close IDEs before a write-producing
sync, check `status`/`sync --dry-run`, sync, reopen the IDE, and verify settings
and an approved plugin installation on a disposable IDE profile.
