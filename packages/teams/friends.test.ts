import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { parseDeepLink } from '../deep-links/index';
import { friendsErrorText } from './errors';
import {
  badgeCount, badgeText, BADGE_CAP, chatEnded, classifyQuery, compareCards, compareFriends, compareRequests, confirmBody, definitive, FRIEND_CHAT, friendCodeOf, friendCollator,
  friendLink, friendsErrorKey, handleValid, lookupBody, matchesFilter, NAME_MAX, nameValid, newRequestId, NOTE_MAX, noteValid, relationAction, SECTION_ROWS, sectionCards, unreadOf,
  type FriendCard, type Proof,
} from './friends';

// The shared vectors every client loads and none adjusts (docs/FRIENDS-CLIENTS.md, section 5).
const vectors = JSON.parse(readFileSync(new URL('./fixtures/friends-vectors.json', import.meta.url), 'utf8'));
const strings = (name: string) => JSON.parse(readFileSync(new URL(`../../resources/translations/${name}.json`, import.meta.url), 'utf8')) as Record<string, string>;

// The collators of every locale the apps show (D5): one order for the vectors' ASCII names.
const locales = ['en-US', 'zh-CN', 'zh-TW', 'ja-JP', 'ko-KR'];
interface Row { uid: string; state?: FriendCard['state']; displayName?: string; since?: number; lastAtMs?: number; handle?: string }
const card = (row: Row): FriendCard => ({
  uid: row.uid, state: row.state ?? 'friends', displayName: row.displayName ?? '', since: row.since ?? 0, ...(row.handle ? { handle: row.handle } : {}),
  ...(row.lastAtMs === undefined ? {} : { lastMessage: { id: 'm', seq: 1, at: new Date(row.lastAtMs).toISOString(), author: { kind: 'user' as const, uid: 'x' }, preview: 'hi' } }),
});
const uids = (cards: { uid: string }[]) => cards.map(value => value.uid);

test('limits and ids are the vectors\'', () => {
  const { limits } = vectors;
  assert.equal(NAME_MAX, limits.displayNameMax);
  assert.equal(NOTE_MAX, limits.noteMax);
  assert.equal(SECTION_ROWS, limits.sectionRows);
  assert.equal(BADGE_CAP, limits.badgeCap);
  assert.equal(FRIEND_CHAT, limits.chatConversationId);
  const pattern = new RegExp(vectors.requestId.pattern, 'u');
  for (const id of vectors.requestId.valid) assert.ok(pattern.test(id), id);
  for (const id of vectors.requestId.invalid) assert.ok(!pattern.test(id), id);
  assert.match(newRequestId(), pattern);
  assert.notEqual(newRequestId(), newRequestId());
});

test('handle: the vectors\' pattern, valid and invalid', () => {
  for (const handle of vectors.handle.valid) assert.ok(handleValid(handle), handle);
  for (const handle of vectors.handle.invalid) assert.ok(!handleValid(handle), handle);
});

test('lengthRule: UTF-16 code units, the name trimmed', () => {
  assert.ok(vectors.lengthRule.length >= 8);
  for (const row of vectors.lengthRule) {
    const value = row.unit.repeat(row.count);
    assert.equal(row.field === 'displayName' ? nameValid(value) : noteValid(value), row.valid, `${row.field} ${row.unit}×${row.count}`);
  }
  assert.ok(!nameValid(''));
});

test('sort.friends: last activity, then name at primary strength, then uid; one order in every locale', () => {
  const { input, expect } = vectors.sort.friends;
  for (const locale of locales) assert.deepEqual(uids(input.map(card).sort(compareFriends(friendCollator(locale)))), expect, locale);
});

test('sort.cards: incoming, outgoing, then friends; the whole collection and its slices', () => {
  const { input, expect } = vectors.sort.cards;
  for (const locale of locales) {
    const collator = friendCollator(locale);
    assert.deepEqual(uids(input.map(card).sort(compareCards(collator))), expect, locale);
    const sections = sectionCards(input.map(card), collator);
    assert.deepEqual([...uids(sections.received), ...uids(sections.sent), ...uids(sections.friends)], expect, locale);
    assert.ok(sections.received.every(value => value.state === 'incoming') && sections.sent.every(value => value.state === 'outgoing') && sections.friends.every(value => value.state === 'friends'));
  }
});

test('sort.requests: incoming before outgoing, then newest, then uid', () => {
  const { input, expect } = vectors.sort.requests;
  assert.deepEqual(uids(input.map(card).sort(compareRequests)), expect);
});

test('filter: folded contains-match on name or handle, a leading @ dropped', () => {
  const friends = vectors.filter.friends.map(card);
  for (const row of vectors.filter.cases) assert.deepEqual(uids(friends.filter((value: FriendCard) => matchesFilter(value, row.query))), row.expect, `${row.query}: ${row.note}`);
});

