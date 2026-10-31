import assert from 'node:assert/strict';
import test from 'node:test';
import { TeamMessaging, applyMessageSnapshot, emptyProjection, pendingCommands, type MessageApi, type MessageSnapshot, type PendingCommands, type CommandStorage, type TeamConversation, type TeamMessage } from '../../../packages/teams/messages.js';
const scope = { teamId: 'test-team', generation: 1 }, uid = 'owner';
const conversation: TeamConversation = { id: 'test-chat', kind: 'dm', title: 'Fixture', participants: ['user:owner'], lastSeq: 1, lastChangeSeq: 1, lastReadSeq: 0 };
const message: TeamMessage = { id: 'message01', seq: 1, author: { kind: 'user', uid }, kind: 'text', text: 'Synthetic', createdAt: null, changeSeq: 1, changeId: 'change001' };
function memoryStorage(): CommandStorage & { records: Map<string, PendingCommands> } {
  const records = new Map<string, PendingCommands>();
  return { records, async load(key) { return structuredClone(records.get(key)); }, async update(key, reduce) { const value = pendingCommands(reduce(structuredClone(records.get(key)))); records.set(key, structuredClone(value)); return value; } };
}
const until = async (condition: () => boolean) => { const end = Date.now() + 3000; while (!condition()) { if (Date.now() > end) throw new Error('Condition timed out'); await new Promise(resolve => setTimeout(resolve, 5)); } };
function fixture(write: MessageApi['messages'] = async () => { throw new Error('Unexpected HTTP read'); }) {
  const listeners = new Set<(snapshot: MessageSnapshot) => void>();
  const api: MessageApi = { messages: write, onMessagesChanged: () => () => {}, watchMessages(current, id, next) {
    listeners.add(next);
    queueMicrotask(() => { if (!listeners.has(next)) return; next({ uid, ...current, fromCache: false, kind: 'directory', conversations: [conversation] }); if (id) next({ uid, ...current, fromCache: false, kind: 'messages', conversationId: id, messages: [message] }); });
    return () => { listeners.delete(next); };
  } };
  return { api, listeners, emit(value: MessageSnapshot) { for (const next of listeners) next(value); } };
}

test('SDK deltas preserve unchanged rows and apply old edits, recalls, and removals', () => {
  const second = { ...message, id: 'message02', seq: 2, text: 'Second' };
  const initial: MessageSnapshot = { uid, ...scope, kind: 'messages', conversationId: conversation.id, fromCache: true, replace: true, messages: [message, second] };
  const state = applyMessageSnapshot(emptyProjection(uid, scope), initial);
  assert.equal(applyMessageSnapshot(state, { ...initial, replace: false, fromCache: false, messages: [] }), state);
  const edited = applyMessageSnapshot(state, { ...initial, replace: false, messages: [{ ...message, text: '', recalled: true, changeSeq: 7 }] });
  assert.equal(edited.threads[conversation.id]!.messages[1], second);
  assert.equal(edited.threads[conversation.id]!.messages[0]!.text, '');
  assert.equal(edited.threads[conversation.id]!.messages[0]!.recalled, true);
  const removed = applyMessageSnapshot(edited, { ...initial, replace: false, messages: [], removed: [second.id] });
  assert.deepEqual(removed.threads[conversation.id]!.messages.map(value => value.id), [message.id]);
  assert.equal(applyMessageSnapshot(removed, { ...initial, messages: [] }).threads[conversation.id]!.messages.length, 0);
});

test('SDK message snapshots update views without any HTTP reads or durable history writes', async context => {
  const storage = memoryStorage(), source = fixture();
  let timers = 0;
  context.mock.method(globalThis, 'setInterval', () => { timers++; throw new Error('Unexpected fixed polling'); });
  const model = new TeamMessaging(source.api, storage, uid, scope); context.after(() => model.close()); await model.start(); model.select(conversation.id);
  await until(() => !!model.state.threads[conversation.id]); await model.refresh();
  assert.equal(model.state.threads[conversation.id]!.messages[0]!.text, 'Synthetic');
  assert.equal(storage.records.size, 0); assert.equal(timers, 0);
  source.emit({ uid, ...scope, kind: 'messages', conversationId: conversation.id, fromCache: true, messages: [{ ...message, text: '', recalled: true, changeSeq: 10 }] });
  assert.equal(model.offline, true); assert.equal(model.state.threads[conversation.id]!.messages[0]!.text, '');
  source.emit({ uid, ...scope, kind: 'messages', conversationId: conversation.id, fromCache: false, messages: [{ ...message, text: '', recalled: true, changeSeq: 10 }] });
  assert.equal(model.offline, false);
});

