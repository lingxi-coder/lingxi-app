import assert from 'node:assert/strict';
import { after, before, beforeEach, test } from 'node:test';
import { readFileSync } from 'node:fs';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getFirestore } from 'firebase-admin/firestore';
import { initializeTestEnvironment } from '@firebase/rules-unit-testing';
import { FriendsStore } from '../src/friends.mjs';
import { PAIR_CONVERSATION_ID, TeamsStore } from '../src/teams.mjs';
import { TeamMessagesStore } from '../src/team-messages.mjs';
import { mirrorFriendChat, mirrorFriendRead } from '../src/friend-summary.mjs';

const projectId = 'demo-lingxi-teams-t0', options = { skip: !process.env.FIRESTORE_EMULATOR_HOST, timeout: 120000 };
const alice = { uid: 'alice', name: 'Alice' }, bob = { uid: 'bob', name: 'Bob' }, carol = { uid: 'carol', name: 'Carol' };
const FIELDS = ['lastSeq', 'lastMessage', 'lastChangeSeq', 'lastReadSeq'];
let app, db, environment, friends, teams, messages, now, serial = 0;

before(async () => {
  if (!process.env.FIRESTORE_EMULATOR_HOST) return;
  const [host, port] = process.env.FIRESTORE_EMULATOR_HOST.split(':');
  assert.ok(['127.0.0.1', 'localhost'].includes(host));
  environment = await initializeTestEnvironment({ projectId, firestore: { host, port: Number(port),
    rules: readFileSync(new URL('../../../infra/firebase/firestore.default.rules', import.meta.url), 'utf8') } });
  app = initializeApp({ projectId }, 'friend-summary-tests'); db = getFirestore(app);
});
beforeEach(async () => {
  if (!environment) return;
  await environment.clearFirestore(); now = Date.now();
  friends = new FriendsStore(db, () => now); teams = new TeamsStore(db, () => now); messages = new TeamMessagesStore(db, () => now);
});
after(async () => { await environment?.cleanup(); await db?.terminate(); if (app) await deleteApp(app); });

const data = async path => (await db.doc(path).get()).data();
const touched = async path => { const snapshot = await db.doc(path).get(); return snapshot.exists ? snapshot.updateTime.toMillis() : null; };
const chatPath = teamId => `teams/${teamId}/conversations/${PAIR_CONVERSATION_ID}`;
const cardPath = (owner, peer) => `users/${owner}/friends/${peer}`;
const cards = async () => [await data(cardPath('alice', 'bob')), await data(cardPath('bob', 'alice'))];
const stamps = () => Promise.all([touched(cardPath('alice', 'bob')), touched(cardPath('bob', 'alice'))]);
// Only the summary fields a card carries; a field that was never written is absent, not undefined.
const summary = async (owner, peer) => Object.fromEntries(Object.entries(await data(cardPath(owner, peer))).filter(([key]) => FIELDS.includes(key)));
const unread = card => Math.max(0, (card.lastSeq ?? 0) - (card.lastReadSeq ?? 0));
const id = prefix => `${prefix}-${String(++serial).padStart(6, '0')}`;
// Friends through the friend link, instantly; returns the pair team.
async function befriend(owner, visitor) {
  await friends.profile(visitor);
  await friends.execute(visitor, { action: 'redeem', code: (await friends.profile(owner)).friendCode.code, requestId: id('request') });
  return (await data(cardPath(owner.uid, visitor.uid))).teamId;
}
const thread = teamId => ({ teamId, generation: 1, conversationId: PAIR_CONVERSATION_ID });
// A write to the chat, and the conversation as that write left it: exactly what the trigger is handed. Keeping these lets
// the tests deliver events twice, late and in any order, as Firestore may.
async function write(teamId, actor, messageId, operation = 'create', text = `text of ${messageId}`) {
  await messages.execute(actor, { action: 'mutate', ...thread(teamId), messageId, changeId: id('change'), operation, text: operation === 'recall' ? '' : text });
  return data(chatPath(teamId));
}
async function mark(teamId, actor, lastReadSeq) {
  await messages.execute(actor, { action: 'read', ...thread(teamId), lastReadSeq });
  return data(`${chatPath(teamId)}/reads/${actor.uid}`);
}
const chat = (teamId, event, conversationId = PAIR_CONVERSATION_ID) => mirrorFriendChat({ db, teamId, conversationId, after: event });
const read = (teamId, uid, event, conversationId = PAIR_CONVERSATION_ID) => mirrorFriendRead({ db, teamId, conversationId, uid, after: event });

