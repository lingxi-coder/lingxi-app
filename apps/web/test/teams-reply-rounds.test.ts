// Bot v3 B7a: what the clients take of the reply rounds of several Bots: the codes they share with the service, the two settings of a Chat, and the commands a member may send.
import assert from 'node:assert/strict';
import test from 'node:test';
import vectors from '../../../packages/teams/contracts/execution-vectors.json';
import { BID_TIERS, executionCommand, HUMAN_MENTION_MODES, MAX_REPLIES_LIMIT, ROUND_OUTCOMES } from '../../../packages/teams/execution';
import { isMemberExecutionCommand } from '../../../packages/teams/execution-validation';
import { watchFirestoreMessages } from '../../../packages/teams/firestore';
import { emptyProjection, mergeConversation, mergeDirectory, type MessageSnapshot, type TeamConversation } from '../../../packages/teams/messages';

const scope = { teamId: 'test-team', generation: 1 }, uid = 'alice';
const payload = { conversationId: 'chat00001', expectedRevision: 2, targets: ['sharedbot01'] };

test('the codes of a reply round are the ones the contract lists, so that iOS, Android and the service read the same list', () => {
  assert.deepEqual([...BID_TIERS], vectors.bidTiers);
  assert.deepEqual([...ROUND_OUTCOMES], vectors.roundOutcomes);
  assert.deepEqual([...HUMAN_MENTION_MODES], vectors.humanMentionModes);
  assert.equal(MAX_REPLIES_LIMIT, vectors.maxRepliesLimit);
  assert.deepEqual([...BID_TIERS], ['addressed', 'rule', 'run', 'may_decline', 'decline'], 'best first');
});

test('every command of the shared vectors is read the same here: the two settings a Chat may add are checked, and a bid is not a member\'s command', () => {
  const rows = vectors.commands.filter(vector => vector.input.action === 'setConversationAutoReply');
  assert.ok(rows.length >= 17 && rows.some(vector => vector.valid) && rows.some(vector => !vector.valid));
  for (const vector of vectors.commands) assert.equal(isMemberExecutionCommand(vector.input), vector.valid, vector.name);
  assert.equal(vectors.commands.find(vector => vector.name === 'reject-bidReply-member')?.valid, false);
  assert.equal(isMemberExecutionCommand({ schemaVersion: 1, commandId: 'command001', ...scope, action: 'bidReply', payload: { taskId: 'task00001', tier: 'run', source: 'decision' } }), false);
});

test('a member sets how many Bots answer and what a message that mentions only people does, with the Bots of the Chat, or leaves them as they are', () => {
  assert.deepEqual(executionCommand(scope, 'setConversationAutoReply', payload, 'command001'), { schemaVersion: 1, commandId: 'command001', ...scope, action: 'setConversationAutoReply', payload });
  for (const more of [{ maxReplies: 1 }, { maxReplies: MAX_REPLIES_LIMIT }, { humanMentions: 'quiet' as const }, { humanMentions: 'judge' as const }, { maxReplies: 3, humanMentions: 'judge' as const }]) {
    const command = executionCommand(scope, 'setConversationAutoReply', { ...payload, ...more }, 'command001');
    assert.deepEqual(command.payload, { ...payload, ...more }); assert.equal(isMemberExecutionCommand(command), true);
  }
  for (const more of [{ maxReplies: 0 }, { maxReplies: MAX_REPLIES_LIMIT + 1 }, { maxReplies: 1.5 }, { maxReplies: '2' }, { humanMentions: 'ignore' }, { humanMentions: null }, { extra: true }]) {
    assert.throws(() => executionCommand(scope, 'setConversationAutoReply', { ...payload, ...more } as never, 'command001'), /INVALID_INPUT/, JSON.stringify(more));
  }
});

