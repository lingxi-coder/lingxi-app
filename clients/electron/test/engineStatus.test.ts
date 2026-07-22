import test from 'node:test';
import assert from 'node:assert/strict';

import { engineLaunchStatus } from '../src/renderer/bridge/engineStatus';

test('engine launch status exposes visible progress for each startup phase', () => {
  assert.deepEqual(engineLaunchStatus({ status: 'spawning' }), {
    phase: 'starting',
    label: 'Starting engine',
    detail: 'Preparing the signed local engine…',
    percent: 32,
    active: true,
    canStart: false,
  });
  assert.equal(engineLaunchStatus({ status: 'connecting' }).percent, 72);
  assert.equal(engineLaunchStatus({ status: 'connected' }).percent, 100);
  assert.equal(engineLaunchStatus({ status: 'error', message: 'failed' }).canStart, true);
});