test('chat events: create, edit and recall carry the conversation pointer, exactly as stored, to both cards', options, async () => {
  const team = await befriend(alice, bob);
  await befriend(carol, alice);
  const start = await cards();
  assert.deepEqual([await summary('alice', 'bob'), await summary('bob', 'alice')], [{}, {}], 'a new friendship has no summary: absent means no message and nothing read');

  const s1 = await write(team, alice, 'message01');
  await chat(team, s1);
  const created = await cards();
  assert.deepEqual(created[0], { ...start[0], lastSeq: 1, lastMessage: s1.lastMessage, lastChangeSeq: 1 }, 'every other field of the card is untouched');
  assert.deepEqual(created[1], { ...start[1], lastSeq: 1, lastMessage: s1.lastMessage, lastChangeSeq: 1 });
  assert.deepEqual([s1.lastMessage.id, s1.lastMessage.seq, s1.lastMessage.author], ['message01', 1, { kind: 'user', uid: 'alice' }]);

  const s2 = await write(team, bob, 'message02'); await chat(team, s2);
  assert.deepEqual(await summary('alice', 'bob'), { lastSeq: 2, lastMessage: s2.lastMessage, lastChangeSeq: 2 });
  assert.deepEqual(await summary('bob', 'alice'), { lastSeq: 2, lastMessage: s2.lastMessage, lastChangeSeq: 2 });

  // Editing an older message leaves the pointer on the newest one; only the change counter moves.
  const s3 = await write(team, alice, 'message01', 'edit', 'first, edited'); await chat(team, s3);
  assert.deepEqual(s3.lastMessage, s2.lastMessage);
  assert.deepEqual(await summary('alice', 'bob'), { lastSeq: 2, lastMessage: s2.lastMessage, lastChangeSeq: 3 });

  // Editing the newest changes its preview.
  const s4 = await write(team, bob, 'message02', 'edit', 'second, edited'); await chat(team, s4);
  assert.notEqual(s4.lastMessage.preview, s2.lastMessage.preview);
  for (const [owner, peer] of [['alice', 'bob'], ['bob', 'alice']]) assert.deepEqual(await summary(owner, peer), { lastSeq: 2, lastMessage: s4.lastMessage, lastChangeSeq: 4 });

  // Recalling the newest blanks the preview and marks the pointer, and the card holds that very pointer.
  const s5 = await write(team, bob, 'message02', 'recall'); await chat(team, s5);
  assert.deepEqual(s5.lastMessage, { ...s4.lastMessage, preview: '', recalled: true });
  for (const [owner, peer] of [['alice', 'bob'], ['bob', 'alice']]) assert.deepEqual(await summary(owner, peer), { lastSeq: 2, lastMessage: s5.lastMessage, lastChangeSeq: 5 });

  // The next message replaces the whole pointer: nothing of the recalled one is merged into it.
  const s6 = await write(team, alice, 'message03'); await chat(team, s6);
  const next = await summary('bob', 'alice');
  assert.deepEqual(next, { lastSeq: 3, lastMessage: s6.lastMessage, lastChangeSeq: 6 });
  assert.equal('recalled' in next.lastMessage, false);

  // Another friendship of the same user is not involved.
  assert.deepEqual([await summary('alice', 'carol'), await summary('carol', 'alice')], [{}, {}]);
});

