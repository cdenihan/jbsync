#!/usr/bin/env python3
"""Exercise the ARM-only release installer using local, disposable artifacts."""
import argparse
import hashlib
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

parser = argparse.ArgumentParser()
parser.add_argument('binary', type=Path)
args = parser.parse_args()
binary = args.binary.resolve()
root = Path(__file__).resolve().parents[2]


class Installer(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='jbsync-installer-')
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name)
        self.version = (root / 'VERSION').read_text().strip()
        self.release = self.path / 'releases/download' / ('v' + self.version)
        self.release.mkdir(parents=True)
        self.artifact = self.release / 'jbsync-macos-aarch64'
        self.artifact.write_bytes(binary.read_bytes())
        self.checksum = self.release / 'jbsync-macos-aarch64.sha256'
        self.checksum.write_text(hashlib.sha256(self.artifact.read_bytes()).hexdigest() + '  jbsync-macos-aarch64\n')
        self.destination = self.path / 'destination with spaces'
        self.destination.mkdir()
        self.installed = self.destination / 'jbsync'
        self.installed.write_bytes(b'previous executable')

    def update(self, **extra):
        self.installed.write_bytes(binary.read_bytes())
        self.installed.chmod(0o755)
        env = dict(os.environ, JBSYNC_RELEASE_BASE_URL=(self.path / 'releases').as_uri())
        env.pop('JBSYNC_INSTALLER_SOURCE_ONLY', None)
        return subprocess.run([str(self.installed), 'update', '--version', self.version, '--json'],
                              env=env, capture_output=True, text=True)

    def test_self_update_json_and_version(self):
        result = self.update()
        self.assertEqual(result.returncode, 0, result.stderr)
        import json
        summary = json.loads(result.stdout)
        self.assertEqual(summary['installed_version'], self.version)
        self.assertEqual(summary['status'], 'current')
        self.assertEqual(self.installed.read_bytes(), binary.read_bytes())

    def test_self_update_checksum_failure_preserves_executable(self):
        self.checksum.write_text('0' * 64 + '\n')
        result = self.update()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.installed.read_bytes(), binary.read_bytes())

    def test_invalid_release_version_keeps_previous_binary(self):
        before = self.installed.read_bytes()
        result = self.install('2026.08.20.1/other')
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(before, self.installed.read_bytes())

    def test_non_arm_artifact_is_rejected(self):
        self.artifact.write_text('#!/bin/sh\necho jbsync ' + self.version + '\n')
        self.checksum.write_text(hashlib.sha256(self.artifact.read_bytes()).hexdigest() + '\n')
        before = self.installed.read_bytes()
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(before, self.installed.read_bytes())

    def install(self, version=None):
        env = dict(os.environ, JBSYNC_RELEASE_BASE_URL=(self.path / 'releases').as_uri())
        env.pop('JBSYNC_INSTALLER_SOURCE_ONLY', None)
        return subprocess.run(['sh', str(root / 'scripts/install.sh'), '--version', version or self.version,
                               '--install-dir', str(self.destination)], env=env, capture_output=True, text=True)

    def unchanged(self):
        self.assertEqual(self.installed.read_bytes(), b'previous executable')
        self.assertEqual([p.name for p in self.destination.iterdir()], ['jbsync'])

    def test_verified_binary_installs_atomically(self):
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.installed.read_bytes(), binary.read_bytes())
        self.assertEqual(self.installed.stat().st_mode & 0o777, 0o755)
        self.assertEqual(subprocess.check_output([str(self.installed), '--version'], text=True).strip(), 'jbsync ' + self.version)
        self.assertEqual([p.name for p in self.destination.iterdir()], ['jbsync'])

    def test_bad_checksum_keeps_previous_binary(self):
        self.checksum.write_text('0' * 64 + '  jbsync-macos-aarch64\n')
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('checksum verification failed', result.stderr)
        self.unchanged()

    def test_malformed_checksum_keeps_previous_binary(self):
        self.checksum.write_text('not a checksum\n')
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('malformed', result.stderr)
        self.unchanged()

    def test_wrong_version_keeps_previous_binary(self):
        wrong_version = '2026.01.01.1'
        self.release.rename(self.release.parent / ('v' + wrong_version))
        result = self.install(wrong_version)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('version does not match', result.stderr)
        self.unchanged()


if __name__ == '__main__':
    unittest.main(argv=[__file__], verbosity=2)