test('unread: max(0, lastSeq - lastReadSeq), a missing counter is 0', () => {
  for (const row of vectors.unread) assert.equal(unreadOf(row.card), row.expect, row.note);
});

test('badge: incoming plus unread, nothing at 0, 99+ from 100', () => {
  for (const row of vectors.badge) {
    assert.equal(badgeCount(row.cards), row.expectCount, row.note);
    assert.equal(badgeText(row.expectCount), row.expectText, row.note);
  }
  // An open chat counted as read (D13) leaves the other cards alone.
  assert.equal(badgeCount([{ state: 'incoming' }, { state: 'friends', lastSeq: 5 }, { state: 'friends', lastSeq: 3 }], value => value.lastSeq === 5 ? 0 : unreadOf(value)), 4);
});

test('relationAction: one action per relation', () => {
  for (const [relation, action] of Object.entries(vectors.relationAction)) assert.equal(relationAction(relation as never), action);
});

test('query: one field, three kinds; lookup and confirm bodies', () => {
  assert.ok(vectors.query.length >= 30);
  for (const row of vectors.query) {
    const query = classifyQuery(row.input);
    assert.deepEqual(query, row.expect, `${JSON.stringify(row.input)}: ${row.note}`);
    if (query.kind === 'invalid') { assert.equal(row.lookup, undefined); continue; }
    assert.deepEqual(lookupBody(query), row.lookup, row.note);
    const { requestId, ...confirm } = confirmBody(query, 'request-id-1');
    assert.equal(requestId, 'request-id-1');
    assert.deepEqual(confirm, row.confirm, row.note);
  }
});

test('confirmBody: a note rides a request, never a redeem; a team member is a proof', () => {
  const handle: Proof = { kind: 'handle', handle: 'sam' };
  assert.deepEqual(confirmBody(handle, 'abcdefgh', '  hi  '), { action: 'request', via: 'handle', handle: 'sam', requestId: 'abcdefgh', note: 'hi' });
  assert.deepEqual(confirmBody(handle, 'abcdefgh', '   '), { action: 'request', via: 'handle', handle: 'sam', requestId: 'abcdefgh' });
  assert.deepEqual(confirmBody({ kind: 'code', code: 'c'.repeat(32) }, 'abcdefgh', 'ignored'), { action: 'redeem', code: 'c'.repeat(32), requestId: 'abcdefgh' });
  const member: Proof = { kind: 'team', teamId: 'team-1', uid: 'bob' };
  assert.deepEqual(lookupBody(member), { action: 'lookup', teamId: 'team-1', uid: 'bob' });
  assert.deepEqual(confirmBody(member, 'abcdefgh', 'hello'), { action: 'request', via: 'team', teamId: 'team-1', uid: 'bob', requestId: 'abcdefgh', note: 'hello' });
});

test('friendCodeOf: a bare code or a friend link with exactly one code parameter', () => {
  const code = 'aB0_aB0_aB0_aB0_aB0_aB0_aB0_aB0-';
  assert.equal(friendCodeOf(` ${code}\n`), code);
  for (const url of [`lingxi://friend/add?code=${code}`, `https://lingxios.net/open/friend/add?code=${code}`, `https://lingxios.net/friends/add?code=${code}`]) assert.equal(friendCodeOf(url), code, url);
  for (const url of [`https://lingxios.net/open/team/join?code=${code}`, `https://lingxios.net/friends/add?code=${code}&x=1`, `https://lingxios.net/friends/add?code=${code}#x`, `https://user@lingxios.net/friends/add?code=${code}`,
    `https://lingxios.net:8443/friends/add?code=${code}`, `http://lingxios.net/friends/add?code=${code}`, 'lingxi://friend/add', 'lingxi://friend/add?code=short']) assert.equal(friendCodeOf(url), null, url);
});

test('errors: the code -> key map, which are definitive, and the English and Chinese texts', () => {
  assert.ok(vectors.errors.length >= 20);
  const en = strings('en'), zh = strings('zh-Hans');
  for (const row of vectors.errors) {
    assert.equal(friendsErrorKey(row.code), row.key, row.code);
    assert.equal(definitive(row.code), row.definitive, row.code);
    assert.equal(friendsErrorText(row.code, false), en[row.key], row.code);
    assert.equal(friendsErrorText(row.code, true), zh[row.key], row.code);
    assert.equal(chatEnded(row.code), row.key === 'friends_chat_unavailable', row.code);
  }
});

test('links.typed: only a parsed friend route is a destination', () => {
  const typed = (url: string) => { try { return friendLink(parseDeepLink(url)); } catch { return null; } };
  assert.ok(vectors.links.typed.length >= 25);
  for (const row of vectors.links.typed) assert.deepEqual(typed(row.url), row.expect, row.url);
});