test('chat events: duplicates, late and out-of-order delivery never move a card back, and a lost event is healed by the next', options, async () => {
  const team = await befriend(alice, bob);
  const s1 = await write(team, alice, 'message01'), s2 = await write(team, bob, 'message02'), s3 = await write(team, bob, 'message02', 'edit', 'second, edited'),
    s4 = await write(team, bob, 'message02', 'recall'), s5 = await write(team, alice, 'message03');
  // The newest event first; everything older, and the same event again, arrive after it and change nothing.
  await chat(team, s5);
  const settled = { lastSeq: 3, lastMessage: s5.lastMessage, lastChangeSeq: 5 };
  assert.deepEqual([await summary('alice', 'bob'), await summary('bob', 'alice')], [settled, settled]);
  const stamped = await stamps();
  for (const late of [s3, s1, s4, s2, s5, s5]) await chat(team, late);
  assert.deepEqual([await summary('alice', 'bob'), await summary('bob', 'alice')], [settled, settled]);
  assert.deepEqual(await stamps(), stamped, 'an ignored event writes nothing');

  // A later edit or recall of the same message that arrives before an earlier event wins over it.
  const u = await befriend(carol, bob);
  const u1 = await write(u, carol, 'message01'), u2 = await write(u, carol, 'message01', 'edit', 'edited'), u3 = await write(u, carol, 'message01', 'recall');
  await chat(u, u3); await chat(u, u1); await chat(u, u2);
  assert.deepEqual(await summary('bob', 'carol'), { lastSeq: 1, lastMessage: u3.lastMessage, lastChangeSeq: 3 });
  assert.equal((await summary('carol', 'bob')).lastMessage.recalled, true);

  // A lost event costs nothing: the next one carries the whole pointer.
  const v = await befriend(alice, carol);
  const v1 = await write(v, alice, 'message01'); await write(v, carol, 'message02'); const v3 = await write(v, carol, 'message03');
  await chat(v, v1); await chat(v, v3);
  assert.deepEqual(await summary('alice', 'carol'), { lastSeq: 3, lastMessage: v3.lastMessage, lastChangeSeq: 3 });

  // The same event handled twice at once is applied once.
  const w1 = await write(team, alice, 'message04');
  await Promise.all([chat(team, w1), chat(team, w1)]);
  assert.deepEqual(await summary('bob', 'alice'), { lastSeq: 4, lastMessage: w1.lastMessage, lastChangeSeq: 6 });

  // Defence in depth: an event that is newer by change but behind in lastSeq (the store cannot produce one) cannot lower it.
  await chat(team, { ...w1, lastSeq: 1, lastChangeSeq: 99 });
  const card = await data(cardPath('alice', 'bob'));
  assert.deepEqual([card.lastSeq, card.lastChangeSeq], [4, 99]);
});

