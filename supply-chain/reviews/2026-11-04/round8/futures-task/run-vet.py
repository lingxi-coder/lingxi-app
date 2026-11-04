#!/usr/bin/env python3
"""Capture a strict frozen cargo-vet run with a fresh cache and input hashes."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[4]
VET = Path('/private/tmp/round5-cargovet-tools/cargo-vet-aarch64-apple-darwin/cargo-vet')
EXPECTED_VET = 'bc8d963023f8a8a9102d2261922a817d59ceeabf41c55f1c9d7fa8ee8382ea82'
INPUTS = ('Cargo.toml', 'Cargo.lock', 'supply-chain/config.toml',
          'supply-chain/audits.toml', 'supply-chain/imports.lock')
sha = lambda data: hashlib.sha256(data).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('label', choices=('before', 'after', 'registration-before', 'registration-after'))
    parser.add_argument('--vet', type=Path, default=VET)
    args = parser.parse_args()
    binary = args.vet.resolve()
    if sha(binary.read_bytes()) != EXPECTED_VET:
        raise RuntimeError('cargo-vet executable does not match verified release')
    env = dict(os.environ, PATH=str(binary.parent) + os.pathsep + os.environ['PATH'],
               CARGO_INCREMENTAL='0', CARGO_PROFILE_DEV_DEBUG='0', CARGO_BUILD_JOBS='2')
    with tempfile.TemporaryDirectory(prefix='lingxi-vet-futures-task-') as temp:
        command = ['cargo', '+1.94.0', 'vet', 'check', '--locked', '--frozen',
                   '--no-minimize-exemptions', '--no-registry-suggestions',
                   '--cache-dir', temp, '--output-format', 'json']
        result = subprocess.run(command, cwd=ROOT, env=env, capture_output=True)
    (HERE / f'{args.label}-vet.json').write_bytes(result.stdout)
    (HERE / f'{args.label}-vet.stderr').write_bytes(result.stderr)
    report = json.loads(result.stdout)
    if result.returncode != 255 or report.get('conclusion') != 'fail (vetting)':
        raise RuntimeError(f'unexpected vet result {result.returncode}: {report.get("conclusion")}')
    context = {'head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
               'command': 'cargo +1.94.0 vet check --locked --frozen --no-minimize-exemptions --no-registry-suggestions --cache-dir <fresh temp> --output-format json',
               'cargo_vet_sha256': sha(binary.read_bytes()), 'exit_code': result.returncode,
               'failure_count': len(report['failures']), 'output_sha256': sha(result.stdout),
               'input_sha256': {name: sha((ROOT / name).read_bytes()) for name in INPUTS}}
    (HERE / f'{args.label}-context.json').write_text(json.dumps(context, indent=2) + '\n')
    print(json.dumps(context, indent=2))


if __name__ == '__main__':
    main()
