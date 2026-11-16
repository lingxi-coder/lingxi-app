import test from 'node:test';
import assert from 'node:assert/strict';
import { admissionId, sharedBotRecord, deploymentRecord, sharedBotPermits, buildChatInputBatch } from '../src/team-execution-contract.mjs';
const bot = { schemaVersion: 1, sharedBotId: 'sharedbot_01', teamId: 'team_0001', maintainer: { uid: 'owner', generation: 1 }, revision: 1, state: 'published', usePolicy: { mode: 'listed', members: [{ uid: 'member', generation: 2 }] }, activeDeploymentId: 'deployment_01' };
const message = seq => ({ messageId: `message_${seq.toString().padStart(4, '0')}`, seq, changeSeq: seq, changeId: `change_${seq.toString().padStart(4, '0')}`, author: { kind: 'user', uid: seq % 2 ? 'alice' : 'bob', displayName: 'Member' }, createdAt: 1000 + seq, text: `${seq}:${'x'.repeat(1024)}` });
const batch = (from, to, bootstrap = false) => ({ schemaVersion: 1, batchId: 'batch_0001', teamId: 'team_0001', sharedBotId: bot.sharedBotId, conversationId: 'chat_0001', sessionEpoch: 1, expectedSessionRevision: 0, fromSeqExclusive: from, toSeqInclusive: to, lastChangeSeq: to, bootstrap, currentMessageId: message(to).messageId, messages: Array.from({ length: to - from }, (_, index) => message(from + index + 1)), changes: [] });
test('listed use requires the current membership generation, including the maintainer', () => {
  assert.equal(sharedBotPermits(bot, { uid: 'member', generation: 2 }), true);
  assert.equal(sharedBotPermits(bot, { uid: 'member', generation: 3 }), false);
  assert.equal(sharedBotPermits(bot, bot.maintainer), false);
  assert.equal(sharedBotPermits({ ...bot, state: 'paused' }, { uid: 'member', generation: 2 }), false);
});
test('new identities reject owner-host keys and compatibility fields', () => {
  assert.throws(() => sharedBotRecord.parse({ ...bot, sharedBotId: 'bot:owner:host:local' }));
  assert.throws(() => sharedBotRecord.parse({ ...bot, legacyBotKey: 'old' }));
  assert.throws(() => sharedBotRecord.parse({ ...bot, executionProtocol: 'v1' }));
  assert.throws(() => sharedBotRecord.parse({ ...bot, usePolicy: { mode: 'listed', members: [bot.maintainer, bot.maintainer] } }));
});
test('deployment accepts only execution 2, boolean durable-wait and write-operation capabilities and no later ones', () => {
  const deployment = { deploymentId: 'deployment_01', sharedBotId: bot.sharedBotId, revision: 1, hostId: 'host_0001', owner: bot.maintainer, instanceId: 'instance_01', definitionDigest: 'a'.repeat(64), capabilitySnapshotDigest: 'b'.repeat(64), executionVersion: 2, capabilities: { durableWait: false, writeOperations: false, delegation: false }, accountEpoch: 1, enabled: true };
  deploymentRecord.parse(deployment);
  assert.throws(() => deploymentRecord.parse({ ...deployment, executionVersion: 1 }));
  deploymentRecord.parse({ ...deployment, capabilities: { ...deployment.capabilities, durableWait: true } });
  assert.throws(() => deploymentRecord.parse({ ...deployment, capabilities: { ...deployment.capabilities, durableWait: 'yes' } }));
  // The operation ledger (M2 Q04) made writes a capability a host declares.
  deploymentRecord.parse({ ...deployment, capabilities: { ...deployment.capabilities, writeOperations: true } });
  assert.throws(() => deploymentRecord.parse({ ...deployment, capabilities: { ...deployment.capabilities, writeOperations: 'yes' } }));
  assert.throws(() => deploymentRecord.parse({ ...deployment, capabilities: { ...deployment.capabilities, delegation: true } }));
});
test('admission identity is stable per message and target with unambiguous framing', () => {
  assert.equal(admissionId('message_0001', 'sharedbot_01'), admissionId('message_0001', 'sharedbot_01'));
  assert.notEqual(admissionId('message_0001', 'sharedbot_01'), admissionId('message_0001', 'sharedbot_02'));
  assert.throws(() => admissionId('old:key', 'sharedbot_01'));
});
test('first batch preserves latest 200 source identities, times and order', () => {
  const input = batch(50, 250, true), result = buildChatInputBatch(input);
  assert.equal(result.messages.length, 200);
  assert.deepEqual(result.messages, input.messages);
  assert.throws(() => buildChatInputBatch(batch(0, 201, true)));
  assert.throws(() => buildChatInputBatch(batch(150, 250, true)));
  assert.throws(() => buildChatInputBatch(batch(1, 3, true)));
});
test('450-message delta above the old frame limit is complete without truncation', () => {
  const input = batch(200, 650), result = buildChatInputBatch(input);
  assert.ok(Buffer.byteLength(JSON.stringify(result)) > 256 * 1024);
  assert.equal(result.messages.length, 450);
  assert.deepEqual(result.messages, input.messages);
});
test('missing, duplicate, reordered, future and mismatched request records are rejected', () => {
  const input = batch(10, 13);
  for (const messages of [input.messages.slice(1), [...input.messages].reverse(), [input.messages[0], input.messages[0], input.messages[2]]]) assert.throws(() => buildChatInputBatch({ ...input, messages }));
  assert.throws(() => buildChatInputBatch({ ...input, currentMessageId: 'message_9999' }));
  assert.throws(() => buildChatInputBatch({ ...input, lastChangeSeq: 12 }));
});
