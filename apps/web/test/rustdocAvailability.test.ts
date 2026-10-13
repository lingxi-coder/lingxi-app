import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { availableRustdocManifest } from '../scripts/rustdoc-availability';

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