test('cross-device read snapshots are monotonic; wrong UID/generation and late selection are ignored', async context => {
  const source = fixture(), model = new TeamMessaging(source.api, memoryStorage(), uid, scope); context.after(() => model.close());
  await model.start(); model.select(conversation.id); await until(() => !!model.state.threads[conversation.id]);
  source.emit({ uid, ...scope, kind: 'read', conversationId: conversation.id, fromCache: false, lastReadSeq: 1 });
  source.emit({ uid, ...scope, kind: 'read', conversationId: conversation.id, fromCache: true, lastReadSeq: 0 });
  assert.equal(model.state.conversations[0]!.lastReadSeq, 1);
  for (const wrong of [{ uid: 'bob', generation: 1 }, { uid, generation: 2 }]) source.emit({ ...scope, ...wrong, kind: 'messages', conversationId: conversation.id, fromCache: false, messages: [] });
  assert.equal(model.state.threads[conversation.id]!.messages.length, 1);
  model.select('other-chat'); source.emit({ uid, ...scope, kind: 'messages', conversationId: conversation.id, fromCache: false, messages: [] });
  assert.equal(model.state.threads[conversation.id], undefined);
});

test('HTTP outbox survives ACK loss and restart; SDK history never enters the command journal', async context => {
  const storage = memoryStorage(); let drop = true; const calls: unknown[] = [];
  const source = fixture(async command => { assert.equal(command.action, 'mutate'); if (drop && calls.length) throw new Error('SERVICE_UNAVAILABLE'); calls.push(structuredClone(command)); if (drop) throw new Error('SERVICE_UNAVAILABLE'); return {} as never; });
  const first = new TeamMessaging(source.api, storage, uid, scope); context.after(() => first.close()); await first.start(); first.select(conversation.id);
  await until(() => !first.offline); await first.queue('create', 'Retained pending text'); await until(() => calls.length === 1 && first.offline); first.close(); drop = false;
  assert.ok([...storage.records.values()].every(value => !('threads' in value) && !('conversations' in value)));
  const second = new TeamMessaging(source.api, storage, uid, scope); context.after(() => second.close()); await second.start();
  await until(() => calls.length === 2 && !second.state.outbox.length); assert.deepEqual(calls[1], calls[0]);
});

test('revocation clears messages immediately even if command-journal cleanup fails', async context => {
  const source = fixture(), storage: CommandStorage = { async load() { return undefined; }, async update() { throw new Error('STORAGE_FAILED'); } };
  const model = new TeamMessaging(source.api, storage, uid, scope); context.after(() => model.close()); await model.start(); model.select(conversation.id); await until(() => !!model.state.threads[conversation.id]);
  source.emit({ uid, ...scope, kind: 'error', fromCache: false, error: 'MEMBERSHIP_REQUIRED' });
  assert.equal(model.selected, ''); assert.deepEqual(model.state.threads, {}); assert.equal(source.listeners.size, 0);
  await assert.rejects(model.queue('create', 'Must not send'), /INVALID_INPUT/);
});

test('pending command claims coordinate windows and an attempted identity cannot be discarded', async context => {
  const storage = memoryStorage(); let release: (() => void) | undefined, sends = 0;
  const pending = emptyProjection(uid, scope);
  pending.outbox = [{ action: 'mutate', ...scope, conversationId: conversation.id, messageId: message.id, changeId: 'pending001', operation: 'create', text: 'Queued' }];
  await storage.update(JSON.stringify([uid, scope.teamId, 1]), () => pending);
  const source = fixture(async () => { sends++; await new Promise<void>(resolve => { release = resolve; }); return {} as never; });
  const first = new TeamMessaging(source.api, storage, uid, scope), second = new TeamMessaging(source.api, storage, uid, scope);
  context.after(() => { first.close(); second.close(); release?.(); }); await Promise.all([first.start(), second.start()]); await until(() => !!release);
  await assert.rejects(second.discard('pending001'), /MESSAGE_IN_FLIGHT/); assert.equal(sends, 1); release!(); await until(() => !first.state.outbox.length || !second.state.outbox.length); assert.equal(sends, 1);
});

test('command notifications coordinate durable backoff across tabs without a retry storm', async context => {
  const memory = memoryStorage(), listeners = new Set<() => void>();
  const storage: CommandStorage = { load: memory.load, onChanged(_key, callback) { listeners.add(callback); return () => { listeners.delete(callback); }; }, async update(key, reduce) {
    const before = JSON.stringify(memory.records.get(key)), value = await memory.update(key, reduce);
    if (JSON.stringify(value) !== before) queueMicrotask(() => listeners.forEach(callback => callback()));
    return value;
  } };
  let online = false; const calls: unknown[] = [];
  const source = fixture(async command => { calls.push(structuredClone(command)); if (!online) throw new Error('SERVICE_UNAVAILABLE'); return {} as never; });
  const first = new TeamMessaging(source.api, storage, uid, scope), second = new TeamMessaging(source.api, storage, uid, scope);
  context.after(() => { first.close(); second.close(); });
  await Promise.all([first.start(), second.start()]); first.select(conversation.id); await first.queue('create', 'Shared durable intent');
  await until(() => calls.length > 0); await new Promise(resolve => setTimeout(resolve, 100)); assert.equal(calls.length, 1);
  first.close(); online = true; const future = Date.now() + 2000; context.mock.method(Date, 'now', () => future);
  await second.refresh(); await until(() => !second.state.outbox.length);
  assert.equal(calls.length, 2); assert.deepEqual(calls[1], calls[0]);
});
