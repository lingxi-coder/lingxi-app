import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { deriveGrantPermit } from '../src/shared-bots.mjs';

const vectors = JSON.parse(readFileSync(new URL('./fixtures/grant-permit-vectors.json', import.meta.url), 'utf8'));
test('grant permit derivation matches fixed host/backend scope, deployment, nonce and epoch fixtures', () => {
  const permits = vectors.cases.map(value => {
    const permit = deriveGrantPermit(value.scopeKind, value.scopeId, value.deploymentId ?? vectors.deploymentId,
      vectors.taskId, value.targetEpoch, value.permitNonce ?? vectors.permitNonce);
    assert.equal(permit, value.permitId);
    return permit;
  });
  assert.equal(new Set(permits).size, permits.length);
  assert.notEqual(deriveGrantPermit('team', 'team_0001', vectors.deploymentId, 'task_0002', 1, vectors.permitNonce), permits[0]);
});
