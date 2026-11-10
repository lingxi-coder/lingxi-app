import assert from 'node:assert/strict';
import test from 'node:test';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { z } from 'zod';
import { FriendsError, FriendsStore, OFFER_GRACE_MS, OFFER_LIMIT, OFFER_TTL_MS, friendLink, friendNotifier, pairKey, pairTeamId } from '../src/friends.mjs';
import { FRIEND_COPY, sendTeamPush } from '../src/push.mjs';
import { TeamsError, createTeamsHandler } from '../src/teams.mjs';
import { conversationRecord } from '../src/team-conversations.mjs';

const sha = value => createHash('sha256').update(value).digest('hex');
const caller = { uid: 'alice', name: 'Alice' };
const code = 'aB0_aB0_aB0_aB0_aB0_aB0_aB0_aB0-';

test('friends: a pair key and team id are symmetric, per-epoch and exactly sha256 over the sorted uids', () => {
  assert.equal(pairKey('alice', 'bob'), pairKey('bob', 'alice'));
  assert.equal(pairKey('alice', 'bob'), sha('alice|bob'));
  assert.equal(pairTeamId('bob', 'alice', 1), pairTeamId('alice', 'bob', 1));
  assert.equal(pairTeamId('alice', 'bob', 1), `f_${sha('alice|bob|1').slice(0, 40)}`);
  assert.notEqual(pairTeamId('alice', 'bob', 1), pairTeamId('alice', 'bob', 2));
  assert.notEqual(pairTeamId('alice', 'bob', 1), pairTeamId('alice', 'carol', 1));
  assert.match(pairTeamId('alice', 'bob', 7), /^f_[a-f0-9]{40}$/u);
  assert.equal(friendLink(code), `https://lingxios.net/open/friend/add?code=${code}`);
});

