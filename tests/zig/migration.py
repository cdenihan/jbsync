#!/usr/bin/env python3
"""Upgrade a real Rust-created store/baselines to Zig, then read it with Rust."""
import importlib.util
from pathlib import Path
import sys
import tempfile

if len(sys.argv) != 3:
    raise SystemExit('usage: migration.py ZIG_BINARY RUST_BINARY')
zig, rust = (Path(p).resolve() for p in sys.argv[1:])
sys.argv = [sys.argv[0], str(zig)]
spec = importlib.util.spec_from_file_location('fixtures', Path(__file__).with_name('integration.py'))
fixtures = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixtures)

with tempfile.TemporaryDirectory(prefix='jbsync-migration-') as temp:
    root = Path(temp)
    remote = root / 'remote.git'
    fixtures.git('init', '--bare', '-b', 'main', remote)
    a = fixtures.Machine(root / 'a', remote)
    b = fixtures.Machine(root / 'b', remote, names=(a.names[0],))
    fixtures.BINARY = rust
    for name in a.names:
        a.option(name)
    a.sync()
    b.sync()
    assert b.value(b.names[0]) == '4'
    # Real persisted Rust defaults, manifest, Git attributes and baseline files
    # remain in place. Zig must recognize disjoint edits against those bases.
    a.option(a.names[0], tabs='8')
    a.option(a.names[1], wrap='false')
    fixtures.BINARY = zig
    a.sync()
    b.sync()
    for m in (a, b):
        for name in m.names:
            assert m.value(name) == '8'
            assert m.value(name, 'wrap') == 'false'
        before = m.snapshot()
        m.sync()
        assert before == m.snapshot(), 'Zig upgrade failed to settle'
    fixtures.git('--git-dir', remote, 'fsck', '--no-reflogs')
    # The XML/store schema must still be readable by the retained Rust baseline.
    fixtures.BINARY = rust
    a.sync()
    b.sync()
    for m in (a, b):
        for name in m.names:
            assert m.value(name) == '8'
            assert m.value(name, 'wrap') == 'false'
    print('Rust → Zig → Rust: two-machine persisted-store migration passed')
