import test from 'node:test';
import assert from 'node:assert/strict';
import { sharedBotCommand } from '../src/shared-bot-commands.mjs';

const command = (action, payload) => ({ schemaVersion: 1, commandId: 'command_0001', teamId: 'team_0001', generation: 1, action, payload });
const host = { deploymentId: 'deployment_01', hostId: 'host_0001', accountEpoch: 1 };
const publication = {
  sharedBotId: 'sharedbot_01', expectedRevision: 0, name: 'Assistant', description: '', usePolicy: { mode: 'all' },
  deployment: { ...host, sharedBotId: 'sharedbot_01', revision: 1, owner: { uid: 'owner', generation: 1 }, instanceId: 'instance_01',
    definitionDigest: 'a'.repeat(64), capabilitySnapshotDigest: 'b'.repeat(64), executionVersion: 2,
    capabilities: { durableWait: true, writeOperations: false, delegation: false }, enabled: true },
  inputs: [{ resourceId: 'persona_01', revision: 1, digest: 'a'.repeat(64), kind: 'persona', sourceAudience: { mode: 'team' } }],
  quotas: { team: 100, member: 20, bot: 100 },
};

test('publication examples are optional, trimmed and bounded by count and UTF-8 bytes', () => {
  const parse = examples => sharedBotCommand.parse(command('publishSharedBot', { ...publication, examples })).payload.examples;
  assert.equal(sharedBotCommand.parse(command('publishSharedBot', publication)).payload.examples, undefined);
  assert.deepEqual(parse([]), []);
  assert.deepEqual(parse(['  What is next? \n', '中'.repeat(666) + 'ab', '😀'.repeat(500)]), ['What is next?', '中'.repeat(666) + 'ab', '😀'.repeat(500)]);
  for (const examples of [['a', 'b', 'c', 'd'], ['  \n'], ['a\0b'], ['中'.repeat(667)], ['😀'.repeat(501)], [1], 'prompt']) {
    assert.throws(() => parse(examples));
  }
  assert.throws(() => sharedBotCommand.parse(command('publishSharedBot', { ...publication, capabilities: { durableWait: true } })));
});

test('heartbeat accepts only a generic enumerated cause for a non-ready host, without client timestamps', () => {
  for (const ready of [true, false]) sharedBotCommand.parse(command('heartbeatDeployment', { ...host, ready }));
  for (const healthReason of ['bot', 'runtime', 'model']) {
    sharedBotCommand.parse(command('heartbeatDeployment', { ...host, ready: false, healthReason }));
    assert.throws(() => sharedBotCommand.parse(command('heartbeatDeployment', { ...host, ready: true, healthReason })));
  }
  for (const extra of [{ healthReason: 'credentials' }, { checkedAt: 1 }, { healthUntil: 90001 }, { reason: 'private details' }]) {
    assert.throws(() => sharedBotCommand.parse(command('heartbeatDeployment', { ...host, ready: false, ...extra })));
  }
});

test('host grant nonces are optional 32-byte lowercase hex secrets; callers cannot select a raw permit or overflow the next epoch', () => {
  const start = { ...host, taskId: 'task_0001', expectedLastChangeSeq: 0 };
  const continuation = { ...host, taskId: 'task_0001', expectedEpoch: 1, checkpointDigest: 'a'.repeat(64), reason: 'recovery', dispatchedRequests: 0 };
  for (const [action, payload] of [['startTask', start], ['continueTask', continuation]]) {
    assert.equal(sharedBotCommand.parse(command(action, payload)).payload.permitNonce, undefined);
    sharedBotCommand.parse(command(action, { ...payload, permitNonce: 'ab'.repeat(32) }));
    for (const permitNonce of ['', 'a'.repeat(63), 'a'.repeat(65), 'AB'.repeat(32), 'gg'.repeat(32), 1, null]) {
      assert.throws(() => sharedBotCommand.parse(command(action, { ...payload, permitNonce })));
    }
    assert.throws(() => sharedBotCommand.parse(command(action, { ...payload, permitId: 'caller_selected_permit' })));
  }
  sharedBotCommand.parse(command('continueTask', { ...continuation, expectedEpoch: Number.MAX_SAFE_INTEGER - 1 }));
  for (const expectedEpoch of [Number.MAX_SAFE_INTEGER, Number.MAX_SAFE_INTEGER + 1]) {
    assert.throws(() => sharedBotCommand.parse(command('continueTask', { ...continuation, expectedEpoch })));
  }
});
