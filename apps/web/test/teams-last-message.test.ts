import assert from 'node:assert/strict';
import test from 'node:test';
import { Timestamp } from 'firebase/firestore';
import { firestoreMessage, firestoreLastMessage, watchFirestoreMessages } from '../../../packages/teams/firestore.js';
import { emptyProjection, mergeConversation, mergeDirectory, type MessageSnapshot, type TeamConversation } from '../../../packages/teams/messages.js';

type Last = NonNullable<TeamConversation['lastMessage']>;
const scope = { teamId: 'test-team', generation: 1 }, uid = 'alice';
const base: TeamConversation = { id: 'test-chat', kind: 'dm', title: 'Fixture', participants: ['user:alice', 'user:bob'], lastSeq: 2, lastChangeSeq: 2, lastReadSeq: 0 };
const at = '2026-11-02T10:00:00.000Z';
const last = (seq: number, more: Partial<Last> = {}): Last => ({ id: `message-${seq}`, seq, at, preview: `Body ${seq}`, author: { kind: 'user', uid: 'bob' }, ...more });
const withLast = (value: Last | undefined, more: Partial<TeamConversation> = {}): TeamConversation => ({ ...base, ...more, ...(value ? { lastMessage: value } : {}) });

test('conversation merge: rows without a lastMessage stay exactly as they were', () => {
  const merged = mergeConversation(undefined, base);
  assert.deepEqual(merged, base); assert.ok(!('lastMessage' in merged));
  assert.equal(mergeConversation(base, { ...base }), base);
});

test('conversation merge: the higher seq wins and an edit or recall at the same seq replaces the preview', () => {
  const old = withLast(last(2));
  assert.deepEqual(mergeConversation(undefined, old).lastMessage, last(2));
  assert.equal(mergeConversation(old, withLast(last(3), { lastSeq: 3 })).lastMessage!.id, 'message-3'); // a newer message
  assert.equal(mergeConversation(old, withLast(last(1))).lastMessage!.id, 'message-2'); // a stale cache snapshot never steps back
  const edited = mergeConversation(old, withLast(last(2, { preview: 'Edited body' })));
  assert.equal(edited.lastMessage!.preview, 'Edited body'); assert.notEqual(edited, old);
  const recalled = mergeConversation(edited, withLast(last(2, { preview: '', recalled: true })));
  assert.equal(recalled.lastMessage!.preview, ''); assert.equal(recalled.lastMessage!.recalled, true);
  assert.equal(mergeConversation(old, withLast(undefined)).lastMessage, old.lastMessage); // a dropped or missing pointer keeps the last good one
  assert.equal(mergeConversation(old, withLast(undefined)), old);
});

test('conversation merge: an identical pointer keeps the row identity, any visible difference does not', () => {
  const old = withLast(last(2));
  assert.equal(mergeConversation(old, withLast(last(2))), old); // a fresh but equal object from the next snapshot
  for (const change of [{ preview: 'Other' }, { recalled: true as const }, { at: '2026-11-02T10:00:01.000Z' }, { id: 'message-9' }, { author: { kind: 'shared_bot' as const, displayName: 'Helper', sharedBotId: 'assistant-fixture' } },
    { author: { kind: 'user' as const, uid: 'carol' } }]) {
    const next = mergeConversation(old, withLast(last(2, change)));
    assert.notEqual(next, old, JSON.stringify(change));
  }
});

test('directory merge: unchanged snapshots reuse the projection and a preview change replaces only that row', () => {
  const other: TeamConversation = { ...base, id: 'other-chat', lastMessage: last(1) };
  let state = mergeDirectory(emptyProjection(uid, scope), [withLast(last(2)), other], true);
  assert.equal(mergeDirectory(state, [withLast(last(2)), { ...other, lastMessage: last(1) }], true), state);
  const next = mergeDirectory(state, [withLast(last(2, { preview: 'Edited body' })), { ...other, lastMessage: last(1) }], true);
  assert.notEqual(next, state);
  assert.equal(next.conversations.find(value => value.id === 'test-chat')!.lastMessage!.preview, 'Edited body');
  assert.equal(next.conversations.find(value => value.id === 'other-chat'), state.conversations.find(value => value.id === 'other-chat'));
  state = next;
  assert.equal(mergeDirectory(state, [withLast(last(2, { preview: 'Edited body' })), { ...other, lastMessage: last(1) }], true), state);
});

test('Firestore pointer: timestamps become ISO strings and every field of the full shape is kept', () => {
  const value = { id: 'message-5', seq: 5, at: Timestamp.fromDate(new Date(at)), preview: 'Hello there', author: { kind: 'shared_bot', displayName: 'Helper', sharedBotId: 'assistant-fixture' } };
  assert.deepEqual(firestoreLastMessage(value), { id: 'message-5', seq: 5, at, preview: 'Hello there', author: { kind: 'shared_bot', displayName: 'Helper', sharedBotId: 'assistant-fixture' } });
  assert.deepEqual(firestoreLastMessage({ ...value, author: { kind: 'user', uid: 'bob' }, preview: '', recalled: true }),
    { id: 'message-5', seq: 5, at, preview: '', recalled: true, author: { kind: 'user', uid: 'bob' } });
  assert.equal(firestoreLastMessage({ ...value, at: null })!.at, null, 'a write made on this device that the server has not stamped yet');
});

