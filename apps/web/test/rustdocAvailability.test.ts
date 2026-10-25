import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { availableRustdocManifest } from '../scripts/rustdoc-availability';

test('partial Rustdoc rebuilds retain only intact profile artifacts and their original build provenance', () => {
  const source = String.raw`
import importlib.util, pathlib, tempfile
spec = importlib.util.spec_from_file_location('handbook_rustdoc', 'scripts/build-rustdoc.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
with tempfile.TemporaryDirectory() as temporary:
    output = pathlib.Path(temporary) / 'rustdoc'
    profile = output / 'llm-core'
    landing = profile / 'lingxi_llm_client/index.html'
    landing.parent.mkdir(parents=True)
    landing.write_text('<h1>original compiled crate</h1>')
    auxiliary = profile / 'search-index.js'
    auxiliary.write_text('original search index')
    saved = dict(id='llm-core', state='ready', url='/rustdoc/llm-core/lingxi_llm_client/index.html',
                 sourceFingerprint='previous-verified-snapshot', artifactFingerprint=module.artifact_fingerprint(profile))
    retained = module.retained_profile(saved, output)
    assert retained == saved
    assert retained['sourceFingerprint'] == 'previous-verified-snapshot', 'Do not relabel a retained build as current'
    auxiliary.write_text('stale search index from another release')
    assert module.retained_profile(saved, output) is None, 'A valid landing cannot conceal stale auxiliary files'
    auxiliary.write_text('original search index')
    assert module.retained_profile(saved, output) == saved
    assert module.retained_profile({key: value for key, value in saved.items() if key != 'artifactFingerprint'}, output) is None
    assert module.retained_profile(dict(saved, url='/rustdoc/llm-core/../../outside.html'), output) is None
    landing.unlink()
    assert module.retained_profile(saved, output) is None
`;
  const result = spawnSync('python3', ['-c', source], { cwd: path.resolve(import.meta.dirname, '..'), encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
});

test('saved Rustdoc evidence cannot expose links to artifacts absent from a clone or static bundle', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'lingxi-rustdoc-availability-'));
  try {
    assert.deepEqual(await availableRustdocManifest(root), { profiles: [] });
    const url = '/rustdoc/llm-core/lingxi_llm_client/index.html';
    await writeFile(path.join(root, 'rustdoc-manifest.json'), JSON.stringify({ sourceFingerprint: 'snapshot', profiles: [{ id: 'llm-core', state: 'ready', url }, { id: 'unsafe', state: 'ready', url: '/../../outside.html' }] }));
    const absent = await availableRustdocManifest(root);
    assert.ok(absent.profiles.every((profile) => profile.state === 'unavailable' && !profile.url));
    await mkdir(path.dirname(path.join(root, url)), { recursive: true });
    await writeFile(path.join(root, url), '<h1>lingxi_llm_client</h1>');
    const ready = await availableRustdocManifest(root);
    assert.equal(ready.profiles[0].url, url);
    assert.equal(ready.profiles[0].state, 'ready');
    assert.equal(ready.profiles[1].state, 'unavailable');
  } finally { await rm(root, { recursive: true, force: true }); }
});
