#!/usr/bin/env python3
"""Record remote main and frozen review inputs; does not alter Git refs."""
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import subprocess
import tomllib

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[4]
INPUTS = ('Cargo.toml', 'Cargo.lock', 'supply-chain/config.toml',
          'supply-chain/audits.toml', 'supply-chain/imports.lock')


def git(*args):
    return subprocess.check_output(['git', *args], cwd=ROOT, text=True).strip()


def main():
    origin = git('remote', 'get-url', 'origin')
    assert origin == 'git@github.com-lingxi-coder:lingxi-coder/lingxi-app.git'
    remote_output = git('ls-remote', '--heads', 'origin', 'main')
    remote_sha, ref = remote_output.split()
    assert ref == 'refs/heads/main'
    head = git('rev-parse', 'HEAD')
    lock = tomllib.loads((ROOT / 'Cargo.lock').read_text())
    pkg = next(p for p in lock['package'] if (p['name'], p['version']) == ('futures-core', '0.3.32'))
    audits = tomllib.loads((ROOT / 'supply-chain/audits.toml').read_text())
    report = {'checked_utc': datetime.now(timezone.utc).isoformat(), 'origin': origin,
              'command': 'git ls-remote --heads origin main', 'remote_output': remote_output,
              'head': head, 'head_matches_remote_at_capture': head == remote_sha,
              'input_sha256': {p: hashlib.sha256((ROOT / p).read_bytes()).hexdigest() for p in INPUTS},
              'package': pkg, 'local_package_audits': audits.get('audits', {}).get('futures-core', [])}
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