const base: TeamConversation = { id: 'test-chat', kind: 'group', title: 'Fixture', participants: ['user:alice', 'shared_bot:bot_one'], lastSeq: 2, lastChangeSeq: 2, lastReadSeq: 0 };
test('conversation merge: a changed setting replaces the row, an unchanged one keeps it', () => {
  const set = { ...base, autoReply: { revision: 1, targets: ['bot_one'], maxReplies: 2, humanMentions: 'judge' as const } };
  assert.equal(mergeConversation(set, { ...set, autoReply: { ...set.autoReply } }), set);
  for (const next of [{ ...set.autoReply, maxReplies: 3 }, { ...set.autoReply, maxReplies: undefined }, { ...set.autoReply, humanMentions: 'quiet' as const }, { ...set.autoReply, humanMentions: undefined }]) {
    const merged = mergeConversation(set, { ...base, autoReply: next });
    assert.notEqual(merged, set, JSON.stringify(next)); assert.deepEqual(merged.autoReply, next);
  }
  const state = mergeDirectory(emptyProjection(uid, scope), [set], true);
  assert.equal(mergeDirectory(state, [{ ...set, autoReply: { ...set.autoReply } }], true), state);
});

type Listener = { path: string; next(snapshot: unknown): void };
function watch() {
  const listeners: Listener[] = [];
  const target = (kind: string, path: unknown[]) => ({ kind, path: path.filter(part => typeof part === 'string').join('/') });
  const sdk = {
    collection: (_db: unknown, ...path: string[]) => target('collection', path), doc: (_db: unknown, ...path: string[]) => target('doc', path),
    query: (source: { path: string }) => ({ ...source, kind: 'query' }), where: () => ({}), limit: () => ({}), orderBy: () => ({}),
    onSnapshot: (source: { kind: string; path: string }, _options: unknown, next: (snapshot: unknown) => void) => { listeners.push({ path: `${source.kind}:${source.path}`, next }); return () => {}; },
  } as unknown as typeof import('firebase/firestore');
  const received: MessageSnapshot[] = [];
  const stop = watchFirestoreMessages(sdk, {} as never, uid, scope, '', value => received.push(value), () => true);
  const on = (path: string) => listeners.find(value => value.path === path)!;
  on('doc:teams/test-team/members/alice').next({ data: () => ({ generation: 1 }), metadata: { fromCache: false } });
  const document = (id: string, value: Record<string, unknown>) => ({ id, data: () => ({ kind: 'group', title: id, participants: ['user:alice'], lastSeq: 2, lastChangeSeq: 2, lastMessage: null, ...value }) });
  const directory = (docs: unknown[]) => on('query:teams/test-team/conversations').next({ metadata: { fromCache: false }, docChanges: () => [], docs });
  return { received, directory, document, stop };
}

test('Firestore listener: a Chat row carries the two settings its document holds, and leaves out one this version cannot read without hiding the Chat', () => {
  const { received, directory, document, stop } = watch();
  directory([
    document('both', { autoReply: { revision: 3, targets: ['bot_one'], maxReplies: 2, humanMentions: 'judge' } }),
    document('maximum', { autoReply: { revision: 1, targets: [], maxReplies: MAX_REPLIES_LIMIT, humanMentions: 'quiet' } }),
    document('plain', { autoReply: { revision: 1, targets: ['bot_one'] } }),
    document('none', {}),
    document('unknown', { autoReply: { revision: 2, targets: ['bot_one'], maxReplies: 99, humanMentions: 'a_new_mode' } }),
    document('junk', { autoReply: { revision: 2, targets: ['bot_one'], maxReplies: '2', humanMentions: 7 } }),
    document('fraction', { autoReply: { revision: 2, targets: ['bot_one'], maxReplies: 1.5, humanMentions: null } }),
  ]);
  stop();
  const rows = new Map(received.find((value): value is Extract<MessageSnapshot, { kind: 'directory' }> => value.kind === 'directory')!.conversations.map(value => [value.id, value]));
  assert.deepEqual(rows.get('both')!.autoReply, { revision: 3, targets: ['bot_one'], maxReplies: 2, humanMentions: 'judge' });
  assert.deepEqual(rows.get('maximum')!.autoReply, { revision: 1, targets: [], maxReplies: MAX_REPLIES_LIMIT, humanMentions: 'quiet' });
  assert.deepEqual(rows.get('plain')!.autoReply, { revision: 1, targets: ['bot_one'] }, 'never set: the Chat reads as the default');
  assert.ok(!('autoReply' in rows.get('none')!));
  for (const id of ['unknown', 'junk', 'fraction']) assert.deepEqual(rows.get(id)!.autoReply, { revision: 2, targets: ['bot_one'] }, id);
  assert.equal(rows.size, 7, 'no Chat is hidden');
  assert.ok(!received.some(value => value.kind === 'error'));
});
