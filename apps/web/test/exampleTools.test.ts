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
snapshot.dependency_mode = 'workspace'
snapshot.source_mode = 'working-tree'
snapshot.expected = snapshot._expected_packages()
registry = 'registry+https://github.com/rust-lang/crates.io-index'

def package(name, version, manifest, source=None):
    return dict(name=name, version=version, manifest_path=str(manifest), source=source,
                id=f'{name}@{version}:{manifest}:{source}')

local_core = package('sdk-core', '1.2.3', core)
local_nix = package('nix', '0.31.3', vendored)

def registry_nix(version):
    return package('nix', version, root / f'registry/nix-{version}/Cargo.toml', registry)

def verify(packages, owner_repo=None):
    metadata = dict(packages=packages, resolve=dict(nodes=[dict(id=p['id']) for p in packages]))
    sdk_tools.command = lambda *args, **kwargs: types.SimpleNamespace(stdout=json.dumps(metadata))
    return snapshot.verify_metadata(root / 'consumer/Cargo.toml', owner_repo=owner_repo)

def rejected(packages, identity, owner_repo=None):
    try:
        verify(packages, owner_repo)
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

const declaredIdentityFixture = String.raw`
pinned = 'ff71b2ec1139bc67cc8719d78b1735aca967bbe8'
unpublished = 'ad8c60f00000000000000000000000000000000000'
url = 'https://github.com/lingxi-coder/llm-client.git'
(sdk_root / 'Cargo.toml').write_text(f'''[workspace]
[workspace.package]
version = "1.2.3"
[workspace.dependencies]
lingxi-llm-client = {{ git = "{url}", rev = "{pinned}" }}
''')
llm = root / 'llm-client/Cargo.toml'
llm.parent.mkdir()
llm.write_text('[package]\nname = "lingxi-llm-client"\nversion = "0.3.0"\n')
hyper = root / 'llm-client/vendor/lingxi-hyper/Cargo.toml'
hyper.parent.mkdir(parents=True)
hyper.write_text('[package]\nname = "lingxi-hyper"\nversion = "1.0.0"\n')
snapshot.expected = snapshot._expected_packages()
snapshot.dependency_mode = 'declared'
git_source = f'git+{url}?rev={pinned}#{pinned}'
git_llm = package('lingxi-llm-client', '0.3.0', root / 'cargo-cache/checkout/Cargo.toml', git_source)
git_hyper = package('lingxi-hyper', '1.0.0', root / 'cargo-cache/checkout/vendor/lingxi-hyper/Cargo.toml', git_source)
`;

test('declared dependencies preserve the owner pin and its Git packages instead of the local SDK overlay', () => {
  checkMetadataIdentity(`${declaredIdentityFixture}\n${String.raw`
identities = verify([local_core, git_llm, git_hyper, registry_nix('0.27.1')], 'harness-runtime')
assert len(identities) == 3
dependency = next(entry for entry in identities if entry['package'] == 'lingxi-llm-client')
assert dependency == dict(package='lingxi-llm-client', manifest='llm-client::package/lingxi-llm-client', source='git', revision=pinned)
assert str(root) not in json.dumps(identities), 'Manifest identities must not reveal Cargo cache paths'
assert next(entry for entry in identities if entry['package'] == 'sdk-core')['source'] == 'working-tree'
snapshot.source_mode = 'committed'
snapshot.source_revisions = {'harness-runtime': 'a' * 40}
assert next(entry for entry in verify([local_core, git_llm], 'harness-runtime') if entry['package'] == 'sdk-core')['revision'] == 'a' * 40
`}`);
});

test('declared dependency validation rejects wrong requested or resolved revisions, URLs and local SDK substitutions', () => {
  checkMetadataIdentity(`${declaredIdentityFixture}\n${String.raw`
for source in [
    f'git+{url}?rev={pinned}#{unpublished}',
    f'git+{url}?rev={unpublished}#{pinned}',
    f'git+https://github.com/other/llm-client.git?rev={pinned}#{pinned}',
    f'git+{url}?branch=main#{pinned}',
    f'git+{url}?rev={pinned[:7]}#{pinned}',
]:
    rejected([local_core, dict(git_llm, source=source)], 'lingxi-llm-client@0.3.0', 'harness-runtime')
rejected([local_core, package('lingxi-llm-client', '0.3.0', llm)], 'lingxi-llm-client@0.3.0', 'harness-runtime')
rejected([local_core, package('unknown-git-package', '1.0.0', root / 'cache/unknown/Cargo.toml', f'git+https://github.com/other/unknown.git?rev={pinned}#{pinned}')], 'unknown-git-package@1.0.0', 'harness-runtime')
rejected([dict(local_core, source=git_source), git_llm], 'sdk-core@1.2.3', 'harness-runtime')
snapshot.dependency_mode = 'workspace'
rejected([local_core, git_llm], 'lingxi-llm-client@0.3.0')
assert verify([local_core, package('lingxi-llm-client', '0.3.0', llm)])[0]['source'] == 'working-tree'
`}`);
});