test('friends: strict commands reject unknown fields, malformed identities and ambiguous lookups before any read', async () => {
  const store = new FriendsStore({ doc() { throw new Error('the database must not be touched'); }, runTransaction() { throw new Error('the database must not be touched'); } });
  const invalid = [
    {}, { action: 'unknown' }, { action: 'profile', uid: 'other' }, { action: 'profile', handle: 'ab' }, { action: 'profile', handle: 'a'.repeat(25) },
    { action: 'profile', handle: 'bad-handle' }, { action: 'profile', handle: 'has space' }, { action: 'profile', displayName: ' ' }, { action: 'profile', displayName: 'x'.repeat(81) },
    { action: 'profile', findByEmail: 'yes' }, { action: 'lookup' }, { action: 'lookup', handle: 'alice_w', email: 'a@b.co' }, { action: 'lookup', handle: 'alice_w', code },
    { action: 'lookup', email: 'not-an-email' }, { action: 'lookup', code: 'short' }, { action: 'lookup', teamId: 'team-0001' }, { action: 'lookup', uid: 'bob' },
    { action: 'lookup', handle: 'alice_w', teamId: 'team-0001', uid: 'bob' }, { action: 'lookup', handle: 'alice_w', extra: true },
    // A request carries the proof `lookup` takes; a bare uid is none, with any `via`.
    { action: 'request', targetUid: 'bob', requestId: 'req-00001', via: 'email' }, { action: 'request', targetUid: 'bob', requestId: 'req-00001', via: 'team' },
    { action: 'request', requestId: 'req-00001', via: 'email' }, { action: 'request', requestId: 'req-00001', via: 'handle', uid: 'bob' },
    { action: 'request', requestId: 'req-00001', via: 'team', uid: 'bob' }, { action: 'request', requestId: 'req-00001', via: 'team', teamId: 'team-0001' },
    { action: 'request', requestId: 'req-00001', via: 'email', uid: 'bob', email: 'bob@example.com' }, { action: 'request', requestId: 'req-00001', via: 'handle', handle: 'bob_b', email: 'bob@example.com' },
    { action: 'request', requestId: 'req-00001', via: 'email', email: 'bob@example.com', handle: 'bob_b' }, { action: 'request', requestId: 'req-00001', via: 'handle', handle: 'bob_b', teamId: 'team-0001', uid: 'bob' },
    { action: 'request', requestId: 'req-00001', via: 'code', handle: 'bob_b' }, { action: 'request', requestId: 'short', via: 'email', email: 'bob@example.com' },
    { action: 'request', requestId: 'req-00001', via: 'email', email: 'not-an-email' }, { action: 'request', requestId: 'req-00001', via: 'handle', handle: 'ab' },
    { action: 'request', requestId: 'req-00001', via: 'team', teamId: 'team-0001', uid: 'a/b' }, { action: 'request', requestId: 'req-00001', via: 'team', teamId: 'team-0001', uid: 'a|b' },
    { action: 'request', requestId: 'req-00001', via: 'email', email: 'bob@example.com', note: ' ' }, { action: 'request', requestId: 'req-00001', via: 'email', email: 'bob@example.com', note: 'x'.repeat(201) },
    { action: 'accept' }, { action: 'accept', otherUid: '' }, { action: 'decline', otherUid: 'bob', extra: 1 }, { action: 'redeem', code }, { action: 'redeem', code: 'short', requestId: 'req-00001' },
    { action: 'remove', otherUid: 'x'.repeat(129) }, { action: 'block', otherUid: 'bad\u0000uid' }, { action: 'rotateCode', extra: true }, { action: 'revokeCode', code },
    // Team offers: every id is required and well formed, and nothing else (no role, note or uid under another name) is accepted.
    { action: 'offerTeam', teamId: 'team-0001', friendUid: 'bob' }, { action: 'offerTeam', friendUid: 'bob', requestId: 'req-00001' },
    { action: 'offerTeam', teamId: 'team-0001', requestId: 'req-00001' }, { action: 'offerTeam', teamId: 'short', friendUid: 'bob', requestId: 'req-00001' },
    { action: 'offerTeam', teamId: 'team/0001', friendUid: 'bob', requestId: 'req-00001' }, { action: 'offerTeam', teamId: 'team-0001', friendUid: 'a|b', requestId: 'req-00001' },
    { action: 'offerTeam', teamId: 'team-0001', friendUid: '', requestId: 'req-00001' }, { action: 'offerTeam', teamId: 'team-0001', friendUid: 'bob', requestId: 'short' },
    { action: 'offerTeam', teamId: 'team-0001', friendUid: 'bob', requestId: 'req-00001', role: 'admin' }, { action: 'offerTeam', teamId: 'team-0001', friendUid: 'bob', requestId: 'req-00001', note: 'hi' },
    { action: 'offerTeam', teamId: 'team-0001', otherUid: 'bob', requestId: 'req-00001' },
    { action: 'acceptOffer', offerId: 'team-0001' }, { action: 'acceptOffer', requestId: 'req-00001' }, { action: 'acceptOffer', offerId: 'x/y-0001', requestId: 'req-00001' },
    { action: 'acceptOffer', offerId: 'short', requestId: 'req-00001' }, { action: 'acceptOffer', offerId: 'team-0001', requestId: 'short' },
    { action: 'acceptOffer', offerId: 'team-0001', requestId: 'req-00001', teamId: 'team-0001' },
    { action: 'declineOffer' }, { action: 'declineOffer', offerId: 'short' }, { action: 'declineOffer', offerId: 'team-0001', requestId: 'req-00001' },
    { action: 'declineOffer', otherUid: 'bob' },
  ];
  for (const value of invalid) await assert.rejects(store.execute(caller, value), z.ZodError, JSON.stringify(value));
  await assert.rejects(store.execute({ uid: 'a|b' }, { action: 'rotateCode' }), z.ZodError);
  await assert.rejects(store.execute({ uid: '' }, { action: 'rotateCode' }), z.ZodError);
});

test('friends: a request is valid with exactly one of the three proofs lookup takes', async () => {
  const store = new FriendsStore({ doc() { throw new Error('reached the database'); }, runTransaction() { throw new Error('reached the database'); } });
  for (const proof of [{ via: 'handle', handle: 'Bob_B' }, { via: 'email', email: ' Bob@Example.COM ' }, { via: 'team', teamId: 'team-0001', uid: 'bob' }]) {
    await assert.rejects(store.execute(caller, { action: 'request', requestId: 'req-00001', note: ' hi ', ...proof }), { message: 'reached the database' }, JSON.stringify(proof));
  }
});

