#!/usr/bin/env python3
"""Deterministic regression against frozen, verified legacy behavior."""
import argparse
import itertools
import json
from pathlib import Path
import random
import subprocess
import tempfile
import xml.etree.ElementTree as ET


def semantic(text):
    if text is None:
        return None
    try:
        n = ET.fromstring(text)
    except ET.ParseError:
        return text
    def tree(n):
        return (n.tag, sorted(n.attrib.items()), (n.text or '').strip(), [tree(c) for c in n])
    return tree(n)


def cases():
    root = Path(__file__).resolve().parents[2]
    result = []
    for p in sorted((root / 'tests/corpus').glob('*.xml')):
        text = p.read_text()
        result.append(dict(op='xml', local=text))
        # Force structural reconciliation without changing the actual settings.
        result.append(dict(op='merge', base=text, local=text, remote=text + '\n', policy='local'))
    rng = random.Random(1700)
    def document(values):
        return '<application><component name="Editor">' + ''.join(
            f'<option name="{k}" value="{v}"/>' for k, v in values.items()) + '</component></application>'
    for _ in range(500):
        base = dict(a='0', b='0', c='0')
        local, remote = base.copy(), base.copy()
        for side in (local, remote):
            for k in list(base) + ['d', 'e']:
                if rng.random() < .5:
                    value = rng.choice([None, '0', '1', '2', 'a &amp; b'])
                    if value is None:
                        side.pop(k, None)
                    else:
                        side[k] = value
        # Keep one stable leaf: empty component pruning is policy-specific.
        for side in (base, local, remote):
            side['stable'] = 'yes'
        for policy in ('local', 'remote', 'neither'):
            result.append(dict(op='merge', base=document(base), local=document(local), remote=document(remote), policy=policy))
    for b, l, r in itertools.product([None, 'DELETED', 'old', 'new'], repeat=3):
        for policy in ('local', 'remote', 'neither'):
            result.append(dict(op='merge', base=b, local=l, remote=r, policy=policy))
    flags = ['-Xmx4g', '-Xmx8g', '-XX:+UseZGC', '-Dfile.encoding=UTF-8', '-Dfile.encoding=ASCII', ' keep leading', 'duplicate', 'duplicate']
    for _ in range(150):
        texts = ['\n'.join(rng.sample(flags, rng.randrange(len(flags)+1)))+'\n' for _ in range(3)]
        for policy in ('local', 'remote', 'neither'):
            result.append(dict(op='merge', base=texts[0], local=texts[1], remote=texts[2], policy=policy))
    for pattern in ['*', '**', '**/*.xml', 'options/**', 'options/*.xml', '{options,colors}/*.xml', 'Py[CT]*', 'a?b', 'a[!x]b', 'a\\?b', '**/a/**/b', 'a/**', '**/a', 'a/**/b']:
        for text in ['editor.xml', 'options/editor.xml', 'colors/dark.xml', 'PyCharm', 'PyT', 'ab', 'a/b', 'a/x/b', 'a?b', 'aéb', 'a中b', 'a/']:
            result.append(dict(op='glob', pattern=pattern, local=text))
    for attribute, text in itertools.product(['a &amp; b', 'c&#10;d', 'a\r\nb', 'a\rb', 'é中😀', 'a&#xD;b'],
                                             ['text', 'a\r\nb', 'a\rb', 'é中😀', '&lt;&amp;&gt;']):
        result.append(dict(op='xml', local=f'<root attribute="{attribute}"><text>{text}</text></root>'))
    for build, since, until, deps, incompatible, product in itertools.product(
            ['', 'IC-262.3', 'PY-262.03', '261.9', '263.1'], ['', '262.1'], ['', '262.*', 'IC-262.*', '262.2'],
            [[], ['python']], [[], ['platform']], ['IntelliJIdea', 'PyCharm']):
        result.append(dict(op='plugin', build=build, product=product, capabilities=['python', 'platform'],
                           plugin=dict(id='x', since_build=since, until_build=until, required_dependencies=deps,
                                       incompatible_with=incompatible, source_products=['PyCharm'])))
    return result


def main():
    p = argparse.ArgumentParser()
    p.add_argument('zig', type=Path)
    args = p.parse_args()
    inputs = cases()
    with tempfile.TemporaryDirectory(prefix='jbsync-oracle-') as temp:
        fixture = Path(temp) / 'cases.json'
        fixture.write_text(json.dumps(inputs))
        outputs = [json.loads(subprocess.check_output([str(args.zig.resolve()), str(fixture)], text=True)), json.loads(Path(__file__).with_name('reference.json').read_text())['expected']]
    if any(len(output) != len(inputs) for output in outputs):
        raise AssertionError('oracle omitted results')
    failures = []
    for i, (c, zig, expected) in enumerate(zip(inputs, *outputs)):
        if c['op'] != 'glob':
            for out in (zig,):
                if 'content' in out:
                    out['content'] = semantic(out['content'])
        if json.loads(json.dumps(zig)) != expected:
            failures.append((i, c, zig, expected))
    for failure in failures[:8]:
        print(json.dumps(failure, ensure_ascii=False))
    print(f'{len(inputs)} legacy regression cases: {len(failures)} differences')
    if failures:
        raise SystemExit(1)

if __name__ == '__main__':
    main()