test('LLM-owned profiles require the local LLM snapshot and do not inherit Harness Git permissions', () => {
  checkMetadataIdentity(`${declaredIdentityFixture}\n${String.raw`
local_llm = package('lingxi-llm-client', '0.3.0', llm)
assert verify([local_llm], 'llm-client')[0]['manifest'] == 'llm-client/Cargo.toml'
rejected([git_llm], 'lingxi-llm-client@0.3.0', 'llm-client')
rejected([local_llm, package('sdk-core', '1.2.3', core)], 'sdk-core@1.2.3', 'llm-client')
rejected([local_llm, package('unknown-git-package', '1.0.0', root / 'cache/unknown/Cargo.toml', git_source)], 'unknown-git-package@1.0.0', 'llm-client')
`}`);
});

test('Mobile-owned profiles validate their snapshot and their own declared dependencies', () => {
  checkMetadataIdentity(`${declaredIdentityFixture}\n${String.raw`
mobile = root / 'mobile-linux-runtime/Cargo.toml'
mobile.parent.mkdir()
mobile.write_text('[package]\nname = "mobile-linux-api"\nversion = "0.1.0"\n')
snapshot.expected = snapshot._expected_packages()
local_mobile = package('mobile-linux-api', '0.1.0', mobile)
assert verify([local_mobile], 'mobile-linux-runtime')[0]['manifest'] == 'mobile-linux-runtime/Cargo.toml'
rejected([local_mobile, git_llm], 'lingxi-llm-client@0.3.0', 'mobile-linux-runtime')
mobile.write_text(mobile.read_text() + f'\n[dependencies]\nclient = {{ package = "lingxi-llm-client", git = "{url}", rev = "{pinned}" }}\n')
identities = verify([local_mobile, git_llm], 'mobile-linux-runtime')
assert next(item for item in identities if item['package'] == 'lingxi-llm-client')['revision'] == pinned
`}`);
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

test('committed SDK snapshots preserve exact Git blobs and submodule pins while ignoring concurrent edits', () => {
  const source = String.raw`
import os, pathlib, subprocess, sys, tempfile
sys.path.insert(0, str(pathlib.Path('examples').resolve()))
from sdk_tools import snapshot_repo

def git(repo, *args):
    env = os.environ.copy()
    env.update(GIT_AUTHOR_NAME='Snapshot fixture', GIT_AUTHOR_EMAIL='fixture@example.invalid',
               GIT_COMMITTER_NAME='Snapshot fixture', GIT_COMMITTER_EMAIL='fixture@example.invalid')
    return subprocess.check_output(['git', '-C', str(repo), '-c', 'commit.gpgsign=false', *args], env=env, stderr=subprocess.DEVNULL).decode().strip()

with tempfile.TemporaryDirectory() as temp:
    root = pathlib.Path(temp)
    child = root / 'child-source'
    child.mkdir()
    git(child, 'init', '-q')
    (child / 'child.rs').write_text('pinned child\n')
    git(child, 'add', '.')
    git(child, 'commit', '-qm', 'pinned child')
    child_pin = git(child, 'rev-parse', 'HEAD')
    repo = root / 'repo'
    repo.mkdir()
    git(repo, 'init', '-q')
    (repo / 'source.rs').write_text('committed source\n')
    (repo / 'deleted.rs').write_text('committed deletion target\n')
    (repo / 'source.link').symlink_to('source.rs')
    git(repo, '-c', 'protocol.file.allow=always', 'submodule', 'add', '-q', str(child), 'deps/child')
    git(repo, 'add', '.')
    git(repo, 'commit', '-qm', 'committed snapshot')
    revision = git(repo, 'rev-parse', 'HEAD')
    (repo / 'source.rs').write_text('staged source\n')
    git(repo, 'add', 'source.rs')
    (repo / 'source.rs').write_text('unstaged source\n')
    (repo / 'deleted.rs').unlink()
    (repo / 'untracked.rs').write_text('untracked source\n')
    submodule = repo / 'deps/child'
    (submodule / 'child.rs').write_text('newer child commit\n')
    git(submodule, 'add', '.')
    git(submodule, 'commit', '-qm', 'newer unpinned child')
    assert git(submodule, 'rev-parse', 'HEAD') != child_pin
    (submodule / 'child.rs').write_text('dirty child\n')
    (submodule / 'untracked.rs').write_text('untracked child\n')
    before = git(repo, 'status', '--porcelain=v1')
    before_index = git(repo, 'ls-files', '--stage')
    destination = root / 'committed-copy'
    snapshot_repo(repo, destination, source_mode='committed', revision=revision)
    assert (destination / 'source.rs').read_text() == 'committed source\n'
    assert (destination / 'deleted.rs').read_text() == 'committed deletion target\n'
    assert not (destination / 'untracked.rs').exists()
    assert (destination / 'source.link').is_symlink()
    assert (destination / 'source.link').read_text() == 'committed source\n'
    assert (destination / 'deps/child/child.rs').read_text() == 'pinned child\n'
    assert not (destination / 'deps/child/untracked.rs').exists()
    assert git(repo, 'status', '--porcelain=v1') == before
    assert git(repo, 'ls-files', '--stage') == before_index
    assert (repo / 'source.rs').read_text() == 'unstaged source\n'
    assert (submodule / 'child.rs').read_text() == 'dirty child\n'
    git(repo, 'update-index', '--cacheinfo', '160000', 'f' * 40, 'deps/child')
    git(repo, 'commit', '-qm', 'unavailable child pin')
    try:
        snapshot_repo(repo, root / 'missing-child-copy', source_mode='committed')
    except RuntimeError as error:
        assert 'Pinned submodule commit is unavailable' in str(error), str(error)
    else:
        raise AssertionError('Missing submodule objects must not fall back to checkout HEAD')
`;
  const result = spawnSync('python3', ['-c', source], { cwd: web, encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
});

test('snapshot dependency modes keep reader overlays separate from committed declared Rustdoc inputs', () => {
  const source = String.raw`
import os, pathlib, subprocess, sys, tempfile
sys.path.insert(0, str(pathlib.Path('examples').resolve()))
import sdk_tools

with tempfile.TemporaryDirectory() as temp:
    workspace = pathlib.Path(temp) / 'workspace'
    workspace.mkdir()
    revisions = {}
    for name in sdk_tools.SDKS:
        repo = workspace / name
        repo.mkdir()
        subprocess.run(['git', 'init', '-q', str(repo)], check=True)
        (repo / 'Cargo.toml').write_text(f'[package]\nname = "{name}"\nversion = "1.0.0"\n')
        (repo / 'rust-toolchain.toml').write_text('[toolchain]\nchannel = "1.94.0"\n')
        subprocess.run(['git', '-C', str(repo), 'add', '.'], check=True)
        subprocess.run(['git', '-C', str(repo), '-c', 'user.name=Snapshot fixture', '-c', 'user.email=fixture@example.invalid',
                        '-c', 'commit.gpgsign=false', 'commit', '-qm', 'baseline'], check=True)
        revisions[name] = subprocess.check_output(['git', '-C', str(repo), 'rev-parse', 'HEAD']).decode().strip()
    harness = workspace / 'harness-runtime'
    (harness / 'untracked.rs').write_text('concurrent source\n')
    (harness / 'Cargo.toml').write_text((harness / 'Cargo.toml').read_text() + '\n# concurrent assembly\n')
    sdk_tools.isolated_cargo_home = lambda source, destination: destination.mkdir()
    cargo_home = os.environ.get('CARGO_HOME')
    declared = sdk_tools.SdkSnapshot(workspace, dependency_mode='declared', source_mode='committed')
    try:
        text = (declared.root / 'harness-runtime/Cargo.toml').read_text()
        assert '[patch.' not in text and 'concurrent assembly' not in text
        assert not (declared.root / 'harness-runtime/untracked.rs').exists()
        assert declared.source_revisions == revisions
        assert declared.source_mode == 'committed' and declared.dependency_mode == 'declared'
        declared_fingerprint = declared.source_fingerprint
    finally:
        declared.close()
    assert os.environ.get('CARGO_HOME') == cargo_home
    reader = sdk_tools.SdkSnapshot(workspace)
    try:
        text = (reader.root / 'harness-runtime/Cargo.toml').read_text()
        assert '[patch."https://github.com/lingxi-coder/llm-client.git"]' in text
        assert 'concurrent assembly' in text
        assert (reader.root / 'harness-runtime/untracked.rs').is_file()
        assert reader.source_mode == 'working-tree' and reader.dependency_mode == 'workspace'
        assert reader.source_fingerprint != declared_fingerprint
    finally:
        reader.close()
    assert os.environ.get('CARGO_HOME') == cargo_home
`;
  const result = spawnSync('python3', ['-c', source], { cwd: web, encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
});