test('friends: the offer actions are valid with exactly their fields, and an offer to oneself is refused before any read', async () => {
  const store = new FriendsStore({ doc() { throw new Error('reached the database'); }, runTransaction() { throw new Error('reached the database'); } });
  for (const value of [{ action: 'offerTeam', teamId: 'team-0001', friendUid: 'bob', requestId: 'req-00001' }, { action: 'acceptOffer', offerId: 'team-0001', requestId: 'req-00001' },
    { action: 'declineOffer', offerId: 'team-0001' }]) await assert.rejects(store.execute(caller, value), { message: 'reached the database' }, value.action);
  await assert.rejects(store.execute(caller, { action: 'offerTeam', teamId: 'team-0001', friendUid: 'alice', requestId: 'req-00001' }), { status: 400, code: 'INVALID_INPUT' });
  assert.deepEqual([OFFER_TTL_MS, OFFER_GRACE_MS, OFFER_LIMIT], [7 * 24 * 3600 * 1000, 7 * 24 * 3600 * 1000, 20]);
});

test('friends: a handle is looked up case-folded and an email trimmed and lower-cased', async () => {
  const emails = [], paths = [];
  const db = { doc(path) { paths.push(path); return { get: async () => ({ data: () => undefined }) }; }, runTransaction: async () => null };
  const auth = { getUserByEmail: async value => { emails.push(value); throw Object.assign(new Error(value), { code: 'auth/user-not-found' }); } };
  const store = new FriendsStore(db, Date.now, { auth });
  // The fake transaction accepts the rate limit; each miss is then the same uniform NOT_FOUND.
  await assert.rejects(store.execute(caller, { action: 'lookup', email: '  Bob@Example.COM ' }), { status: 404, code: 'NOT_FOUND' });
  await assert.rejects(store.execute(caller, { action: 'lookup', handle: 'Bob_B' }), { status: 404, code: 'NOT_FOUND' });
  assert.deepEqual(emails, ['bob@example.com']);
  assert.ok(paths.includes('handles/bob_b'));
  // Without an injected Auth the email route fails closed instead of pretending nobody exists.
  await assert.rejects(new FriendsStore(db).execute(caller, { action: 'lookup', email: 'bob@example.com' }), { status: 503, code: 'SERVICE_UNAVAILABLE' });
  const failing = new FriendsStore(db, Date.now, { auth: { getUserByEmail: async () => { throw Object.assign(new Error('quota'), { code: 'auth/quota-exceeded' }); } } });
  await assert.rejects(failing.execute(caller, { action: 'lookup', email: 'bob@example.com' }), { code: 'auth/quota-exceeded' });
});

test('friends: errors are Teams errors with a status and a code, so the Teams handler maps them', () => {
  const error = new FriendsError(404, 'NOT_FOUND');
  assert.ok(error instanceof TeamsError);
  assert.deepEqual([error.status, error.code, error.message], [404, 'NOT_FOUND', 'NOT_FOUND']);
});

test('conversation records: one builder serves trusted creation and pair teams', () => {
  const { record, memberUids, requestDigest } = conversationRecord({ kind: 'dm', title: 'Friends', participants: ['user:bob', 'user:alice'], callerUid: 'alice' });
  assert.deepEqual(memberUids, ['alice', 'bob']);
  assert.deepEqual(record, { kind: 'dm', title: 'Friends', participants: ['user:alice', 'user:bob'], memberUids: ['alice', 'bob'], createdBy: 'alice',
    requestDigest, lastSeq: 0, lastMessage: null, lastChangeSeq: 0, lastChange: null });
  assert.equal(requestDigest, sha(JSON.stringify({ kind: 'dm', title: 'Friends', participants: ['user:alice', 'user:bob'], createdBy: 'alice' })));
  assert.throws(() => conversationRecord({ kind: 'dm', title: 'x', participants: ['user:alice'], callerUid: 'alice' }), { status: 400 });
  assert.throws(() => conversationRecord({ kind: 'dm', title: 'x', participants: ['user:alice', 'user:alice'], callerUid: 'alice' }), { status: 400 });
  assert.throws(() => conversationRecord({ kind: 'dm', title: 'x', participants: ['user:alice', 'user:bob', 'user:carol'], callerUid: 'alice' }), { status: 400 });
  assert.throws(() => conversationRecord({ kind: 'dm', title: 'x', participants: ['user:bob', 'user:carol'], callerUid: 'alice' }), { status: 403 });
  assert.throws(() => conversationRecord({ kind: 'dm', title: 'x', participants: ['user:alice', 'shared_bot:bot'], callerUid: 'alice' }), { status: 400 });
});

