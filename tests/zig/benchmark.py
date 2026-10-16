#!/usr/bin/env python3
"""Disposable settled-sync benchmark; reports medians, no timing gate in CI."""
import importlib.util
import json
from pathlib import Path
import statistics
import sys
import tempfile
import time

binaries = [Path(p).resolve() for p in sys.argv[1:]]
sys.argv = [sys.argv[0], str(binaries[0])]
spec = importlib.util.spec_from_file_location('fixtures', Path(__file__).with_name('integration.py'))
fixtures = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixtures)
for binary in binaries:
    fixtures.BINARY = binary
    with tempfile.TemporaryDirectory(prefix='jbsync-bench-') as root:
        m = fixtures.Machine(Path(root) / 'machine')
        data = m.app / 'data'
        data.mkdir()
        (data / 'sync.toml').write_text('[jetbrains]\ninclude=["options/**"]\n[plugins]\nenabled=false\n')
        for name in m.names:
            for i in range(100):
                m.write(name, f'options/setting-{i}.xml', '<application><component name="Editor">' + ''.join(f'<option name="option-{j}" value="{j}"/>' for j in range(20)) + '</component></application>')
            for i in range(1000):
                m.write(name, f'workspace/cache-{i}.xml', '<private/>')
        m.sync()
        before = m.snapshot()
        samples = []
        for _ in range(5):
            start = time.perf_counter()
            m.sync()
            samples.append(time.perf_counter() - start)
            assert before == m.snapshot(), 'settled sync changed settings'
        print(json.dumps(dict(binary=str(binary), files=200, leaves=4000, excluded_files=2000, median_seconds=round(statistics.median(samples), 4), samples=samples)))
