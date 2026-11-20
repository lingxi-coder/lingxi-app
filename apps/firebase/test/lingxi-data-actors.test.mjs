import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import assert from 'node:assert/strict';
import test from 'node:test';

import { createDataActorProjection } from '../src/lingxi-data-actors.mjs';

test('overlapping operations cannot overwrite a recipient actor or skip a renewed authority check', async t => {
  const folder = await mkdtemp(join(tmpdir(), 'lingxi-data-actors-'));
  t.after(() => rm(folder, { recursive: true, force: true }));
  const path = join(folder, 'actors.json');
  const run = await createDataActorProjection(path);
  const snapshot = async () => JSON.parse(await readFile(path, 'utf8'));
  const scope = { kind: 'team', teamId: 'team-a', generation: 7 };
  const pause = Promise.withResolvers();
  const prepared = Promise.withResolvers();
  const events = [];
  assert.deepEqual(await snapshot(), {});
  const share = run('owner', scope, async () => {}, async () => {
    prepared.resolve();
    await pause.promise;
    assert.deepEqual(await snapshot(), {
      owner: { id: 'owner', teams: { 'team-a': 7 } },
      reader: { id: 'reader', teams: { 'team-a': 9 } },
    });
    events.push('share finished');
  }, [{ uid: 'reader', generation: 9 }]);
  await prepared.promise;
  const next = run('reader', { kind: 'personal' }, async () => {
    events.push('rechecked after share');
    throw new Error('revoked');
  }, () => assert.fail('revoked reader was admitted'));
  pause.resolve();
  await share;
  await assert.rejects(next, /revoked/u);
  assert.deepEqual(events, ['share finished', 'rechecked after share']);
  assert.deepEqual(await snapshot(), {});
});
