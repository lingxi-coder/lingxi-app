import assert from 'node:assert/strict';
import test from 'node:test';
import { randomUUID } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getFirestore } from 'firebase-admin/firestore';
import { CatalogStore } from '../src/store.mjs';
import { validateCatalog } from '../src/catalog.mjs';

test('Firestore: sharded payloads, concurrent draft conflict, immutable publication and restore', { skip: !process.env.FIRESTORE_EMULATOR_HOST }, async () => {
  const app = initializeApp({ projectId: 'demo-lingxi-backend' }, randomUUID());
  const db = getFirestore(app, `test-${randomUUID()}`);
  const seed = validateCatalog(JSON.parse(readFileSync(new URL('../src/seed.json', import.meta.url), 'utf8')));
  const store = new CatalogStore(db, seed);
  try {
    const attempts = await Promise.allSettled([store.save(seed, 0, 'admin-a'), store.save(seed, 0, 'admin-b')]);
    assert.equal(attempts.filter((r) => r.status === 'fulfilled').length, 1);
    assert.equal(attempts.find((r) => r.status === 'rejected').reason.status, 409);
    const draft = await store.draft(); assert.deepEqual(draft.catalog, seed); assert.equal(draft.revision, 1);
    const first = await store.publish(1, 'Initial', 'admin');
    const edited = structuredClone(seed); edited.models[0].enabled = !edited.models[0].enabled;
    await store.save(edited, 1, 'admin');
    assert.deepEqual((await store.current()).catalog, seed, 'draft edits must not mutate the published snapshot');
    await assert.rejects(store.publish(1, 'Stale', 'admin'), { status: 409 });
    await store.publish(2, 'Edited', 'admin'); assert.deepEqual((await store.current()).catalog, edited);
    const restored = await store.restore(first.id, 2, 'admin'); assert.equal(restored.revision, 3); assert.deepEqual(restored.catalog, seed);
    assert.deepEqual((await store.current()).catalog, edited, 'restoring a draft must not silently publish it');
    assert.equal((await store.history()).length, 2);
  } finally { await db.terminate(); await deleteApp(app); }
});