test('read marks: a mark goes forward, on its own user\'s card only; unread is lastSeq minus lastReadSeq, never below zero', options, async () => {
  const team = await befriend(alice, bob);
  let latest;
  for (const messageId of ['message01', 'message02', 'message03']) latest = await write(team, alice, messageId);
  await chat(team, latest);
  assert.equal(unread(await data(cardPath('bob', 'alice'))), 3);

  const two = await mark(team, bob, 2);
  assert.deepEqual(two, { lastReadSeq: 2, generation: 1 });
  await read(team, 'bob', two);
  const bobs = await data(cardPath('bob', 'alice'));
  assert.equal(bobs.lastReadSeq, 2);
  assert.equal(unread(bobs), 1);
  assert.equal('lastReadSeq' in (await data(cardPath('alice', 'bob'))), false, 'the mark is the reader\'s own and never reaches the friend\'s card');
  assert.deepEqual(await summary('bob', 'alice'), { lastSeq: 3, lastMessage: latest.lastMessage, lastChangeSeq: 3, lastReadSeq: 2 }, 'the chat summary is untouched');

  // A mark that is late, repeated or lower does nothing; one from another membership generation is not this card's.
  const stamped = await touched(cardPath('bob', 'alice'));
  await read(team, 'bob', { lastReadSeq: 1, generation: 1 }); await read(team, 'bob', two); await read(team, 'bob', { lastReadSeq: 3, generation: 2 });
  assert.equal((await data(cardPath('bob', 'alice'))).lastReadSeq, 2);
  assert.equal(await touched(cardPath('bob', 'alice')), stamped, 'an ignored mark writes nothing');

  const three = await mark(team, bob, 3); await read(team, 'bob', three);
  assert.equal(unread(await data(cardPath('bob', 'alice'))), 0);
  // Each user's mark lands on that user's card.
  await read(team, 'alice', await mark(team, alice, 3));
  assert.deepEqual([(await data(cardPath('alice', 'bob'))).lastReadSeq, (await data(cardPath('bob', 'alice'))).lastReadSeq], [3, 3]);

  // Someone who is not in the friendship has no card to receive anything.
  await read(team, 'carol', { lastReadSeq: 3, generation: 1 });
  assert.equal((await db.collection('users/carol/friends').get()).size, 0);

  // Events arrive in any order: a mark ahead of the pointer is zero unread, not negative, and the pointer still lands.
  const u = await befriend(carol, alice);
  const u1 = await write(u, carol, 'message01');
  await read(u, 'alice', await mark(u, alice, 1));
  const early = await data(cardPath('alice', 'carol'));
  assert.deepEqual([early.lastSeq, early.lastReadSeq, unread(early)], [undefined, 1, 0]);
  await chat(u, u1);
  const late = await data(cardPath('alice', 'carol'));
  assert.deepEqual([late.lastSeq, late.lastReadSeq, unread(late)], [1, 1, 0]);
  assert.equal(unread({ lastSeq: 1, lastReadSeq: 5 }), 0);
});

test('no write after the friendship ended: remove and block, and an older friendship\'s events never reach new cards', options, async () => {
  const old = await befriend(alice, bob);
  const m1 = await write(old, alice, 'message01'); await chat(old, m1);
  const m2 = await write(old, bob, 'message02'); // its event is still on the way
  const bobsMark = await mark(old, bob, 2);
  await friends.execute(alice, { action: 'remove', otherUid: 'bob' });
  assert.deepEqual(await stamps(), [null, null]);
  await chat(old, m2); await read(old, 'bob', bobsMark);
  assert.deepEqual(await stamps(), [null, null], 'late events create or revive no card');

  // A card that is no longer a friendship, whatever team it names, is not written either.
  await db.doc(cardPath('alice', 'bob')).set({ state: 'outgoing', displayName: 'Bob', teamId: old, since: new Date(now), updatedAt: new Date(now) });
  await chat(old, m2);
  assert.deepEqual(await summary('alice', 'bob'), {});
  await db.doc(cardPath('alice', 'bob')).delete();

  // Friends again: a new team and new cards with no summary. The old team's events, even the newest, do not reach them.
  const fresh = await befriend(bob, alice);
  assert.notEqual(fresh, old);
  assert.deepEqual([await summary('alice', 'bob'), await summary('bob', 'alice')], [{}, {}]);
  for (const late of [m2, m1]) await chat(old, late);
  await read(old, 'bob', bobsMark); await read(old, 'alice', { lastReadSeq: 2, generation: 1 });
  assert.deepEqual([await summary('alice', 'bob'), await summary('bob', 'alice')], [{}, {}]);
  // The new chat counts from its own sequence, however far the old one had come.
  const n1 = await write(fresh, bob, 'message01'); await chat(fresh, n1);
  assert.deepEqual(await summary('alice', 'bob'), { lastSeq: 1, lastMessage: n1.lastMessage, lastChangeSeq: 1 });
  const n2 = await write(fresh, alice, 'message02');
  await friends.execute(bob, { action: 'block', otherUid: 'alice' });
  assert.deepEqual(await stamps(), [null, null]);
  await chat(fresh, n2); await read(fresh, 'alice', { lastReadSeq: 1, generation: 1 });
  assert.deepEqual(await stamps(), [null, null], 'nothing is written after a block either');
});

