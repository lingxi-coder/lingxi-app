import vectors from '../../../packages/teams/contracts/execution-vectors.json';
import { isMemberExecutionCommand } from '../../../packages/teams/execution-validation';
import assert from 'node:assert/strict';
import test from 'node:test';
import { executionCommand, sharedBotUsable, type TeamTaskView } from '../../../packages/teams/execution';
import { TeamMessaging, pendingPayload, projectionKey } from '../../../packages/teams/messages';
import type { MessageCommand, PendingCommands } from '../../../packages/teams/messages';
import { FakeTeams, botCard, conversation, memoryStorage, until } from './support/fakeTeams';
const scope = { teamId: 'test-team', generation: 7 };
function fixture() {
  const fake = new FakeTeams(), card = botCard();
  fake.conversations.set(scope.teamId, [conversation('test-chat', { kind: 'bot_dm', participants: ['user:alice', `shared_bot:${card.sharedBotId}`] })]);
  const storage = memoryStorage(), model = new TeamMessaging(fake.api, storage, 'alice', scope);
  return { fake, card, storage, model };
}
test('New command envelope separates stable shared identity from local instances', () => {
  const payload = { conversationId: 'test-chat', messageId: 'message001', changeId: 'change001', text: 'Prepare a trip', targets: ['shared-123'] };
  assert.deepEqual(executionCommand(scope, 'submit', payload, 'command001'), { schemaVersion: 1, commandId: 'command001', ...scope, action: 'submit', payload });
  assert.equal(sharedBotUsable(botCard()), true); assert.equal(sharedBotUsable(botCard({ state: 'paused' })), false); assert.equal(sharedBotUsable(botCard({ permitted: false })), false);
});
test('Bot DM submits raw shared IDs; human edits keep existing mutation infrastructure', async context => {
  const { fake, card, model } = fixture(); context.after(() => model.close()); await model.start(); await until(() => !model.offline); model.select('test-chat');
  await model.queue('create', 'Help me pack'); await until(() => fake.commands.some(c => c.action === 'submit'));
  const submit = fake.commands.find(c => c.action === 'submit')!; assert.equal(submit.action, 'submit'); if (submit.action !== 'submit') return;
  assert.deepEqual(submit.payload.targets, [card.sharedBotId]); assert.equal(submit.schemaVersion, 1);
  await model.queue('edit', 'Revised', 'message001'); await until(() => fake.commands.some(c => c.action === 'mutate')); assert.equal('targets' in fake.commands.find(c => c.action === 'mutate')!, false);
  await assert.rejects(model.queue('edit', 'No new run', 'message001', undefined, [card.sharedBotId]), /INVALID_BOT_TARGET/);
});
test('ACK loss and restart retry exact persisted submit identity and payload', async context => {
  const { fake, card, storage, model } = fixture(); context.after(() => model.close()); let drop = true; const calls: MessageCommand[] = [];
  fake.messages = async <T>(command: MessageCommand) => { calls.push(structuredClone(command)); if (drop) throw new Error('SERVICE_UNAVAILABLE'); return {} as T; };
  await model.start(); await until(() => !model.offline); model.select('test-chat'); const targets = [card.sharedBotId]; await model.queue('create', 'Exact input', undefined, undefined, targets); targets.length = 0;
  await until(() => model.state.outbox[0]?.retryDelay !== undefined); model.close(); drop = false;
  const saved = structuredClone([...storage.records.values()][0]!); assert.equal(saved.version, 2); assert.equal(saved.outbox[0]!.command.action, 'submit'); assert.equal('threads' in saved, false);
  const future = Date.now() + 5000; context.mock.method(Date, 'now', () => future);
  const second = new TeamMessaging(fake.api, storage, 'alice', scope); context.after(() => second.close()); await second.start(); await until(() => !second.state.outbox.length && calls.length === 2);
  assert.deepEqual(calls[1], calls[0]); const payload = pendingPayload(saved.outbox[0]!); assert.deepEqual('targets' in payload ? payload.targets : [], [card.sharedBotId]);
});
test('Shared conversation creation persists new envelope with raw shared IDs', async context => {
  const { fake, card, storage, model } = fixture(); context.after(() => model.close()); fake.messages = async () => { throw new Error('SERVICE_UNAVAILABLE'); };
  await model.start(); await model.create('bot_dm', 'Trip assistant', ['alice'], [card.sharedBotId]); const saved = [...storage.records.values()][0]!.creation!;
  assert.equal(saved.command.action, 'createConversation'); const payload = pendingPayload(saved); assert.deepEqual('sharedBotIds' in payload ? payload.sharedBotIds : [], [card.sharedBotId]); assert.equal(JSON.stringify(saved).includes('botKey'), false);
});
test('Retired projection and stale membership are discarded without replay or conversion', async context => {
  for (const old of [{ version: 1, uid: 'alice', ...scope, answerOutbox: [{ action: 'botAnswer' }], outbox: [{ action: 'recoverBot' }] }, { version: 2, uid: 'alice', ...scope, generation: 8, outbox: [{ command: { action: 'submit' } }] }]) {
    const { fake, storage, model } = fixture(); context.after(() => model.close()); storage.records.set(projectionKey('alice', scope), old as unknown as PendingCommands); await model.start(); await model.refresh(); assert.equal(fake.commands.length, 0); assert.deepEqual(model.state.outbox, []); assert.equal(model.state.version, 2); model.close();
  }
});
test('Quota and host failures are permanent pending errors and never automatically resubmitted', async context => {
  for (const code of ['REQUEST_QUOTA_EXCEEDED', 'HOST_UNAVAILABLE']) {
    const { fake, model } = fixture(); context.after(() => model.close()); let calls = 0; fake.messages = async () => { calls++; throw new Error(code); };
    await model.start(); model.select('test-chat'); await model.queue('create', 'Try once'); await until(() => !!model.state.outbox[0]?.error); const command = structuredClone(model.state.outbox[0]!.command); await model.refresh(); assert.equal(calls, 1); assert.deepEqual(model.state.outbox[0]!.command, command); assert.equal(model.state.outbox[0]!.error, code); model.close();
  }
});

