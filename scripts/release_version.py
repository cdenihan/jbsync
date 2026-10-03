#!/usr/bin/env python3
"""Reserve monotonically increasing UTC calendar versions; no network or Git writes."""
import datetime
from pathlib import Path
import re
import sys


def parse(version):
    match = re.fullmatch(r'(\d{4})\.(\d{2})\.(\d{2})\.([1-9]\d*)', version)
    if not match:
        raise ValueError('expected YYYY.MM.DD.N release version')
    year, month, day, count = map(int, match.groups())
    return datetime.date(year, month, day), count


def next_version(current, today):
    previous, count = parse(current)
    day = max(previous, today)
    return f'{day:%Y.%m.%d}.{count + 1 if day == previous else 1}'


def package_version(version):
    day, count = parse(version)
    return f'{day.year}.{day.month}.{day.day}+{count}'


if __name__ == '__main__':
    root = Path(__file__).resolve().parents[1]
    version = next_version((root / 'VERSION').read_text().strip(), datetime.datetime.now(datetime.timezone.utc).date())
    if sys.argv[1:] == ['--write']:
        (root / 'VERSION').write_text(version + '\n')
        manifest = root / 'build.zig.zon'
        manifest.write_text(re.sub(r'\.version = "[^"]+"', f'.version = "{package_version(version)}"', manifest.read_text(), count=1))
    elif sys.argv[1:]:
        raise SystemExit('usage: release_version.py [--write]')
    print(version)