// A push device and the stores around it, in the style of team-message-push.test.mjs.
function pushWorld({ locale = 'en-US', cards = {} } = {}) {
  const now = Date.now(), sent = [];
  const device = { id: 'synthetic-device', uid: 'alice', revision: 1, fcmInstallationId: 'synthetic-fid', expiresAt: { toMillis: () => now + 3600000 }, locale, appId: 'com.lingxi.code', platform: 'android' };
  const pushStore = { async active(uid) { return uid === 'alice' ? [device] : []; }, device() { return { async get() { return { data: () => device }; } }; } };
  const db = { doc: path => ({ get: async () => ({ data: () => cards[path] }) }) };
  return { now, sent, notify: friendNotifier({ db, pushStore, messaging: { async send(value) { sent.push(value); } }, clock: () => now }), pushStore, device };
}

test('friend push: generic localized copy, the friend route, no note text, and only while the announced card exists', async () => {
  const request = { kind: 'request', to: 'alice', from: 'bob', name: ' Bob ', key: 'pair:request-1' };
  const en = pushWorld({ cards: { 'users/alice/friends/bob': { state: 'incoming', note: 'a private note' } } });
  await en.notify(request);
  assert.equal(en.sent.length, 1);
  const [message] = en.sent;
  assert.deepEqual([message.data.route, message.data.kind, message.data.title, message.data.body, message.data.recipientUid],
    ['friend/requests', 'friend_request', 'Bob', FRIEND_COPY.request.en, 'alice']);
  assert.ok(!JSON.stringify(message).includes('private note'));
  assert.equal(message.apns.payload.aps.alert.body, FRIEND_COPY.request.en);
  const zh = pushWorld({ locale: 'zh-Hans', cards: { 'users/alice/friends/bob': { state: 'friends' } } });
  await zh.notify({ ...request, kind: 'accepted' });
  assert.equal(zh.sent[0].data.body, FRIEND_COPY.accepted.zh);
  assert.equal(zh.sent[0].data.kind, 'friend_accepted');
  await zh.notify({ ...request, kind: 'linked', key: 'pair:linked-1' });
  assert.equal(zh.sent[1].data.body, FRIEND_COPY.linked.zh);
  assert.notEqual(zh.sent[0].data.notificationId, zh.sent[1].data.notificationId);
  // The request was cancelled (no card), or a request card is no longer 'incoming': nothing is sent.
  const gone = pushWorld({ cards: {} });
  await gone.notify(request);
  const moved = pushWorld({ cards: { 'users/alice/friends/bob': { state: 'friends' } } });
  await moved.notify(request);
  assert.equal(gone.sent.length + moved.sent.length, 0);
  const blank = pushWorld({ cards: { 'users/alice/friends/bob': { state: 'incoming' } } });
  await blank.notify({ ...request, name: '   ' });
  assert.equal(blank.sent[0].data.title, 'LingXi');
  assert.ok(blank.sent[0].android.collapseKey.startsWith('friend:'));
  assert.ok(Buffer.byteLength(blank.sent[0].android.collapseKey) <= 64);
});

