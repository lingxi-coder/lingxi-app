import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { docPages } from '../src/data/docs';

const web = fileURLToPath(new URL('..', import.meta.url));

const metadataIdentityFixture = String.raw`
import json, pathlib, sys, tempfile, types
sys.path.insert(0, str(pathlib.Path('examples').resolve()))
import sdk_tools

temporary = tempfile.TemporaryDirectory()
root = pathlib.Path(temporary.name).resolve()
sdk_root = root / 'harness-runtime'
sdk_root.mkdir()
(sdk_root / 'Cargo.toml').write_text('[workspace]\n[workspace.package]\nversion = "1.2.3"\n')
core = sdk_root / 'crates/sdk-core/Cargo.toml'
core.parent.mkdir(parents=True)
core.write_text('[package]\nname = "sdk-core"\nversion.workspace = true\n')
vendored = sdk_root / 'packages/webrtc/vendor/nix/Cargo.toml'
vendored.parent.mkdir(parents=True)
vendored.write_text('[package]\nname = "nix"\nversion = "0.31.3"\n')
third_party = sdk_root / 'third_party/common/Cargo.toml'
third_party.parent.mkdir(parents=True)
third_party.write_text('[package]\nname = "common"\nversion = "1.0.0"\n')
snapshot = sdk_tools.SdkSnapshot.__new__(sdk_tools.SdkSnapshot)
snapshot.root = root
snapshot.expected = snapshot._expected_packages()
registry = 'registry+https://github.com/rust-lang/crates.io-index'

def package(name, version, manifest, source=None):
    return dict(name=name, version=version, manifest_path=str(manifest), source=source,
                id=f'{name}@{version}:{manifest}:{source}')

local_core = package('sdk-core', '1.2.3', core)
local_nix = package('nix', '0.31.3', vendored)

def registry_nix(version):
    return package('nix', version, root / f'registry/nix-{version}/Cargo.toml', registry)

def verify(packages):
    metadata = dict(packages=packages, resolve=dict(nodes=[dict(id=p['id']) for p in packages]))
    sdk_tools.command = lambda *args, **kwargs: types.SimpleNamespace(stdout=json.dumps(metadata))
    return snapshot.verify_metadata(root / 'consumer/Cargo.toml')

def rejected(packages, identity):
    try:
        verify(packages)
    except RuntimeError as error:
        assert f'Wrong SDK identity for {identity}' in str(error), str(error)
    else:
        raise AssertionError(f'{identity} escaped snapshot identity validation')
`;