test('host-unavailable messages can be retried explicitly with their original command identity', async context => {
  const { fake, model } = fixture(); context.after(() => model.close());
  const calls: MessageCommand[] = []; let hostReady = false;
  fake.messages = async <T>(command: MessageCommand) => { calls.push(structuredClone(command)); if (!hostReady) throw new Error('HOST_UNAVAILABLE'); return {} as T; };
  await model.start(); model.select('test-chat'); await model.queue('create', 'Please retry');
  await until(() => model.state.outbox[0]?.error === 'HOST_UNAVAILABLE', 'host failure');
  const failed = structuredClone(model.state.outbox[0]!.command);
  await model.refresh(); assert.equal(calls.length, 1, 'a failed host does not retry without the user');
  hostReady = true; await model.retryHostUnavailable(pendingPayload(model.state.outbox[0]!).changeId);
  await until(() => calls.length === 2 && model.state.outbox.length === 0, 'explicit retry succeeds');
  assert.deepEqual(calls[1], failed, 'the same message and command identities are retained');
});

test('Shared member boundary vectors accept only the new strict envelope', () => { for (const vector of vectors.commands) assert.equal(isMemberExecutionCommand(vector.input), vector.valid, vector.name); });

test('IndexedDB upgrade deletes only the retired command store and preserves unrelated stores', async context => {
  const { indexedDbCommandStorage } = await import('../../../packages/teams/command-storage');
  const stores = new Set(['projections', 'protected-sentinel']);
  const deleted: string[] = [], opened: unknown[] = [];
  const factory = { open(name: string, version: number) {
    opened.push([name, version]);
    const request: Record<string, any> = {};
    request.result = { objectStoreNames: { contains: (key: string) => stores.has(key) },
      deleteObjectStore(key: string) { deleted.push(key); stores.delete(key); }, createObjectStore(key: string) { stores.add(key); }, close() {},
      transaction(store: string) {
        assert.equal(store, 'commands-v2'); const tx: Record<string, any> = {};
        tx.objectStore = () => ({ get() { const record = { result: undefined }; queueMicrotask(() => tx.oncomplete()); return record; } }); return tx;
      } };
    queueMicrotask(() => { request.onupgradeneeded(); request.onsuccess(); }); return request;
  } };
  const before = Object.getOwnPropertyDescriptor(globalThis, 'indexedDB');
  Object.defineProperty(globalThis, 'indexedDB', { configurable: true, value: factory }); context.after(() => { if (before) Object.defineProperty(globalThis, 'indexedDB', before); else Reflect.deleteProperty(globalThis, 'indexedDB'); });
  assert.equal(await indexedDbCommandStorage().load('account-team-generation'), undefined);
  assert.deepEqual(opened, [['lingxi-team-messages', 3]]); assert.deepEqual(deleted, ['projections']); assert.deepEqual([...stores], ['protected-sentinel', 'commands-v2']);
});
test('durable wait member commands are strict and host-only commands are not member commands', () => {
  const answer = { conversationId: 'chat_0001', waitId: 'wait_0001', messageId: 'message_0002', changeId: 'change_0002', text: 'Lisbon' };
  assert.deepEqual(executionCommand(scope, 'answerWait', answer, 'command001'), { schemaVersion: 1, commandId: 'command001', ...scope, action: 'answerWait', payload: answer });
  assert.deepEqual(executionCommand(scope, 'cancelTask', { taskId: 'task_0001' }, 'command002').payload, { taskId: 'task_0001' });
  assert.throws(() => executionCommand(scope, 'answerWait', { ...answer, text: '' }, 'command001'));
  assert.throws(() => executionCommand(scope, 'answerWait', { ...answer, text: 'x'.repeat(8193) }, 'command001'));
  assert.throws(() => executionCommand(scope, 'answerWait', { ...answer, extra: 1 } as never, 'command001'));
  assert.throws(() => executionCommand(scope, 'cancelTask', { taskId: 'short' }, 'command001'));
  for (const action of ['pauseTask', 'continueTask', 'startTask', 'completeTask']) {
    assert.equal(isMemberExecutionCommand({ schemaVersion: 1, commandId: 'command001', ...scope, action, payload: { taskId: 'task_0001' } }), false, action);
  }
  const view: TeamTaskView = { taskId: 'task_0001', conversationId: 'chat_0001', sharedBotId: 'sharedbot_01', requester: { uid: 'alice', generation: 1 },
    state: 'waiting', createdAt: 1, deadline: 2, epoch: 1, activeSpentMs: 1000,
    wait: { waitId: 'wait_0001', kind: 'human_input', recipient: { uid: 'alice', generation: 1 }, expiresAt: 2, messageId: 'bot_message' } };
  const cancelled: TeamTaskView = { ...view, state: 'cancelled', cancelRequestedAt: 3, wait: undefined };
  assert.equal(view.wait?.kind, 'human_input'); assert.equal(cancelled.state, 'cancelled');
});

