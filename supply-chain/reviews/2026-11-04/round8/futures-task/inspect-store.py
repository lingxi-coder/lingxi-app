#!/usr/bin/env python3
"""Inspect all frozen store inputs for this package's exact name."""
import hashlib
import json
from pathlib import Path
import tomllib

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[4]
NAME = 'futures-task'


def matches(value, path=''):
    found = []
    if isinstance(value, dict):
        for key, item in value.items():
            child = f'{path}.{key}' if path else key
            if NAME in key:
                found.append({'path': child, 'kind': 'key'})
            found.extend(matches(item, child))
    elif isinstance(value, list):
        for i, item in enumerate(value):
            found.extend(matches(item, f'{path}[{i}]'))
    elif isinstance(value, str) and NAME in value:
        found.append({'path': path, 'kind': 'value', 'value': value})
    return found


def main():
    result = {}
    for path in ('supply-chain/audits.toml', 'supply-chain/config.toml', 'supply-chain/imports.lock'):
        raw = (ROOT / path).read_bytes()
        parsed = tomllib.loads(raw.decode())
        result[path] = {'sha256': hashlib.sha256(raw).hexdigest(), 'matches': matches(parsed),
                        'raw_name_present': NAME.encode() in raw}
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
