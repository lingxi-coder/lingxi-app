#!/usr/bin/env python3
"""Verify the exact audit insertion and strictly frozen solver set difference."""
import hashlib
import json
from pathlib import Path
import subprocess
import tomllib

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[4]
sha = lambda data: hashlib.sha256(data).hexdigest()
contexts = {kind: json.loads((HERE / f'registration-{kind}-context.json').read_text())
            for kind in ('before', 'after')}
reports = {kind: json.loads((HERE / f'registration-{kind}-vet.json').read_text())
           for kind in ('before', 'after')}
baseline = json.loads((HERE / 'before-context.json').read_text())
for kind in ('before', 'after'):
    assert sha((HERE / f'registration-{kind}-vet.json').read_bytes()) == contexts[kind]['output_sha256']
    assert reports[kind]['conclusion'] == 'fail (vetting)'
    assert contexts[kind]['cargo_vet_sha256'] == 'bc8d963023f8a8a9102d2261922a817d59ceeabf41c55f1c9d7fa8ee8382ea82'
    assert contexts[kind]['command'] == 'cargo +1.94.0 vet check --locked --frozen --no-minimize-exemptions --no-registry-suggestions --cache-dir <fresh temp> --output-format json'
    assert contexts[kind]['exit_code'] == 255
    assert contexts[kind]['failure_count'] == len(reports[kind]['failures'])


def failures(report):
    rows = {(x['name'], x['version'], tuple(x['missing_criteria'])) for x in report['failures']}
    assert len(rows) == len(report['failures'])
    return rows


before, after = (failures(reports[kind]) for kind in ('before', 'after'))
only = ('futures-sink', '0.3.32', ('safe-to-deploy',))
assert len(before) == 814 and len(after) == 813
assert before - after == {only} and not after - before
original_inputs = contexts['before']['input_sha256']
new_inputs = contexts['after']['input_sha256']
assert set(original_inputs) == set(new_inputs)
for path in original_inputs:
    actual = sha((ROOT / path).read_bytes())
    assert actual == new_inputs[path], path
    if path != 'supply-chain/audits.toml':
        assert original_inputs[path] == new_inputs[path], path
base = subprocess.check_output(['git', 'show', f"{baseline['head']}:supply-chain/audits.toml"], cwd=ROOT)
assert sha(base) == original_inputs['supply-chain/audits.toml']
old_store = tomllib.loads(base.decode())
new_store = tomllib.loads((ROOT / 'supply-chain/audits.toml').read_text())
assert 'futures-sink' not in old_store['audits']
records = new_store['audits'].pop('futures-sink')
assert new_store == old_store and len(records) == 1
record = records[0]
assert set(record) == {'who', 'criteria', 'version', 'notes'}
assert record['version'] == '0.3.32' and record['criteria'] == 'safe-to-deploy'
assert record['who'] == 'Codex AI-assisted'
for path in ('review.md', 'root-cross-review.md'):
    assert sha((HERE / path).read_bytes()) in record['notes'], path
preflight = json.loads((HERE / 'registration-preflight.json').read_text())
assert preflight['remote_output'].split() == [baseline['head'], 'refs/heads/main']
assert preflight['input_sha256'] == original_inputs == baseline['input_sha256']
assert preflight['local_package_audits'] == []
assert contexts['before']['head'] == contexts['after']['head'] == preflight['head']
inspection = json.loads((HERE / 'store-inspection.json').read_text())
for path, evidence in inspection.items():
    assert evidence['sha256'] == original_inputs[path] and not evidence['matches']
    assert not evidence['raw_name_present']
for path, expected in original_inputs.items():
    remote_data = subprocess.check_output(['git', 'show', f"{baseline['head']}:{path}"], cwd=ROOT)
    assert sha(remote_data) == expected, path
imports = tomllib.loads((ROOT / 'supply-chain/imports.lock').read_text())
def contains_package(value):
    if isinstance(value, dict):
        return any(key == 'futures-sink' or contains_package(item) for key, item in value.items())
    if isinstance(value, list):
        return any(map(contains_package, value))
    return False
assert not contains_package(imports)
print(json.dumps({'before_failures': len(before), 'after_failures': len(after),
                  'removed': [{'name': only[0], 'version': only[1], 'missing_criteria': list(only[2])}],
                  'added': [], 'before_output_sha256': contexts['before']['output_sha256'],
                  'after_output_sha256': contexts['after']['output_sha256'],
                  'after_audits_sha256': new_inputs['supply-chain/audits.toml'],
                  'other_inputs_unchanged': True, 'only_audits_toml_change': 'futures-sink 0.3.32 exact full record',
                  'overall_conclusion': reports['after']['conclusion']}, indent=2))
