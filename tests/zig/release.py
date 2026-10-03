#!/usr/bin/env python3
"""Validate release version reservation and package metadata without publishing."""
import datetime
import importlib.util
from pathlib import Path
import re
import unittest
root = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('versions', root / 'scripts/release_version.py')
v = importlib.util.module_from_spec(spec)
spec.loader.exec_module(v)

class Release(unittest.TestCase):
    def test_next_day(self):
        self.assertEqual(v.next_version('2026.08.20.1', datetime.date(2026, 10, 3)), '2026.10.03.1')
    def test_same_day(self):
        self.assertEqual(v.next_version('2026.10.03.9', datetime.date(2026, 10, 3)), '2026.10.03.10')
    def test_clock_rollback(self):
        self.assertEqual(v.next_version('2026.10.03.1', datetime.date(2026, 10, 2)), '2026.10.03.2')
    def test_invalid_versions(self):
        for version in ('v2026.10.03.1', '2026.02.30.1', '2026.10.03.0', '2026.10.03.1/evil', 'latest'):
            with self.assertRaises(ValueError): v.parse(version)
    def test_manifest_version(self):
        version = (root / 'VERSION').read_text().strip()
        actual = re.search(r'\.version = "([^"]+)"', (root / 'build.zig.zon').read_text())[1]
        self.assertEqual(actual, v.package_version(version))
if __name__ == '__main__': unittest.main()
