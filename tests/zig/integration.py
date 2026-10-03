#!/usr/bin/env python3
"""Real CLI/Git fixtures. Runs against either implementation for parity checks."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
import xml.etree.ElementTree as ET
import zipfile

PARSER = argparse.ArgumentParser()
PARSER.add_argument('binary', type=Path)
PARSER.add_argument('--rust-baseline', action='store_true')
ARGS = PARSER.parse_args()
BINARY = ARGS.binary.resolve()


def git(*args):
    return subprocess.run(['git', *map(str, args)], check=True, capture_output=True, text=True).stdout.strip()


class Machine:
    def __init__(self, path, remote=None, names=('IntelliJIdea2026.2', 'PyCharm2026.2'), launched=True):
        self.path = Path(path)
        self.app = self.path / 'jbsync'
        self.root = self.path / 'JetBrains'
        self.install = self.path / 'install'
        self.install.mkdir(parents=True)
        self.app.mkdir(parents=True)
        self.names = names
        # TOML accepts JSON double-quoted strings for paths used by these fixtures.
        (self.app / 'config.toml').write_text(
            '[repo]\n' + (f'remote = {json.dumps(str(remote))}\n' if remote else '') +
            f'[jetbrains]\nroot = {json.dumps(str(self.root))}\ninstall_roots = [{json.dumps(str(self.install))}]\n'
            f'[machine]\nid = {json.dumps(self.path.name)}\n'
        )
        for name in names:
            (self.root / name / 'options').mkdir(parents=True)
            if launched:
                self.write(name, 'options/other.xml', '<application/>')

    def write(self, name, relative, text):
        path = self.root / name / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def option(self, name, tabs='4', wrap='true'):
        self.write(name, 'options/editor.xml', f'<application><component name="Editor"><option name="tabs" value="{tabs}"/><option name="wrap" value="{wrap}"/></component></application>')

    def value(self, name, option='tabs'):
        path = self.root / name / 'options/editor.xml'
        if not path.exists():
            return None
        node = ET.parse(path).find(f"./component[@name='Editor']/option[@name='{option}']")
        return None if node is None else node.attrib['value']

    def run(self, *args, expected=0):
        env = dict(os.environ, GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_GLOBAL=os.devnull)
        env.pop('JBSYNC_CONFIG_DIR', None)
        p = subprocess.run([str(BINARY), '--config-dir', str(self.app), *args], capture_output=True, text=True, env=env)
        accepted = (expected, 2) if ARGS.rust_baseline and expected == 1 else (expected,)
        if p.returncode not in accepted:
            raise AssertionError(f'{args}: exit {p.returncode}\n{p.stdout}\n{p.stderr}')
        return p.stdout

    def sync(self, *args, expected=0):
        return self.run('sync', '--no-install-plugins', *args, expected=expected)

    def snapshot(self):
        # Git fetch may change transport metadata; these are the user/store bytes.
        return {str(p.relative_to(self.path)): hashlib.sha256(p.read_bytes()).hexdigest()
                for p in self.path.rglob('*') if p.is_file() and '.git' not in p.parts and p.name != 'sync.lock'}


class Integration(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='jbsync-zig-')
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name)
        self.remote = self.path / 'remote.git'
        git('init', '--bare', '-b', 'main', self.remote)

    def machine(self, name='a', **kw):
        return Machine(self.path / name, self.remote, **kw)

    def test_two_ides_disjoint_edits_converge_and_back_up(self):
        m = self.machine()
        for name in m.names:
            m.option(name)
        m.sync()
        m.option(m.names[0], tabs='8')
        m.option(m.names[1], wrap='false')
        out = m.sync()
        self.assertNotIn(': conflict ', out)
        for name in m.names:
            self.assertEqual(m.value(name), '8')
            self.assertEqual(m.value(name, 'wrap'), 'false')
        self.assertTrue(list((m.app / 'backups').rglob('editor.xml')))
        before = m.snapshot()
        m.sync()
        self.assertEqual(before, m.snapshot(), 'second sync is idempotent')

    def test_second_machine_adopts_and_two_machines_preserve_disjoint_edits(self):
        a = self.machine(names=('IntelliJIdea2026.2',))
        a.option(a.names[0], tabs='2')
        a.sync()
        b = self.machine('b', names=a.names)
        b.sync()
        self.assertEqual(b.value(b.names[0]), '2')
        a.option(a.names[0], tabs='8')
        a.sync()
        b.option(b.names[0], tabs='2', wrap='false')
        self.assertNotIn(': conflict ', b.sync())
        a.sync()
        for m in (a, b):
            self.assertEqual(m.value(m.names[0]), '8')
            self.assertEqual(m.value(m.names[0], 'wrap'), 'false')
        git('--git-dir', self.remote, 'fsck', '--full')

    @unittest.skipIf(ARGS.rust_baseline, 'Rust engine currently writes with --prefer neither; Zig intentionally aborts')
    def test_conflict_policies_and_neither_does_not_write(self):
        m = self.machine()
        for name in m.names:
            m.option(name)
        m.sync()
        m.option(m.names[0], tabs='8')
        m.option(m.names[1], tabs='2')
        before = m.snapshot()
        m.sync('--prefer', 'neither', expected=1)
        self.assertEqual(before, m.snapshot())
        self.assertIn('conflict', m.sync('--prefer', 'remote'))
        for name in m.names:
            self.assertEqual(m.value(name), '8')

    def test_dry_run_is_honest_and_changes_no_settings(self):
        m = self.machine()
        for name in m.names:
            m.option(name)
        m.sync()
        m.option(m.names[1], tabs='8')
        before = m.snapshot()
        out = m.sync('--dry-run')
        self.assertEqual(before, m.snapshot())
        self.assertIn('8', out)
        self.assertIn('8', m.sync())
        for name in m.names:
            self.assertEqual(m.value(name), '8')

    def test_privacy_and_surgical_writeback(self):
        m = self.machine()
        m.write(m.names[0], 'options/project.default.xml', '<application><component name="ProjectManager"><defaultProject><component name="TypeScriptCompiler"><option name="memoryAutoIncrease" value="true"/></component></defaultProject></component></application>')
        m.write(m.names[1], 'options/project.default.xml', '<application><component name="ProjectManager"><defaultProject><component name="WindowStateProjectService"><state x="123"/></component><component name="PropertiesComponent">local secret</component></defaultProject></component></application>')
        m.write(m.names[0], 'options/ide.general.xml', '<application><component name="Registry"><entry key="ide" value="yes" source="SYSTEM"/><entry key="user" value="yes" source="USER"/></component></application>')
        m.sync()
        shared = (m.app / 'data/shared/options/project.default.xml').read_text()
        self.assertNotIn('local secret', shared)
        self.assertNotIn('WindowState', shared)
        local = (m.root / m.names[1] / 'options/project.default.xml').read_text()
        self.assertIn('local secret', local)
        self.assertIn('WindowState', local)
        self.assertIn('memoryAutoIncrease', local)
        self.assertNotIn('SYSTEM', (m.app / 'data/shared/options/ide.general.xml').read_text())

    def test_exclusions_apply_to_incoming_and_machine_overrides(self):
        a = self.machine(names=('IntelliJIdea2026.2',))
        a.option(a.names[0], tabs='8')
        a.sync()
        b = self.machine('b', names=a.names)
        (b.app / 'data/machines').mkdir(parents=True)
        (b.app / 'data/machines/b.toml').write_text("[jetbrains]\nexclude=['options/editor.xml']\n")
        b.sync()
        self.assertIsNone(b.value(b.names[0]))

    def test_unlaunched_defaults_are_learned_and_not_written(self):
        m = self.machine(launched=False)
        for name in m.names:
            m.option(name)
        m.sync()
        self.assertFalse((m.app / 'data/shared/options/editor.xml').exists())
        self.assertTrue(list((m.app / 'data/defaults').glob('*.toml')))
        m.write(m.names[0], 'options/other.xml', '<application/>')
        m.option(m.names[0], tabs='8')
        m.sync()
        self.assertEqual(m.value(m.names[1]), '4', 'unlaunched IDE must not receive settings')
        shared = (m.app / 'data/shared/options/editor.xml').read_text()
        self.assertIn('8', shared)
        self.assertNotIn('wrap', shared)

    def test_plugin_jar_manifest_and_rules(self):
        m = self.machine(names=('IntelliJIdea2026.2',))
        plugin = m.root / m.names[0] / 'plugins/test/lib'
        plugin.mkdir(parents=True)
        with zipfile.ZipFile(plugin / 'test.jar', 'w', zipfile.ZIP_DEFLATED) as z:
            z.writestr('META-INF/plugin.xml', '<idea-plugin><id>example.plugin</id><name>Example</name><version>1.0</version><depends optional="true">optional</depends></idea-plugin>')
        m.sync()
        manifest = json.loads((m.app / 'data/plugins.json').read_text())
        self.assertEqual(manifest['plugins'][0]['id'], 'example.plugin')
        m.run('plugins', 'only', 'example.plugin', '--ide', 'IntelliJ*')
        before = (m.app / 'data/sync.toml').read_bytes()
        m.run('plugins', 'only', 'example.plugin', '--ide', 'IntelliJ*')
        self.assertEqual(before, (m.app / 'data/sync.toml').read_bytes())

    def test_vmoptions_and_collect_only(self):
        m = self.machine()
        (m.app / 'data').mkdir()
        (m.app / 'data/sync.toml').write_text("[jetbrains]\nexplicit_include=['*.vmoptions']\nvmoptions_names={PyCharm='pycharm.vmoptions'}\n")
        m.write(m.names[0], 'idea.vmoptions', '-Xmx4g\n-XX:+UseZGC\n')
        m.sync('--collect-only')
        self.assertFalse((m.root / m.names[1] / 'pycharm.vmoptions').exists())
        m.sync()
        self.assertIn('UseZGC', (m.root / m.names[1] / 'pycharm.vmoptions').read_text())

    @unittest.skipIf(ARGS.rust_baseline, 'Rust currently retains deleted shared files on the other machine')
    def test_remote_deletion_preserves_private_content(self):
        a = self.machine(names=('IntelliJIdea2026.2',))
        a.option(a.names[0], tabs='8')
        a.sync()
        b = self.machine('b', names=a.names)
        b.sync()
        (a.root / a.names[0] / 'options/editor.xml').unlink()
        a.sync()
        b.sync()
        self.assertIsNone(b.value(b.names[0]))

    @unittest.skipIf(os.name == 'nt', 'POSIX advisory lock fixture')
    def test_concurrent_run_is_rejected(self):
        import fcntl
        m = self.machine()
        m.sync()
        with (m.app / 'sync.lock').open('a') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            m.sync(expected=1)

    @unittest.skipIf(os.name == 'nt', 'Windows symlink creation needs extra privileges')
    @unittest.skipIf(ARGS.rust_baseline, 'Rust skips symlink files rather than rejecting the sync')
    def test_incoming_settings_never_follow_a_symlink(self):
        m = self.machine()
        m.option(m.names[0], tabs='8')
        m.sync()
        editor = m.root / m.names[1] / 'options/editor.xml'
        editor.unlink()
        secret = self.path / 'unmanaged.xml'
        secret.write_text('<application/>')
        editor.symlink_to(secret)
        before = secret.read_bytes()
        m.sync(expected=1)
        self.assertEqual(before, secret.read_bytes())

    def test_invalid_config_and_command_specific_flags(self):
        m = self.machine()
        m.run('repo', 'unset', '--dry-run', expected=1)
        self.assertIn('remote', (m.app / 'config.toml').read_text())
        (m.app / 'data').mkdir()
        (m.app / 'data/sync.toml').write_text("[jetbrains]\nbackups='false'\n")
        m.sync(expected=1)


    def test_builtin_sync_switches_and_unknown_flags(self):
        m = self.machine()
        m.run('disable-builtin-sync', '--dry-run')
        self.assertFalse((m.root / m.names[0] / 'options/settingsSync.xml').exists())
        m.run('disable-builtin-sync')
        self.assertIn('false', (m.root / m.names[0] / 'options/settingsSync.xml').read_text())
        m.run('sync', '--dr-run', expected=1)

    def test_version_matches_release_metadata(self):
        m = self.machine()
        version = (Path(__file__).resolve().parents[2] / 'VERSION').read_text().strip()
        self.assertEqual(m.run('--version').strip(), 'jbsync ' + version)

    def seed_plugins(self, m, plugins, policy=''):
        m.sync()
        (m.app / 'data/plugins.json').write_text(json.dumps(dict(version=1, plugins=plugins)))
        (m.app / 'data/sync.toml').write_text(policy)

    def test_existing_file_permissions_survive_replacement(self):
        m = self.machine()
        for name in m.names:
            m.option(name)
        m.sync()
        target = m.root / m.names[1] / 'options/editor.xml'
        target.chmod(0o640)
        m.option(m.names[0], tabs='8')
        m.sync()
        self.assertEqual(target.stat().st_mode & 0o777, 0o640)
        self.assertEqual(m.value(m.names[1]), '8')

    @unittest.skipIf(ARGS.rust_baseline, 'Rust second-resolution backup folders coalesce rapid syncs')
    def test_backup_retention_and_original_bytes(self):
        m = self.machine()
        for name in m.names:
            m.option(name)
        m.sync()
        for i in range(12):
            old = (m.root / m.names[1] / 'options/editor.xml').read_bytes()
            m.option(m.names[0], tabs=str(8+i))
            m.sync()
            runs = sorted((m.app / 'backups').iterdir())
            self.assertLessEqual(len(runs), 10)
            self.assertEqual((runs[-1] / m.names[1] / 'options/editor.xml').read_bytes(), old)
        self.assertEqual(len(list((m.app / 'backups').iterdir())), 10)

    def test_backup_opt_out(self):
        m = self.machine()
        for name in m.names:
            m.option(name)
        m.sync()
        (m.app / 'data/sync.toml').write_text('[jetbrains]\nbackups=false\n')
        m.option(m.names[0], tabs='8')
        m.sync()
        self.assertEqual(m.value(m.names[1]), '8')
        self.assertFalse((m.app / 'backups').exists())

    def test_capability_does_not_suppress_plugin_install(self):
        m = self.machine(names=('IntelliJIdea2026.2',))
        self.seed_plugins(m, [dict(id='example', source_products=['IntelliJIdea'])],
                          "[[plugins.capability]]\nide='*'\nadd=['example']\n")
        self.assertIn('install example', m.sync())

    def test_bundled_plugins_and_capability_removal(self):
        m = self.machine(names=('IntelliJIdea2026.2',))
        self.seed_plugins(m, [dict(id='example', source_products=['IntelliJIdea'])])
        m.write(m.names[0], 'bundled_plugins.txt', 'example|1.0\n')
        self.assertNotIn('install example', m.sync())
        (m.app / 'data/sync.toml').write_text("[[plugins.capability]]\nide='*'\nremove=['example']\n")
        self.assertIn('install example', m.sync())

    def test_product_build_prefix_and_conservative_plugin_heuristic(self):
        m = self.machine(names=('IntelliJIdea2026.2',))
        (m.install / 'product-info.json').write_text(json.dumps(dict(
            dataDirectoryName=m.names[0], buildNumber='IC-262.3', modules=['platform'])))
        self.seed_plugins(m, [dict(id='compatible', until_build='262.*', required_dependencies=['platform']),
                              dict(id='unknown', source_products=['PyCharm']),
                              dict(id='too-new', since_build='263.1', required_dependencies=['platform'])])
        out = m.sync()
        self.assertIn('install compatible', out)
        self.assertNotIn('install unknown', out)
        self.assertNotIn('install too-new', out)

    def test_plugin_launcher_is_called_only_after_dry_run(self):
        m = self.machine(names=('IntelliJIdea2026.2',))
        marker = self.path / 'installed.txt'
        launcher = self.path / 'launcher'
        launcher.write_text('#!/bin/sh\nprintf "%s\\n" "$@" >> ' + str(marker) + '\n')
        launcher.chmod(0o700)
        self.seed_plugins(m, [dict(id='example', source_products=['IntelliJIdea'])],
                          '[plugins.launchers]\nIntelliJIdea=' + json.dumps(str(launcher)) + '\n')
        before = m.snapshot()
        m.run('sync', '--dry-run')
        self.assertFalse(marker.exists())
        self.assertEqual(before, m.snapshot())
        m.run('sync')
        self.assertEqual(marker.read_text().splitlines(), ['installPlugins', 'example'])

    def test_managed_plugin_dependencies_are_planned(self):
        m = self.machine(names=('IntelliJIdea2026.2',))
        self.seed_plugins(m, [dict(id='a-dependent', required_dependencies=['z-provider']),
                              dict(id='z-provider', source_products=['IntelliJIdea'])])
        out = m.sync()
        self.assertIn('install z-provider', out)
        self.assertIn('install a-dependent', out)

    @unittest.skipIf(ARGS.rust_baseline, 'Rust loads remote policy on the next sync')
    def test_remote_policy_applies_in_same_sync(self):
        a = self.machine(names=('IntelliJIdea2026.2',))
        a.option(a.names[0], tabs='8')
        a.sync()
        (a.app / 'data/sync.toml').write_text("[jetbrains]\nexclude=['options/editor.xml']\n")
        a.sync()
        b = self.machine('b', names=a.names)
        b.sync()
        self.assertIsNone(b.value(b.names[0]))

    @unittest.skipIf(ARGS.rust_baseline, 'Rust collapses repeated IntelliLang projection addresses during edits')
    def test_real_xml_corpus_converges_without_loss(self):
        m = self.machine()
        m.sync()
        (m.app / 'data/sync.toml').write_text("[jetbrains]\ninclude=['**']\n")
        for fixture in sorted((Path(__file__).resolve().parents[1] / 'corpus').glob('*.xml')):
            relative = fixture.name.replace('__', '/')
            m.write(m.names[0], relative, fixture.read_text())
        m.sync()
        before = m.snapshot()
        m.sync()
        self.assertEqual(before, m.snapshot())
        # IntelliLang has repeated keyed siblings; every injection must remain.
        source = ET.parse(m.root / m.names[0] / 'options/IntelliLang.xml')
        target = ET.parse(m.root / m.names[1] / 'options/IntelliLang.xml')
        def tree(n):
            return (n.tag, sorted(n.attrib.items()), (n.text or '').strip(), [tree(c) for c in n])
        self.assertEqual(tree(source.getroot()), tree(target.getroot()))
        # A later edit to repeated keyed siblings must remain lossless too.
        path = m.root / m.names[0] / 'options/IntelliLang.xml'
        path.write_text(path.read_text().replace('injector-id="groovy"', 'injector-id="changed"', 1))
        m.sync()
        self.assertEqual(tree(ET.parse(path).getroot()), tree(ET.parse(m.root / m.names[1] / 'options/IntelliLang.xml').getroot()))

    @unittest.skipIf(ARGS.rust_baseline, 'Rust currently suppresses failed fetches')
    def test_failed_fetch_leaves_settings_untouched(self):
        m = self.machine()
        m.option(m.names[0])
        m.sync()
        config = m.app / 'config.toml'
        config.write_text(config.read_text().replace(str(self.remote), str(self.path / 'absent.git')))
        before = m.snapshot()
        m.sync(expected=1)
        self.assertEqual(before, m.snapshot())

    def test_ide_selector_limits_writes(self):
        m = self.machine()
        m.option(m.names[0], tabs='8')
        m.sync('--ide', 'IntelliJ*')
        self.assertIsNone(m.value(m.names[1]))
        m.sync()
        self.assertEqual(m.value(m.names[1]), '8')

    def test_unlaunched_ide_with_ambiguous_factory_xml(self):
        m = self.machine(launched=False)
        fixture = Path(__file__).resolve().parents[1] / 'corpus/options__IntelliLang.xml'
        m.write(m.names[0], 'options/IntelliLang.xml', fixture.read_text())
        m.option(m.names[0])
        before = (m.root / m.names[0] / 'options/IntelliLang.xml').read_bytes()
        m.sync()
        self.assertEqual(before, (m.root / m.names[0] / 'options/IntelliLang.xml').read_bytes())
        self.assertFalse((m.app / 'data/shared/options/IntelliLang.xml').exists())
        self.assertTrue(list((m.app / 'data/defaults').glob('*.toml')))

    @unittest.skipIf(ARGS.rust_baseline, 'Zig refuses ambiguous whole-file replacement that would discard private settings')
    def test_ambiguous_xml_with_private_values_refuses_destructive_write(self):
        m = self.machine()
        m.sync()
        (m.app / 'data/sync.toml').write_text(
            "[jetbrains]\nexplicit_include=['options/ambiguous.xml']\n"
            "[[xml.omit]]\nfile='options/ambiguous.xml'\ncomponent='Private'\noption='secret'\n")
        raw = '<application><component name="Injection"><injection language="RegExp" value="one"/><injection language="RegExp" value="two"/></component><component name="Private"><option name="secret" value="private"/></component></application>'
        for name in m.names:
            m.write(name, 'options/ambiguous.xml', raw)
        m.sync()
        m.write(m.names[0], 'options/ambiguous.xml', raw.replace('value="one"', 'value="changed"'))
        before = m.snapshot()
        m.sync(expected=1)
        self.assertEqual(before, m.snapshot())

    @unittest.skipIf(ARGS.rust_baseline, 'Rust omits the selected global color scheme from its leaf projection')
    def test_color_scheme_selection_conflicts_as_one_value(self):
        m = self.machine()
        def scheme(value):
            return '<application><component name="EditorColorsManager"><global_color_scheme name="' + value + '"/></component></application>'
        for name in m.names:
            m.write(name, 'options/colors.scheme.xml', scheme('Dark'))
        m.sync()
        m.write(m.names[0], 'options/colors.scheme.xml', scheme('Solarized'))
        m.write(m.names[1], 'options/colors.scheme.xml', scheme('Blue'))
        out = m.sync('--prefer', 'remote')
        self.assertIn(': conflict ', out)
        for name in m.names:
            nodes = ET.parse(m.root / name / 'options/colors.scheme.xml').findall('./component/global_color_scheme')
            self.assertEqual(len(nodes), 1)
            self.assertEqual(nodes[0].attrib['name'], 'Solarized')


if __name__ == '__main__':
    unittest.main(argv=[str(__file__)], verbosity=2)
