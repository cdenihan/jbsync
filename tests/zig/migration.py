#!/usr/bin/env python3
"""Upgrade frozen legacy stores and baselines on two machines."""
import importlib.util
import base64
import json
from pathlib import Path
import sys
import tempfile

if len(sys.argv) != 2:
    raise SystemExit('usage: migration.py ZIG_BINARY')
zig = Path(sys.argv[1]).resolve()
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
    saved = json.loads(Path(__file__).with_name('legacy-store.json').read_text())['machines']
    for name, machine in [('a', a), ('b', b)]:
        for relative, encoded in saved[name].items():
            path = machine.path / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(base64.b64decode(encoded))
        store = machine.app / 'data'
        fixtures.git('-C', store, 'init', '-b', 'main')
        fixtures.git('-C', store, 'config', 'user.name', 'migration fixture')
        fixtures.git('-C', store, 'config', 'user.email', 'fixture@example.invalid')
        fixtures.git('-C', store, 'add', '.')
        fixtures.git('-C', store, 'commit', '-m', 'Frozen legacy store')
        fixtures.git('-C', store, 'remote', 'add', 'origin', remote)
    fixtures.git('-C', a.app / 'data', 'push', 'origin', 'main')
    # Both stores begin at the same commit, as after the original two-machine sync.
    fixtures.git('-C', b.app / 'data', 'fetch', 'origin')
    fixtures.git('-C', b.app / 'data', 'reset', '--hard', 'origin/main')
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
    print('Legacy → Zig: two-machine persisted-store migration passed')