test('Firestore pointer: null is a conversation with no message yet, and anything but the whole pointer is a data error', () => {
  const stamp = Timestamp.fromDate(new Date(at)), whole = { id: 'message-2', seq: 2, at: stamp, preview: 'Hi', author: { kind: 'user', uid: 'bob' } };
  assert.equal(firestoreLastMessage(null), undefined);
  assert.ok(firestoreLastMessage(whole));
  const { author: _author, ...withoutAuthor } = whole, { preview: _preview, ...withoutPreview } = whole, { at: _at, ...withoutTime } = whole;
  for (const value of [
    undefined, 0, 'message-2', [], {}, { id: 'message-2' }, { seq: 2 },
    { id: 'message-2', seq: 2, at: stamp }, // written before previews existed
    withoutAuthor, withoutPreview, withoutTime,
    { ...whole, id: '' }, { ...whole, id: 7 }, { ...whole, seq: '2' }, { ...whole, seq: 2.5 }, { ...whole, seq: Number.NaN }, { ...whole, at: '2026-11-02' },
    { ...whole, preview: 7 }, { ...whole, preview: null }, { ...whole, recalled: false }, { ...whole, recalled: 'yes' },
    ...[null, 'bob', [], {}, { kind: 'user' }, { kind: 'user', uid: '' }, { kind: 'user', uid: 5 }, { uid: 'bob' }, { kind: 'robot', uid: 'bob' }, { kind: 'shared_bot', uid: 'owner' }, { kind: 'shared_bot', displayName: 'Helper', sharedBotId: '' }]
      .map(author => ({ ...whole, author })),
  ]) assert.throws(() => firestoreLastMessage(value), { message: 'INVALID_DATA' }, JSON.stringify(value));
});

type Listener = { path: string; next(snapshot: unknown): void };
function watch() {
  const listeners: Listener[] = [];
  let stopped = 0;
  const target = (kind: string, path: unknown[]) => ({ kind, path: path.filter(part => typeof part === 'string').join('/') });
  const sdk = {
    collection: (_db: unknown, ...path: string[]) => target('collection', path),
    doc: (_db: unknown, ...path: string[]) => target('doc', path),
    query: (source: { path: string }) => ({ ...source, kind: 'query' }),
    where: () => ({}), orderBy: () => ({}),
    onSnapshot: (source: { kind: string; path: string }, _options: unknown, next: (snapshot: unknown) => void) => { listeners.push({ path: `${source.kind}:${source.path}`, next }); return () => { stopped++; }; },
  } as unknown as typeof import('firebase/firestore');
  const received: MessageSnapshot[] = [];
  const stop = watchFirestoreMessages(sdk, {} as never, uid, scope, '', value => received.push(value), () => true);
  const on = (path: string) => listeners.find(value => value.path === path)!;
  on('doc:teams/test-team/members/alice').next({ data: () => ({ generation: 1 }), metadata: { fromCache: false } });
  const document = (id: string, value: Record<string, unknown>) => ({ id, data: () => ({ kind: 'dm', title: id, participants: ['user:alice'], lastSeq: 2, lastChangeSeq: 2, lastMessage: null, ...value }) });
  const directory = (docs: unknown[]) => on('query:teams/test-team/conversations').next({ metadata: { fromCache: false }, docChanges: () => [], docs });
  return { received, directory, document, stop, stopped: () => stopped };
}

test('directory snapshots carry each conversation lastMessage through the Firestore watcher', () => {
  const { received, directory, document, stop } = watch();
  directory([
    document('with-preview', { lastMessage: { id: 'message-2', seq: 2, at: Timestamp.fromDate(new Date(at)), preview: 'Hello', author: { kind: 'user', uid: 'bob' } } }),
    document('recalled', { lastMessage: { id: 'message-3', seq: 3, at: Timestamp.fromDate(new Date(at)), preview: '', recalled: true, author: { kind: 'shared_bot', displayName: 'Helper', sharedBotId: 'assistant-fixture' } } }),
    document('empty', {}),
  ]);
  stop();
  const rows = new Map(received.find((value): value is Extract<MessageSnapshot, { kind: 'directory' }> => value.kind === 'directory')!.conversations.map(value => [value.id, value]));
  assert.deepEqual(rows.get('with-preview')!.lastMessage, { id: 'message-2', seq: 2, at, preview: 'Hello', author: { kind: 'user', uid: 'bob' } });
  assert.deepEqual(rows.get('recalled')!.lastMessage, { id: 'message-3', seq: 3, at, preview: '', recalled: true, author: { kind: 'shared_bot', displayName: 'Helper', sharedBotId: 'assistant-fixture' } });
  assert.ok(!('lastMessage' in rows.get('empty')!));
  assert.ok(!received.some(value => value.kind === 'error'));
});

test('a conversation whose pointer breaks the contract stops the listeners with INVALID_DATA instead of being shown half-parsed', () => {
  const stamp = Timestamp.fromDate(new Date(at));
  for (const lastMessage of [{ id: 'message-2', seq: 2, at: stamp }, 'oops', {}]) {
    const { received, directory, document, stopped } = watch();
    directory([document('fine', {}), document('broken', { lastMessage })]);
    assert.ok(!received.some(value => value.kind === 'directory'), 'no directory with a guessed row');
    const failure = received.find((value): value is Extract<MessageSnapshot, { kind: 'error' }> => value.kind === 'error');
    assert.equal(failure?.error, 'INVALID_DATA', JSON.stringify(lastMessage));
    assert.equal(stopped(), 3, 'the member, directory and signal listeners are released');
    directory([document('fine', {})]);
    assert.equal(received.filter(value => value.kind === 'error').length, 1, 'and nothing arrives afterwards');
  }
});

test('SDK message reader rejects retired Bot author format', () => { assert.throws(() => firestoreMessage('old-message', { seq: 1, author: { kind: 'bot', uid: 'owner', botKey: 'retired' } }), /INVALID_DATA/); });