test('friend push: a team offer announces the sender, never the team, opens the requests list and is sent only while the offer exists', async () => {
  const offer = { kind: 'offer', to: 'alice', from: 'bob', name: 'Bob', offerId: 'team-0001', key: 'pair:request-9' };
  const live = pushWorld({ cards: { 'users/alice/teamOffers/team-0001': { teamId: 'team-0001', teamName: 'Secret Project', fromUid: 'bob', fromName: 'Bob', role: 'member' } } });
  await live.notify(offer);
  assert.equal(live.sent.length, 1);
  const [message] = live.sent;
  assert.deepEqual([message.data.route, message.data.kind, message.data.title, message.data.body, message.data.recipientUid],
    ['friend/requests', 'friend_offer', 'Bob', FRIEND_COPY.offer.en, 'alice']);
  assert.ok(!JSON.stringify(message).includes('Secret Project'), 'the team name is user-authored text and stays out of the push');
  assert.equal(message.apns.payload.aps.alert.body, 'invited you to join a team');
  const zh = pushWorld({ locale: 'zh-Hans', cards: { 'users/alice/teamOffers/team-0001': { teamId: 'team-0001' } } });
  await zh.notify(offer);
  assert.equal(zh.sent[0].data.body, '邀请你加入团队');
  // The offer was declined, accepted or removed with the friendship before the push was built: nothing is sent. A friend card does not stand in for it.
  const gone = pushWorld({ cards: { 'users/alice/friends/bob': { state: 'friends' } } });
  await gone.notify(offer);
  assert.equal(gone.sent.length, 0);
  // Another team's offer or another request is another push.
  await live.notify({ ...offer, offerId: 'team-0002' });
  assert.equal(live.sent.length, 1);
  await live.notify({ ...offer, key: 'pair:request-10' });
  assert.equal(live.sent.length, 2);
  assert.notEqual(live.sent[0].data.notificationId, live.sent[1].data.notificationId);
});

test('Team push defaults are unchanged and a Friends push overrides route, kind and copy only', async () => {
  const plain = pushWorld(), custom = pushWorld({ locale: 'zh-CN' });
  await sendTeamPush(plain.pushStore, { async send(value) { plain.sent.push(value); } }, { uid: 'alice', title: 'Author', notificationId: 'n1', collapseKey: 'team:c' }, async () => true, () => plain.now);
  assert.deepEqual([plain.sent[0].data.route, plain.sent[0].data.kind, plain.sent[0].data.body], ['team/inbox', 'team_message', 'New message in your team']);
  await sendTeamPush(custom.pushStore, { async send(value) { custom.sent.push(value); } }, { uid: 'alice', title: 'Author', notificationId: 'n2', collapseKey: 'team:c',
    route: 'friend/chat?uid=bob', kind: 'friend_message', copy: FRIEND_COPY.message }, async () => true, () => custom.now);
  assert.deepEqual([custom.sent[0].data.route, custom.sent[0].data.kind, custom.sent[0].data.body], ['friend/chat?uid=bob', 'friend_message', '好友有新消息']);
});