test('An answer to a Bot question is a persisted answerWait, never a submit, and a retry after a lost response reuses its ids', async context => {
  const { fake, storage, model } = fixture(); context.after(() => model.close()); let drop = true; const calls: MessageCommand[] = [];
  fake.messages = async <T>(command: MessageCommand) => { calls.push(structuredClone(command)); if (drop) throw new Error('SERVICE_UNAVAILABLE'); return {} as T; };
  await model.start(); await until(() => !model.offline); model.select('test-chat');
  await model.answer('wait-test', 'Lisbon');
  await until(() => model.state.outbox[0]?.retryDelay !== undefined); model.close(); drop = false;
  const saved = structuredClone([...storage.records.values()][0]!); const stored = saved.outbox[0]!.command;
  assert.equal(stored.action, 'answerWait', 'a Bot DM would otherwise submit its text to the Bot');
  const future = Date.now() + 5000; context.mock.method(Date, 'now', () => future);
  const second = new TeamMessaging(fake.api, storage, 'alice', scope); context.after(() => second.close()); await second.start(); await until(() => !second.state.outbox.length && calls.length === 2);
  assert.deepEqual(calls[1], calls[0]); assert.deepEqual(calls.map(c => c.action), ['answerWait', 'answerWait']);
  const payload = pendingPayload(saved.outbox[0]!); assert.ok('waitId' in payload && payload.waitId === 'wait-test' && payload.messageId !== payload.changeId);
});
test('A refused answer is thrown to the caller and leaves nothing queued; other failures stay queued', async context => {
  for (const code of ['WAIT_CLOSED', 'WAIT_RECIPIENT_REQUIRED', 'WAIT_REQUIRED', 'TASK_REVOKED']) {
    const { fake, model } = fixture(); const calls: MessageCommand[] = [];
    fake.messages = async (command: MessageCommand) => { calls.push(command); throw new Error(code); };
    await model.start(); model.select('test-chat');
    await assert.rejects(model.answer('wait-test', 'Lisbon'), new RegExp(code)); assert.equal(model.state.outbox.length, 0, code); assert.equal(calls.length, 1, `${code} is not retried`); model.close();
  }
  const { fake, model } = fixture(); context.after(() => model.close()); fake.messages = async () => { throw new Error('MESSAGE_ID_CONFLICT'); };
  await model.start(); model.select('test-chat'); await model.answer('wait-test', 'Lisbon');
  assert.equal(model.state.outbox[0]?.error, 'MESSAGE_ID_CONFLICT', 'an id conflict is kept for the person to discard, like any rejected message');
  await assert.rejects(model.answer('wait-test', '  '), /INVALID_INPUT/);
});
test('The message model carries a waitId from Firestore, and the client has the two new commands and errors', async () => {
  const { firestoreMessage } = await import('../../../packages/teams/firestore');
  const base = { seq: 2, author: { kind: 'user', uid: 'alice' }, kind: 'text', text: 'Lisbon', changeSeq: 2, changeId: 'change-2', createdAt: null };
  assert.equal(firestoreMessage('message-2', { ...base, waitId: 'wait-test', replyTo: 'message-1' }).waitId, 'wait-test');
  assert.equal('waitId' in firestoreMessage('message-2', base), false);
  const { messageErrorText } = await import('../../../packages/teams/errors');
  for (const code of ['WAIT_CLOSED', 'WAIT_RECIPIENT_REQUIRED', 'WAIT_REQUIRED', 'TASK_REVOKED', 'TASK_CANCEL_FORBIDDEN']) for (const zh of [false, true]) assert.doesNotMatch(messageErrorText(code, zh), /Could not sync|消息同步未完成/, `${code} has its own text`);
});
