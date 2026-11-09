import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mirrorFriendChat, mirrorFriendRead } from '../src/friend-summary.mjs';
import { onFriendChatRead, onFriendChatUpdated, onTeamMessage } from '../src/index.mjs';

// A database that fails the test when it is touched at all.
const untouched = new Proxy({}, { get: (_, name) => () => { throw new Error(`db.${String(name)} was called`); } });
const pair = `f_${'a'.repeat(40)}`;
const chat = { memberUids: ['alice', 'bob'], lastSeq: 2, lastChangeSeq: 2, lastMessage: { id: 'message02', seq: 2 } };

test('summary triggers: only a friend chat costs a read — other ids and unusable events return before any database call', async () => {
  for (const [teamId, conversationId] of [['plain-team', 'friend_chat'], [pair, 'bot_chat_01'], [`team_${pair}`, 'friend_chat'], [undefined, 'friend_chat'], [pair, undefined]]) {
    await mirrorFriendChat({ db: untouched, teamId, conversationId, after: chat });
    await mirrorFriendRead({ db: untouched, teamId, conversationId, uid: 'bob', after: { lastReadSeq: 1, generation: 1 } });
  }
  for (const after of [undefined, {}, { ...chat, memberUids: ['alice'] }, { ...chat, memberUids: ['alice', 'alice'] }, { ...chat, lastSeq: -1 }, { ...chat, lastSeq: 1.5 },
    { ...chat, lastChangeSeq: 0 }, { ...chat, lastChangeSeq: '2' }, { ...chat, lastMessage: 'text' }]) await mirrorFriendChat({ db: untouched, teamId: pair, conversationId: 'friend_chat', after });
  for (const after of [undefined, {}, { lastReadSeq: -1 }, { lastReadSeq: 1.5 }, { lastReadSeq: '2' }]) await mirrorFriendRead({ db: untouched, teamId: pair, conversationId: 'friend_chat', uid: 'bob', after });
  await mirrorFriendRead({ db: untouched, teamId: pair, conversationId: 'friend_chat', uid: undefined, after: { lastReadSeq: 1, generation: 1 } });
});

test('summary triggers: wired next to onTeamMessage — same region, service account and database — on the literal friend_chat path, without retry', () => {
  const base = onTeamMessage.__endpoint;
  for (const [trigger, type, document] of [[onFriendChatUpdated, 'updated', 'teams/{teamId}/conversations/friend_chat'],
    [onFriendChatRead, 'written', 'teams/{teamId}/conversations/friend_chat/reads/{uid}']]) {
    const endpoint = trigger.__endpoint;
    assert.deepEqual([endpoint.region, endpoint.serviceAccountEmail, endpoint.availableMemoryMb, endpoint.maxInstances], [base.region, base.serviceAccountEmail, base.availableMemoryMb, base.maxInstances]);
    assert.equal(endpoint.eventTrigger.eventType, `google.cloud.firestore.document.v1.${type}`);
    assert.deepEqual(endpoint.eventTrigger.eventFilters, { database: '(default)', namespace: '(default)' });
    // The conversation id is literal, so a conversation of an ordinary Team never starts the function.
    assert.deepEqual(endpoint.eventTrigger.eventFilterPathPatterns, { document });
    assert.equal(endpoint.eventTrigger.retry, false);
  }
});