function checkMetadataIdentity(source: string): void {
  const result = spawnSync('python3', ['-c', `${metadataIdentityFixture}\n${source}`], { cwd: web, encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
}

test('vendored crate identity does not capture a different registry version with the same name', () => {
  checkMetadataIdentity(String.raw`
identities = verify([local_core, registry_nix('0.27.1'), local_nix])
assert {entry['package'] for entry in identities} == {'sdk-core', 'nix'}
assert next(entry for entry in identities if entry['package'] == 'nix')['manifest'] == str(vendored.relative_to(root))
`);
});

test('the vendored version still requires its local path and no external source', () => {
  checkMetadataIdentity(String.raw`
rejected([local_core, registry_nix('0.31.3')], 'nix@0.31.3')
rejected([local_core, dict(local_nix, source=registry)], 'nix@0.31.3')
rejected([local_core, dict(local_nix, manifest_path=str(root / 'other/nix/Cargo.toml'))], 'nix@0.31.3')
rejected([local_core, package('common', '1.0.0', root / 'registry/common/Cargo.toml', registry)], 'common@1.0.0')
`);
});

test('first-party SDK identities reject an external source, wrong path or different version', () => {
  checkMetadataIdentity(String.raw`
assert verify([local_core])[0]['package'] == 'sdk-core'
rejected([dict(local_core, source='git+https://example.invalid/sdk')], 'sdk-core@1.2.3')
rejected([dict(local_core, manifest_path=str(root / 'registry/sdk-core/Cargo.toml'))], 'sdk-core@1.2.3')
rejected([dict(local_core, version='1.2.4')], 'sdk-core@1.2.4')
`);
});

test('copyable reader manifests match the isolated consumer inputs exactly', () => {
  const blocks = docPages.flatMap((page) => page.sections.flatMap((section) => section.code ?? []));
  for (const sdk of ['harness', 'llm', 'mobile']) {
    const matches = blocks.filter((block) => block.exampleId === `${sdk}-working-tree-manifest`);
    assert.equal(matches.length, 1, `Missing complete reader manifest: ${sdk}`);
    const template = readFileSync(`${web}/examples/${sdk}/Cargo.toml`, 'utf8').trim();
    assert.equal(matches[0].code.trim(), template, `${sdk} manifest cannot silently differ from tested configuration`);
  }
  const harness = readFileSync(`${web}/examples/harness/Cargo.toml`, 'utf8');
  assert.match(harness, /patch\."https:\/\/github\.com\/lingxi-coder\/llm-client\.git"/);
  for (const name of ['mobile-linux-api', 'mobile-linux-core', 'mobile-linux-android', 'mobile-linux-ios', 'platform-pty']) {
    assert.ok(harness.includes(`${name} = { path = "../mobile-linux-runtime/crates/${name}" }`));
  }
});

test('validator extracts every compiled reader block and rejects missing stable identities', () => {
  const result = spawnSync('python3', ['scripts/check-doc-examples.py', '--inventory'], { cwd: web, encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
  const inventory = JSON.parse(result.stdout) as Array<{ exampleId?: string; verification?: string }>;
  const checked = inventory.filter((block) => ['compile', 'mock'].includes(block.verification ?? ''));
  assert.ok(checked.length >= 10);
  assert.equal(new Set(checked.map((block) => block.exampleId)).size, checked.length);
  assert.ok(checked.every((block) => block.exampleId));
  assert.ok(checked.some((block) => block.exampleId === 'harness-lifecycle-mock' && block.verification === 'mock'));
  assert.ok(checked.some((block) => block.exampleId === 'llm-transport-mock' && block.verification === 'mock'));
});

test('source snapshots preserve staged and unstaged reader reality while excluding build artifacts', () => {
  const source = String.raw`
import importlib.util, pathlib, subprocess, tempfile, sys
sys.path.insert(0, str(pathlib.Path('examples').resolve()))
from sdk_tools import snapshot_repo, isolated_cargo_home
with tempfile.TemporaryDirectory() as temp:
    root = pathlib.Path(temp)
    repo = root / 'repo'
    repo.mkdir()
    subprocess.run(['git', 'init', '-q', str(repo)], check=True)
    (repo / '.gitignore').write_text('target/\n')
    (repo / 'source.rs').write_text('old')
    (repo / 'deleted.rs').write_text('deleted')
    subprocess.run(['git', '-C', str(repo), 'add', '.'], check=True)
    (repo / 'source.rs').write_text('staged')
    subprocess.run(['git', '-C', str(repo), 'add', 'source.rs'], check=True)
    (repo / 'source.rs').write_text('working-tree')
    (repo / 'new.rs').write_text('new')
    (repo / 'deleted.rs').unlink()
    (repo / 'target').mkdir()
    (repo / 'target' / 'build.rs').write_text('ignored')
    destination = root / 'snapshot'
    snapshot_repo(repo, destination)
    assert (destination / 'source.rs').read_text() == 'working-tree'
    assert (destination / 'new.rs').read_text() == 'new'
    assert not (destination / 'deleted.rs').exists()
    assert not (destination / 'target').exists()
    staged = subprocess.check_output(['git', '-C', str(repo), 'show', ':source.rs']).decode()
    assert staged == 'staged', 'Snapshot must not change the index'
    cache = root / 'original-cargo-home'
    existing = cache / 'registry' / 'src' / 'registry-id' / 'already-extracted'
    existing.mkdir(parents=True)
    (existing / 'source.rs').write_text('cached source')
    (cache / 'registry' / 'src' / '.DS_Store').write_bytes(b'finder metadata')
    mirror = root / 'writable-cargo-home'
    isolated_cargo_home(cache, mirror)
    assert (mirror / 'registry/src/registry-id/already-extracted/source.rs').read_text() == 'cached source'
    extracted = mirror / 'registry/src/registry-id/newly-extracted'
    extracted.mkdir()
    (extracted / 'source.rs').write_text('new source')
    assert not (cache / 'registry/src/registry-id/newly-extracted').exists()
`;
  const result = spawnSync('python3', ['-c', source], { cwd: web, encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
});
