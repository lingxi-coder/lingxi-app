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

test('delegation (B8): a publication says who may hand it a request and whom it may hand one to, and the two lists go with their settings', () => {
  const parse = extra => sharedBotCommand.parse(command('publishSharedBot', { ...publication, ...extra })).payload;
  const plain = parse({});
  for (const field of ['acceptDelegation', 'acceptFrom', 'delegateTo', 'delegation']) assert.equal(plain[field], undefined, field);
  for (const acceptDelegation of ['none', 'team_bots']) assert.equal(parse({ acceptDelegation }).acceptDelegation, acceptDelegation);
  assert.deepEqual(parse({ acceptDelegation: 'listed', acceptFrom: ['sharedbot_02'] }).acceptFrom, ['sharedbot_02']);
  assert.deepEqual(parse({ delegateTo: ['sharedbot_02', 'sharedbot_03'], delegation: { daily: 5 } }).delegateTo, ['sharedbot_02', 'sharedbot_03']);
  for (const extra of [
    { acceptDelegation: 'everyone' }, { acceptDelegation: 'listed' }, { acceptDelegation: 'none', acceptFrom: ['sharedbot_02'] },
    { acceptDelegation: 'team_bots', acceptFrom: ['sharedbot_02'] }, { acceptDelegation: 'listed', acceptFrom: [] },
    { acceptDelegation: 'listed', acceptFrom: ['sharedbot_02', 'sharedbot_02'] },
    { delegateTo: ['sharedbot_02'] }, { delegation: { daily: 5 } }, { delegateTo: [], delegation: { daily: 5 } },
    { delegateTo: ['sharedbot_01'], delegation: { daily: 5 } }, { delegateTo: ['sharedbot_02'], delegation: { daily: 0 } },
    { delegateTo: ['sharedbot_02'], delegation: { daily: 5, extra: 1 } },
    { delegateTo: Array.from({ length: 21 }, (_, i) => `sharedbot_${String(i + 2).padStart(2, '0')}`), delegation: { daily: 5 } },
  ]) assert.throws(() => parse(extra), JSON.stringify(extra));
});

test('delegation (B8): a run delegates with delegateTask, or pauses with a delegation; the request is bounded text and the step is the call\'s', () => {
  const call = { ...host, taskId: 'task_0001', logicalStepId: 'task_0001/t0/c0', target: 'sharedbot_02', request: 'Find the 2026 figures.' };
  const send = { ...call, permitId: 'permit_0001', inputGrantIds: [], outputAudience: { mode: 'published_inputs' } };
  sharedBotCommand.parse(command('delegateTask', send));
  sharedBotCommand.parse(command('detachDelegation', { ...host, taskId: 'task_0001', waitId: 'wait_0001' }));
  const { logicalStepId, target, request } = call;
  const pause = { ...host, taskId: 'task_0001', permitId: 'permit_0001', waitId: 'wait_0001', requestDigest: 'a'.repeat(64), checkpointDigest: 'b'.repeat(64),
    pausedAt: 1, expiresInMs: 600000, dispatchedRequests: 1, inputGrantIds: [], outputAudience: { mode: 'published_inputs' }, kind: 'delegation', delegation: { logicalStepId, target, request } };
  assert.equal(sharedBotCommand.parse(command('pauseTask', pause)).payload.delegation.target, 'sharedbot_02');
  for (const bad of [{ request: '' }, { request: '  ' }, { request: 'x'.repeat(2001) }, { request: 'a\0b' }, { target: '' }, { logicalStepId: 'has space' }, { expectReply: true }, { extra: 1 }]) {
    assert.throws(() => sharedBotCommand.parse(command('delegateTask', { ...send, ...bad })), JSON.stringify(bad));
  }
  sharedBotCommand.parse(command('delegateTask', { ...send, request: 'é'.repeat(1000) }));
  assert.throws(() => sharedBotCommand.parse(command('delegateTask', { ...send, request: 'é'.repeat(1001) })));
  assert.throws(() => sharedBotCommand.parse(command('pauseTask', { ...pause, delegation: { ...pause.delegation, request: '' } })));
  assert.throws(() => sharedBotCommand.parse(command('pauseTask', { ...pause, delegation: undefined })));
  assert.throws(() => sharedBotCommand.parse(command('pauseTask', { ...pause, question: 'Which one?' })));
  assert.throws(() => sharedBotCommand.parse(command('detachDelegation', { ...host, taskId: 'task_0001' })));
});
