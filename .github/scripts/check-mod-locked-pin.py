#!/usr/bin/env python3
"""Runner-only exact git-pin experiment; never mutates the committed App pin."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tomllib

root = Path(os.environ['GITHUB_WORKSPACE'])
temp = Path(os.environ['RUNNER_TEMP'])
manifest = root / 'Cargo.toml'
lockfile = root / 'Cargo.lock'
old = 'ff076c70babd85c1dd5f9cb04034b472d68e1797'
new = '02c0beac6b3e17632f44e7bff0b2c03cb6600b20'
repo = 'https://github.com/lingxi-coder/harness-runtime.git'
original_text = manifest.read_text()
manifest_data = tomllib.loads(original_text)
selected = {name: spec for name, spec in manifest_data['workspace']['dependencies'].items()
            if isinstance(spec, dict) and spec.get('git') == repo}
if len(selected) != 45 or any(spec.get('rev') != old for spec in selected.values()):
    raise SystemExit('unexpected Harness dependency inventory or base pin')
patched = {name: spec for name, spec in manifest_data.get('patch', {}).get('crates-io', {}).items()
           if isinstance(spec, dict) and spec.get('git') == repo}
if set(patched) != {'nix'} or any(spec.get('rev') != old for spec in patched.values()):
    raise SystemExit('unexpected Harness crates-io patch inventory or base pin')
original_lock = lockfile.read_bytes()
print(f'Original App git pin: {old}; direct dependencies: {len(selected)}; patched packages: {len(patched)}', flush=True)
print(f'Original Cargo.lock SHA256: {hashlib.sha256(original_lock).hexdigest()}', flush=True)
lines = original_text.splitlines(keepends=True)
changed = 0
for index, line in enumerate(lines):
    if f'git = "{repo}"' in line:
        old_marker = f'rev = "{old}"'
        if old_marker not in line:
            raise SystemExit(f'unexpected source line: {line}')
        lines[index] = line.replace(old_marker, f'rev = "{new}"')
        changed += 1
if changed != len(selected) + len(patched):
    raise SystemExit(f'changed {changed} entries but expected {len(selected) + len(patched)}')
manifest.write_text(''.join(lines))
subprocess.run(['cargo', 'update', '--package', 'harness-runtime'], cwd=root, check=True)
with (temp / 'pin-lock-metadata.json').open('w') as stream:
    subprocess.run(['cargo','metadata','--locked','--all-features','--format-version=1'],
                   cwd=root, stdout=stream, check=True)
lock = tomllib.loads(lockfile.read_text())
entries = [p for p in lock['package'] if p.get('source', '').startswith('git+' + repo + '?')]
if not entries or any(p['source'] != f'git+{repo}?rev={new}#{new}' for p in entries):
    raise SystemExit('Cargo.lock did not resolve every Harness package to the exact diagnostic git revision')
metadata = json.loads((temp / 'pin-lock-metadata.json').read_text())
actual = [p for p in metadata['packages'] if (p.get('source') or '').startswith('git+' + repo + '?')]
if not actual or any(p['source'] != f'git+{repo}?rev={new}#{new}' for p in actual):
    raise SystemExit('cargo metadata did not resolve the exact diagnostic git revision')
print(f'Canonical git pin: {new}; Cargo.lock Harness packages: {len(entries)}; metadata packages: {len(actual)}', flush=True)
print(f'Updated Cargo.lock SHA256: {hashlib.sha256(lockfile.read_bytes()).hexdigest()}', flush=True)
subprocess.run(['git','diff','--stat','--','Cargo.toml','Cargo.lock'],cwd=root,check=True)
with (temp / 'pin-lock.diff').open('w') as out:
    subprocess.run(['git','diff','--','Cargo.toml','Cargo.lock'],cwd=root,stdout=out,check=True)
(temp / 'pin-lock-Cargo.lock').write_bytes(lockfile.read_bytes())