test('non-pair data is ignored: an ordinary Team\'s friend_chat, other conversations, an f_ id without a pair team, and unusable events', options, async () => {
  const team = await befriend(alice, bob);
  const good = await write(team, alice, 'message01'), start = await cards();
  let stamped = await stamps();
  const unchanged = async () => { assert.deepEqual(await cards(), start); assert.deepEqual(await stamps(), stamped); };

  // An ordinary Team may have a conversation with that id, even between the same two people.
  await teams.execute(alice, { action: 'create', teamId: 'plain-team', name: 'Plain' });
  const { code } = await teams.execute(alice, { action: 'invite', teamId: 'plain-team', requestId: 'invite01', role: 'member', maxUses: 1, ttlSeconds: 3600 });
  await teams.execute(bob, { action: 'join', code, requestId: 'join-bob' });
  await messages.execute(alice, { action: 'create', teamId: 'plain-team', generation: 1, conversationId: PAIR_CONVERSATION_ID, kind: 'dm', title: 'Plain', participantUids: ['alice', 'bob'] });
  await messages.execute(alice, { action: 'mutate', ...thread('plain-team'), messageId: 'plain-01', changeId: 'plain-change-01', operation: 'create', text: 'Team text' });
  await chat('plain-team', await data(chatPath('plain-team')));
  await read('plain-team', 'bob', await mark('plain-team', bob, 1));
  await unchanged();

  // Another conversation of the pair team (a shared Bot chat, say) is not the friend chat.
  await chat(team, { ...good, lastSeq: 9, lastChangeSeq: 9 }, 'bot_chat_01');
  await read(team, 'bob', { lastReadSeq: 1, generation: 1 }, 'bot_chat_01');
  await unchanged();

  // An f_ id is reserved, but only a team of kind 'pair' is a friendship, even if a card were to name it.
  for (const [teamId, record] of [['f_handmade_one', { name: 'Hand', state: 'active' }], ['f_handmade_two', { name: 'Hand', state: 'active', kind: 'team' }]]) {
    await db.doc(`teams/${teamId}`).set(record);
    await db.doc(chatPath(teamId)).set({ ...good, lastSeq: 9, lastChangeSeq: 9 });
    await db.doc(cardPath('alice', 'bob')).update({ teamId });
    await db.doc(cardPath('bob', 'alice')).update({ teamId });
    await chat(teamId, await data(chatPath(teamId))); await read(teamId, 'bob', { lastReadSeq: 5, generation: 1 });
    assert.deepEqual([await summary('alice', 'bob'), await summary('bob', 'alice')], [{}, {}]);
  }
  await db.doc(cardPath('alice', 'bob')).update({ teamId: team });
  await db.doc(cardPath('bob', 'alice')).update({ teamId: team });
  assert.deepEqual(await cards(), start);
  stamped = await stamps();

  // Events that carry nothing usable are dropped, and a deleted read mark is not a mark.
  for (const bad of [undefined, {}, { ...good, memberUids: ['alice'] }, { ...good, memberUids: ['alice', 'alice'] }, { ...good, memberUids: ['alice', 'bob', 'carol'] },
    { ...good, lastSeq: -1 }, { ...good, lastSeq: 1.5 }, { ...good, lastChangeSeq: 0 }, { ...good, lastChangeSeq: '9' }, { ...good, lastMessage: 'text' }]) await chat(team, bad);
  for (const bad of [undefined, {}, { lastReadSeq: -1, generation: 1 }, { lastReadSeq: 1.5, generation: 1 }, { lastReadSeq: '2', generation: 1 }, { lastReadSeq: 2 }]) await read(team, 'bob', bad);
  await unchanged();
  assert.deepEqual(await cards(), start);
});