test('Teams handler friends routes: authenticated, size-limited, strictly routed and mapped to the shared error shape', async () => {
  const calls = [];
  const friends = {
    async profile(user) { calls.push(['profile', user.uid]); return { uid: user.uid }; },
    async execute(user, body) {
      calls.push(['execute', user.uid, body]);
      if (body.action === 'accept') throw new FriendsError(404, 'REQUEST_UNAVAILABLE');
      if (body.action === 'profile') throw new z.ZodError([]);
      if (body.action === 'revokeCode') throw new Error('provider detail must not leak');
      return { ok: true };
    },
  };
  const verifyToken = async token => { if (token !== 'valid') throw new Error('invalid'); return { uid: 'alice', name: 'Alice' }; };
  const handler = createTeamsHandler({ store: {}, friends, verifyToken, createCustomToken: async () => '', origins: ['https://lingxios.net'] });
  const without = createTeamsHandler({ store: {}, verifyToken, createCustomToken: async () => '', origins: ['https://lingxios.net'] });
  const send = (target, method, body, { token = 'valid', origin } = {}) => target(new Request('https://test.invalid/v1/friends' + (method === 'GET' ? '/profile' : ''),
    { method, headers: { ...(token ? { Authorization: `Bearer ${token}` } : {}), ...(origin ? { Origin: origin } : {}) }, ...(body === undefined ? {} : { body: typeof body === 'string' ? body : JSON.stringify(body) }) }));
  const profile = await send(handler, 'GET');
  assert.equal(profile.status, 200);
  assert.deepEqual(await profile.json(), { uid: 'alice' });
  assert.equal(profile.headers.get('Cache-Control'), 'no-store');
  assert.equal((await send(handler, 'GET', undefined, { token: null })).status, 401);
  assert.equal((await send(handler, 'GET', undefined, { token: 'bad' })).status, 401);
  assert.equal((await send(handler, 'GET', undefined, { origin: 'https://evil.invalid' })).status, 403);
  assert.equal((await send(handler, 'POST', { action: 'revokeCode' }, { token: null })).status, 401);
  assert.equal((await send(handler, 'POST', { action: 'lookup', handle: 'alice_w' })).status, 200);
  const unavailable = await send(handler, 'POST', { action: 'accept', otherUid: 'bob' });
  assert.equal(unavailable.status, 404);
  assert.deepEqual(await unavailable.json(), { error: { code: 'REQUEST_UNAVAILABLE' } });
  assert.equal((await send(handler, 'POST', { action: 'profile' })).status, 400);
  const unexpected = await send(handler, 'POST', { action: 'revokeCode' });
  assert.equal(unexpected.status, 503);
  assert.deepEqual(await unexpected.json(), { error: { code: 'SERVICE_UNAVAILABLE' } });
  assert.equal((await send(handler, 'POST', '{')).status, 400);
  assert.equal((await send(handler, 'POST', 'x'.repeat(16385))).status, 413);
  assert.equal((await send(handler, 'PUT', { action: 'revokeCode' })).status, 404);
  assert.equal((await send(handler, 'DELETE')).status, 404);
  const other = await handler(new Request('https://test.invalid/v1/friends/other', { method: 'POST', headers: { Authorization: 'Bearer valid' }, body: '{}' }));
  assert.equal(other.status, 404);
  const sub = await handler(new Request('https://test.invalid/v1/friends/profile', { method: 'POST', headers: { Authorization: 'Bearer valid' }, body: '{}' }));
  assert.equal(sub.status, 404);
  // Without a Friends store the routes do not exist.
  assert.equal((await send(without, 'GET')).status, 404);
  assert.equal((await send(without, 'POST', { action: 'revokeCode' })).status, 404);
  assert.ok(calls.every(call => call[1] === 'alice'));
  assert.ok(calls.some(call => call[0] === 'profile'));
});

test('friends: Hosting sends the Friends API to teamsApi ahead of the catalog catch-all, the link page sends no referrer, and the raw code field is index-exempt', () => {
  const config = JSON.parse(readFileSync(new URL('../../../firebase.json', import.meta.url), 'utf8'));
  const sources = config.hosting.rewrites.map(rewrite => rewrite.source);
  for (const source of ['/v1/friends', '/v1/friends/**']) {
    assert.ok(sources.includes(source) && sources.indexOf(source) < sources.indexOf('/v1/**'), source);
    assert.equal(config.hosting.rewrites[sources.indexOf(source)].function.functionId, 'teamsApi');
  }
  const page = config.hosting.headers.find(header => header.source === '/open/friend/add');
  assert.deepEqual(Object.fromEntries(page.headers.map(header => [header.key, header.value])), { 'Referrer-Policy': 'no-referrer', 'Cache-Control': 'no-store' });
  const indexes = JSON.parse(readFileSync(new URL('../../../infra/firebase/firestore.teams.indexes.json', import.meta.url), 'utf8'));
  assert.ok(indexes.fieldOverrides.some(item => item.collectionGroup === 'friendCodes' && item.fieldPath === 'code' && item.indexes.length === 0));
  // The scheduled prune of expired team offers is a collection group query on `expiresAt`, which needs that scope enabled.
  const offers = indexes.fieldOverrides.find(item => item.collectionGroup === 'teamOffers' && item.fieldPath === 'expiresAt');
  assert.deepEqual(offers.indexes.map(index => [index.order, index.queryScope]).sort(), [['ASCENDING', 'COLLECTION'], ['ASCENDING', 'COLLECTION_GROUP']]);
});
