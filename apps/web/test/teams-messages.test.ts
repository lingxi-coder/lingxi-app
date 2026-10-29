import assert from 'node:assert/strict';
import test from 'node:test';
import { TeamMessaging, applyMessagePage, emptyProjection, mergeConversation, type MessageApi, type MessageCommand, type MessagePage, type MessageProjection, type MessageStorage, type TeamConversation, type TeamMessage } from '../../../packages/teams/messages';
const scope = { teamId: 'test-team', generation: 1 };
const conversation: TeamConversation = { id: 'test-chat', kind: 'dm', title: 'Fixture', participants: ['user:owner', 'user:alice'], lastSeq: 1, lastChangeSeq: 1, lastReadSeq: 0 };
const message: TeamMessage = { id: 'message01', seq: 1, author: { kind: 'user', uid: 'owner' }, kind: 'text', text: 'Synthetic', createdAt: null, changeSeq: 1, changeId: 'change001' };
const page: MessagePage = { conversation, changes: [{ id: 'change001', seq: 1, messageId: 'message01', operation: 'create' }], messages: [message], hasMore: false };
const clone = <T>(value: T): T => structuredClone(value);
function memoryStorage() {
  const records = new Map<string, MessageProjection>();
  let failure = false;
  return { records, fail() { failure = true; }, async update(key, reduce) { const value = reduce(clone(records.get(key))); if (failure) { failure = false; throw new Error('STORAGE_FAILED'); } records.set(key, clone(value)); return clone(value); } } satisfies MessageStorage & { records: Map<string, MessageProjection>; fail(): void };
}
const until = async (condition: () => boolean) => { const end = Date.now() + 3000; while (!condition()) { if (Date.now() > end) throw new Error('Condition timed out'); await new Promise(resolve => setTimeout(resolve, 5)); } };

test('T2 projection applies contiguous changes once and old pages cannot restore recalled plaintext or regress cursors', () => {
  let state = applyMessagePage(emptyProjection('owner', scope), conversation.id, page);
  assert.deepEqual(applyMessagePage(state, conversation.id, page), state);
  state = applyMessagePage(state, conversation.id, { ...page, conversation: { ...conversation, lastChangeSeq: 2, lastReadSeq: 1 }, changes: [{ id: 'change002', seq: 2, messageId: message.id, operation: 'recall' }], messages: [{ ...message, text: '', recalled: true, changeSeq: 2, changeId: 'change002' }] });
  const replay = applyMessagePage(state, conversation.id, page);
  assert.equal(replay.threads[conversation.id]!.cursor, 2); assert.equal(replay.threads[conversation.id]!.messages[0]!.text, ''); assert.equal(replay.conversations[0]!.lastChangeSeq, 2); assert.equal(replay.conversations[0]!.lastReadSeq, 1);
  assert.throws(() => applyMessagePage(state, conversation.id, { ...page, changes: [{ id: 'gap', seq: 4, messageId: message.id, operation: 'edit' }] }), /CHANGE_GAP/);
});
test('T2 stale conversation list retains confirmed read and message watermarks', () => {
  const newer = { ...conversation, lastSeq: 10, lastChangeSeq: 12, lastReadSeq: 10 };
  assert.deepEqual(mergeConversation(newer, conversation), newer);
});
test('T2 failed local commit leaves both body and cursor unchanged, then replay succeeds', async () => {
  const storage = memoryStorage(), key = 'synthetic';
  await storage.update(key, () => emptyProjection('owner', scope)); storage.fail();
  await assert.rejects(storage.update(key, old => applyMessagePage(old!, conversation.id, page)), /STORAGE_FAILED/);
  assert.deepEqual(storage.records.get(key)!.threads, {});
  await storage.update(key, old => applyMessagePage(old!, conversation.id, page));
  assert.equal(storage.records.get(key)!.threads[conversation.id]!.cursor, 1);
});
test('T2 durable outbox survives an ambiguous response and retries the same identity after restart', async () => {
  const storage = memoryStorage(); let online = true, drop = true, calls: MessageCommand[] = [];
  const api: MessageApi = { onMessagesChanged: () => () => {}, async messages<T>(command: MessageCommand): Promise<T> {
    if (!online) throw new Error('SERVICE_UNAVAILABLE');
    if (command.action === 'list') return { conversations: [conversation], next: null } as T;
    if (command.action === 'sync') return { ...page, changes: command.after ? [] : page.changes } as T;
    if (command.action === 'mutate') { calls.push(command); if (drop) { drop = false; online = false; throw new Error('SERVICE_UNAVAILABLE'); } return { changeSeq: 1 } as T; }
    return {} as T;
  } };
  const first = new TeamMessaging(api, storage, 'owner', scope); await first.start(); await until(() => !first.offline); first.select(conversation.id);
  await first.queue('create', 'Retained intent'); await until(() => calls.length === 1 && first.offline); first.close();
  const intent = calls[0]; online = true;
  const second = new TeamMessaging(api, storage, 'owner', scope); await second.start();
  await until(() => calls.length === 2 && second.state.outbox.length === 0);
  assert.deepEqual(calls[1], intent); second.close();
});
test('T2 pending intent can be discarded; an attempted intent stays protected after lease expiry', async context => {
  const storage = memoryStorage(); let send: (() => void) | undefined; const sent: string[] = [];
  const initial = emptyProjection('owner', scope);
  initial.outbox = [1, 2].map(i => ({ action: 'mutate', ...scope, conversationId: conversation.id, messageId: `message0${i}`, changeId: `change00${i}`, operation: 'create', text: `Pending ${i}` }));
  await storage.update(JSON.stringify(['owner', scope.teamId, 1]), () => initial);
  const api: MessageApi = { onMessagesChanged: () => () => {}, async messages<T>(value: MessageCommand): Promise<T> {
    if (value.action === 'list') return { conversations: [conversation], next: null } as T;
    if (value.action === 'mutate') { sent.push(value.changeId); if (sent.length === 1) await new Promise<void>(resolve => { send = resolve; }); }
    return {} as T;
  } };
  const model = new TeamMessaging(api, storage, 'owner', scope); await model.start(); await until(() => Boolean(send));
  await assert.rejects(model.discard('change001'), /MESSAGE_IN_FLIGHT/); const later = Date.now() + 31000; context.mock.method(Date, 'now', () => later); await assert.rejects(model.discard('change001'), /MESSAGE_IN_FLIGHT/); context.mock.restoreAll(); await model.discard('change002'); send!();
  await until(() => model.state.outbox.length === 0); assert.deepEqual(sent, ['change001']); model.close();
});
test('T2 concurrent tabs share durable claims and never regress read watermarks', async () => {
  const storage = memoryStorage(); let release: (() => void) | undefined; let stale = false;
  const api: MessageApi = { onMessagesChanged: () => () => {}, async messages<T>(value: MessageCommand): Promise<T> {
    if (value.action === 'list') { if (stale) await new Promise<void>(resolve => { release = resolve; }); return { conversations: [conversation], next: null } as T; }
    if (value.action === 'sync') return { ...page, changes: value.after ? [] : page.changes } as T;
    if (value.action === 'read') return { lastReadSeq: 1 } as T;
    return {} as T;
  } };
  const model = new TeamMessaging(api, storage, 'owner', scope); await model.start(); await until(() => !model.offline); model.select(conversation.id); await until(() => Boolean(model.state.threads[conversation.id]));
  stale = true; const pending = model.refresh(); await until(() => Boolean(release)); await model.markRead(); release!(); await pending;
  assert.equal(model.state.conversations[0]!.lastReadSeq, 1); model.close();
});

