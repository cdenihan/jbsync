#!/usr/bin/env python3
"""Read-only XML round trips. Never writes IDE files or prints their contents."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import xml.etree.ElementTree as ET


def semantic(text):
    node = ET.fromstring(text)
    def tree(n):
        return (n.tag, sorted(n.attrib.items()), (n.text or '').strip(), (n.tail or '').strip(), [tree(c) for c in n])
    return tree(node)


def run(binary, cases):
    with tempfile.TemporaryDirectory(prefix='jbsync-corpus-') as temp:
        path = Path(temp) / 'cases.json'
        path.write_text(json.dumps(cases))
        return json.loads(subprocess.check_output([str(binary.resolve()), str(path)], text=True))


def main():
    p = argparse.ArgumentParser()
    p.add_argument('validator', type=Path)
    p.add_argument('root', type=Path)
    args = p.parse_args()
    texts = []
    opaque = 0
    for directory, dirs, names in os.walk(args.root):
        dirs[:] = [d for d in dirs if d not in ('.git', 'plugins', 'system', 'workspace', 'log', 'event-log-metadata') and not (Path(directory) / d).is_symlink()]
        for name in sorted(names):
            path = Path(directory) / name
            if path.suffix != '.xml' or path.is_symlink() or path.stat().st_size > 32 * 1024 * 1024:
                continue
            try:
                text = path.read_text()
                semantic(text)
            except (UnicodeError, ET.ParseError):
                opaque += 1
                continue
            texts.append(text)
    if not texts:
        raise SystemExit('No readable XML documents found')
    checked = 0
    # Batch below the validator's bounded input size even with large IDE files.
    for text in texts:
        out = run(args.validator, [dict(op='xml', local=text)])[0]
        if out.get('error'):
            opaque += 1
            continue
        assert semantic(text) == semantic(out['content']), 'XML changed during round trip'
        second = run(args.validator, [dict(op='xml', local=out['content'])])[0]
        assert second['content'] == out['content'], 'Serialization is not idempotent'
        checked += 1
    print(f'{checked} XML documents round-trip without loss; {opaque} opaque/unsupported documents')

if __name__ == '__main__': main()