test('T2 a permanently rejected conversation creation cannot block existing message synchronization', async () => {
  const storage = memoryStorage(); let creates = 0;
  const api: MessageApi = { onMessagesChanged: () => () => {}, async messages<T>(value: MessageCommand): Promise<T> {
    if (value.action === 'create') { creates++; throw new Error('PARTICIPANT_REQUIRED'); }
    if (value.action === 'list') return { conversations: [conversation], next: null } as T;
    if (value.action === 'sync') return { ...page, changes: value.after ? [] : page.changes } as T;
    return {} as T;
  } };
  const model = new TeamMessaging(api, storage, 'owner', scope); await model.start(); await until(() => !model.offline);
  await model.create('dm', 'Unavailable recipient', ['owner', 'removed']);
  await until(() => model.state.creation?.error === 'PARTICIPANT_REQUIRED');
  model.select(conversation.id); await until(() => Boolean(model.state.threads[conversation.id]));
  assert.equal(creates, 1); assert.equal(model.offline, false); await model.discardCreation(); assert.equal(model.state.creation, undefined); model.close();
});
test('T2 revoked access clears the view and stops sending even when durable storage fails', async () => {
  const backing = memoryStorage(); let denied = false, clearing = false;
  const storage: MessageStorage = { async update(key, reduce) { if (clearing) throw new Error('STORAGE_FAILED'); return backing.update(key, reduce); } };
  const api: MessageApi = { onMessagesChanged: () => () => {}, async messages<T>(value: MessageCommand): Promise<T> {
    if (denied) { clearing = true; throw new Error('MEMBERSHIP_REQUIRED'); }
    if (value.action === 'list') return { conversations: [conversation], next: null } as T;
    if (value.action === 'sync') return { ...page, changes: value.after ? [] : page.changes } as T;
    return {} as T;
  } };
  const model = new TeamMessaging(api, storage, 'owner', scope); await model.start(); await until(() => !model.offline); model.select(conversation.id); await until(() => Boolean(model.state.threads[conversation.id]));
  denied = true; await model.refresh(); await until(() => model.error === 'MEMBERSHIP_REQUIRED');
  assert.deepEqual(model.state.threads, {}); assert.deepEqual(model.state.outbox, []); assert.equal(model.selected, '');
  await assert.rejects(model.queue('create', 'Must not send'), /INVALID_INPUT/); await model.refresh();
});

test('T2 mark-read authorization failure clears a visible conversation immediately', async () => {
  const api: MessageApi = { onMessagesChanged: () => () => {}, async messages<T>(value: MessageCommand): Promise<T> {
    if (value.action === 'list') return { conversations: [conversation], next: null } as T;
    if (value.action === 'sync') return { ...page, changes: value.after ? [] : page.changes } as T;
    if (value.action === 'read') throw new Error('MEMBERSHIP_REQUIRED');
    return {} as T;
  } };
  const model = new TeamMessaging(api, memoryStorage(), 'owner', scope); await model.start(); await until(() => !model.offline); model.select(conversation.id); await until(() => Boolean(model.state.threads[conversation.id]));
  await assert.rejects(model.markRead(), /MEMBERSHIP_REQUIRED/); assert.equal(model.selected, ''); assert.deepEqual(model.state.threads, {});
});
