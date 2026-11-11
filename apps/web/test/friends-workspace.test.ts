import assert from 'node:assert/strict';
import test from 'node:test';
import type { LookupResult, Proof } from '../../../packages/teams/friends';
import { conversation, message } from './support/fakeTeams';
import { block, card, friendsFixture, offer, pairTeamId, pointer, profile, settle, uid, until } from './support/fakeFriends';

const handle: Proof = { kind: 'handle', handle: 'sam' };
const found = (more: Partial<LookupResult> = {}): LookupResult => ({ uid: 'sam', displayName: 'Sam', relation: 'none', ...more });
const ready = (snapshot: () => { loading: boolean; blocksLoading: boolean }) => until(() => !snapshot().loading && !snapshot().blocksLoading, 'both listeners');

// ── The listeners (D2) ──

test('the workspace runs the three owner-only listeners and nothing per friend, and closing releases them (D2, D19)', async context => {
  const { fake, workspace, snapshot } = friendsFixture(f => { f.cards = [card('bob'), card('carol', { state: 'incoming' }), card('dave', { state: 'outgoing' })]; f.blocks = [block('erin', 5)]; });
  context.after(() => workspace.close());
  assert.equal(snapshot().loading, true); assert.equal(snapshot().uid, uid);
  assert.deepEqual(fake.open, { cards: 1, blocks: 1, offers: 1 });
  await ready(snapshot);
  assert.deepEqual(snapshot().cards.map(value => value.uid), ['bob', 'carol', 'dave'], 'unsorted: each view sorts with its own locale');
  assert.equal(snapshot().badge, 1, 'only the incoming request counts');
  assert.deepEqual(fake.open, { cards: 1, blocks: 1, offers: 1 }, 'no listener per friend');
  workspace.close();
  assert.deepEqual(fake.open, { cards: 0, blocks: 0, offers: 0 });
  assert.equal(await workspace.respond('carol', 'accept'), 'INVALID_SESSION', 'nothing is sent once the account is gone');
  assert.deepEqual(await workspace.confirm(handle, found(), ''), { error: 'INVALID_SESSION' });
  assert.deepEqual(await workspace.lookup(handle), { error: 'INVALID_SESSION' });
  assert.equal(fake.commands.length, 0);
});

test('blocks are newest first; a failing listener is shown and retry restarts both', async context => {
  const { fake, workspace, snapshot } = friendsFixture(f => { f.cards = [card('bob')]; f.blocks = [block('a', 1), block('b', 9), block('c', 5), block('d', 5)]; });
  context.after(() => workspace.close());
  await ready(snapshot);
  assert.deepEqual(snapshot().blocks.map(value => value.uid), ['b', 'c', 'd', 'a'], 'the newest block first, a tie by uid');

  fake.failCards('SERVICE_UNAVAILABLE'); fake.failBlocks('SERVICE_UNAVAILABLE');
  assert.equal(snapshot().loadError, 'SERVICE_UNAVAILABLE'); assert.equal(snapshot().blocksError, 'SERVICE_UNAVAILABLE'); assert.equal(snapshot().loading, false);
  const profileReads = fake.profileCalls;
  workspace.retry();
  assert.deepEqual(fake.open, { cards: 1, blocks: 1, offers: 1 }, 'the failed watches were replaced, not stacked');
  assert.equal(snapshot().loading, true); assert.equal(snapshot().loadError, '');
  await ready(snapshot);
  assert.equal(snapshot().cards.length, 1); assert.equal(snapshot().blocksError, '');
  assert.equal(fake.profileCalls, profileReads, 'a profile that was read is not read again');
});

test('a listener that failed before the retry cannot report into the new one', async context => {
  const { fake, workspace, snapshot } = friendsFixture(f => { f.cards = [card('bob')]; });
  context.after(() => workspace.close());
  await ready(snapshot);
  const [stale] = fake.cardWatches;
  workspace.retry();
  stale!.failed('SERVICE_UNAVAILABLE'); stale!.next([card('ghost')]);
  assert.equal(snapshot().loadError, '');
  await ready(snapshot);
  assert.deepEqual(snapshot().cards.map(value => value.uid), ['bob']);
});

// ── The profile (D2, D11) ──

test('the profile is read on open and on every open of its screens; a failure never blocks the list', async context => {
  const { fake, workspace, snapshot } = friendsFixture(f => { f.cards = [card('bob')]; f.profileError = 'SERVICE_UNAVAILABLE'; });
  context.after(() => workspace.close());
  await ready(snapshot);
  await until(() => snapshot().profileError !== '', 'the profile error');
  assert.equal(snapshot().profile, undefined); assert.equal(snapshot().profileError, 'SERVICE_UNAVAILABLE'); assert.equal(snapshot().cards.length, 1);
  fake.profileError = undefined;
  await workspace.refreshProfile();
  assert.equal(snapshot().profile?.displayName, 'Alice'); assert.equal(snapshot().profileError, ''); assert.equal(snapshot().profileLoading, false);
  const reads = fake.profileCalls;
  await workspace.refreshProfile();
  assert.equal(fake.profileCalls, reads + 1, 'every open reads it again');
});

test('saving the profile sends one action with only the changes and shows the answer; an error is returned', async context => {
  const { fake, workspace, snapshot } = friendsFixture(f => { f.cards = [card('bob')]; });
  context.after(() => workspace.close());
  await until(() => !!snapshot().profile, 'the profile');
  fake.replies.set('profile', profile({ displayName: 'Ally', findByEmail: true }));
  assert.equal(await workspace.saveProfile({ displayName: 'Ally', findByEmail: true }), '');
  assert.deepEqual(fake.executed('profile'), [{ action: 'profile', displayName: 'Ally', findByEmail: true }]);
  assert.equal(snapshot().profile?.displayName, 'Ally'); assert.equal(snapshot().profile?.findByEmail, true);
  fake.fail('profile', 'HANDLE_TAKEN');
  assert.equal(await workspace.saveProfile({ handle: 'taken' }), 'HANDLE_TAKEN');
  assert.equal(snapshot().profile?.displayName, 'Ally', 'a refused save changes nothing');
});

test('the friend link: a new one replaces the old, turning it off clears it', async context => {
  const { fake, workspace, snapshot } = friendsFixture();
  context.after(() => workspace.close());
  await until(() => !!snapshot().profile, 'the profile');
  assert.equal(snapshot().profile?.friendCode?.code, 'a'.repeat(32));
  assert.equal(await workspace.rotateCode(), '');
  assert.equal(snapshot().profile?.friendCode?.code, 'b'.repeat(32)); assert.ok(snapshot().profile?.friendCode?.link.endsWith('b'.repeat(32)));
  assert.equal(await workspace.revokeCode(), '');
  assert.equal(snapshot().profile?.friendCode, null);
  fake.fail('rotateCode', 'RATE_LIMITED', 429);
  assert.equal(await workspace.rotateCode(), 'RATE_LIMITED');
  assert.deepEqual(fake.commands.map(command => command.action), ['rotateCode', 'revokeCode', 'rotateCode']);
});

// ── Unread and the badge (D13, D14) ──

test('the badge is the incoming requests plus the unread of the friends; an open chat counts as read and the card\'s value returns when it closes', async context => {
  const { workspace, snapshot } = friendsFixture(f => {
    f.cards = [card('bob', { lastSeq: 5, lastReadSeq: 2, lastMessage: pointer(5) }), card('carol', { lastSeq: 2, lastReadSeq: 2 }), card('dave', { state: 'incoming' }), card('erin', { state: 'outgoing' }), card('finn', { lastSeq: 9 })];
  });
  context.after(() => workspace.close());
  await ready(snapshot);
  assert.equal(snapshot().badge, 1 + 3 + 9);
  workspace.openChat('bob');
  assert.equal(snapshot().cards.find(value => value.uid === 'bob')?.lastReadSeq, 5, 'the chat on screen is read');
  assert.equal(snapshot().cards.find(value => value.uid === 'finn')?.lastReadSeq, undefined, 'others are the card\'s own');
  assert.equal(snapshot().badge, 1 + 9);
  workspace.show('profile');
  assert.equal(snapshot().badge, 1 + 3 + 9, 'leaving the chat gives the card\'s value back');
});

test('a message that arrives in the open chat leaves its unread at zero', async context => {
  const { fake, workspace, snapshot } = friendsFixture(f => { f.cards = [card('bob', { lastSeq: 1, lastReadSeq: 1 })]; });
  context.after(() => workspace.close());
  await ready(snapshot);
  workspace.openChat('bob');
  fake.emitCards([card('bob', { lastSeq: 4, lastReadSeq: 1, lastMessage: pointer(4) })]);
  assert.equal(snapshot().badge, 0);
  assert.equal(snapshot().cards[0]!.lastSeq, 4);
});

// ── Add friend (D6, D7, D18) ──

test('lookup answers the person or the error code, and sends nothing else', async context => {
  const { fake, workspace } = friendsFixture();
  context.after(() => workspace.close());
  fake.lookup({ uid: 'sam', handle: 'sam', relation: 'incoming' });
  assert.deepEqual(await workspace.lookup(handle), { result: { uid: 'sam', displayName: 'Sam', handle: 'sam', relation: 'incoming' } });
  assert.deepEqual(await workspace.lookup({ kind: 'email', email: 'sam@example.com' }), { result: { uid: 'sam', displayName: 'Sam', handle: 'sam', relation: 'incoming' } });
  assert.deepEqual(await workspace.lookup({ kind: 'team', teamId: 'team-aaaa', uid: 'sam' }), { result: { uid: 'sam', displayName: 'Sam', handle: 'sam', relation: 'incoming' } });
  fake.fail('lookup', 'NOT_FOUND', 404);
  assert.deepEqual(await workspace.lookup({ kind: 'code', code: 'c'.repeat(32) }), { error: 'NOT_FOUND' });
  assert.deepEqual(fake.commands, [
    { action: 'lookup', handle: 'sam' }, { action: 'lookup', email: 'sam@example.com' }, { action: 'lookup', teamId: 'team-aaaa', uid: 'sam' }, { action: 'lookup', code: 'c'.repeat(32) },
  ]);
});

test('a confirmation keeps its request id for a retry after an unknown outcome and mints a new one after any definitive answer (D18)', async context => {
  const { fake, workspace } = friendsFixture();
  context.after(() => workspace.close());
  const ids = () => fake.executed('request').map(command => (command as { requestId: string }).requestId);
  fake.fail('request', 'SERVICE_UNAVAILABLE', 503);
  assert.deepEqual(await workspace.confirm(handle, found(), ''), { error: 'SERVICE_UNAVAILABLE' });
  assert.deepEqual(await workspace.confirm(handle, found(), ''), { error: 'SERVICE_UNAVAILABLE' });
  assert.equal(ids()[0], ids()[1], 'the same confirmation is retried with the same id');
  fake.replies.delete('request');
  assert.deepEqual(await workspace.confirm(handle, found(), ''), { done: 'sent', name: 'Sam' });
  assert.equal(ids()[2], ids()[0], 'and the retry that got through still carries it');
  assert.deepEqual(await workspace.confirm(handle, found(), ''), { done: 'sent', name: 'Sam' });
  assert.notEqual(ids()[3], ids()[0], 'a confirmation after a success is a new one');

  fake.fail('request', 'RATE_LIMITED', 429);
  await workspace.confirm(handle, found(), '');
  fake.replies.delete('request');
  await workspace.confirm(handle, found(), '');
  assert.notEqual(ids()[5], ids()[4], 'a definitive answer ends the id');

  fake.fail('request', 'SERVICE_UNAVAILABLE', 503);
  await workspace.confirm(handle, found(), 'hello');
  await workspace.confirm(handle, found(), 'another note');
  await workspace.confirm({ kind: 'handle', handle: 'sam2' }, found({ uid: 'sam2' }), 'another note');
  assert.equal(new Set(ids().slice(6)).size, 3, 'a different note or person is a different confirmation');
  assert.match(ids()[0]!, /^[A-Za-z0-9_-]{8,64}$/u);
});

test('what a confirmation sends: a request with the trimmed note, an accept for an incoming request, a redeem for a link, a team request for a member', async context => {
  const { fake, workspace, snapshot } = friendsFixture();
  context.after(() => workspace.close());
  assert.deepEqual(await workspace.confirm(handle, found(), '  hi there  '), { done: 'sent', name: 'Sam' });
  assert.deepEqual(fake.commands.at(-1), { action: 'request', via: 'handle', handle: 'sam', requestId: (fake.commands.at(-1) as { requestId: string }).requestId, note: 'hi there' });
  await workspace.confirm({ kind: 'email', email: 'sam@example.com' }, found(), '   ');
  assert.ok(!('note' in fake.commands.at(-1)!), 'a blank note is no note');
  await workspace.confirm({ kind: 'team', teamId: 'team-aaaa', uid: 'sam' }, found(), 'from the team');
  assert.deepEqual({ ...fake.commands.at(-1) as object, requestId: '' }, { action: 'request', via: 'team', teamId: 'team-aaaa', uid: 'sam', requestId: '', note: 'from the team' });

  fake.commands.length = 0;
  assert.deepEqual(await workspace.confirm(handle, found({ relation: 'incoming' }), 'ignored'), { done: 'friends', name: 'Sam' });
  assert.deepEqual(fake.commands, [{ action: 'accept', otherUid: 'sam' }]);
  assert.equal(snapshot().page, 'chat'); assert.equal(snapshot().chat?.uid, 'sam'); assert.equal(snapshot().chat?.joined, true, 'the new friendship is greeted once');

  fake.commands.length = 0;
  const code: Proof = { kind: 'code', code: 'c'.repeat(32) };
  assert.deepEqual(await workspace.confirm(code, found({ relation: 'incoming' }), 'ignored'), { done: 'friends', name: 'Sam' });
  assert.equal(fake.commands.length, 1); assert.equal(fake.commands[0]!.action, 'redeem', 'a link always redeems, even over a pending request');
  assert.ok(!('note' in fake.commands[0]!), 'a link takes no note');
  assert.equal((fake.commands[0] as { code: string }).code, code.code);
});

test('a request that answers friends (a mutual request) opens the chat; one that answers outgoing only says it was sent', async context => {
  const { fake, workspace, snapshot } = friendsFixture();
  context.after(() => workspace.close());
  fake.replies.set('request', { uid: 'sam', state: 'friends', teamId: pairTeamId('sam') });
  assert.deepEqual(await workspace.confirm(handle, found(), ''), { done: 'friends', name: 'Sam' });
  assert.equal(snapshot().page, 'chat');
  workspace.show('add');
  fake.replies.set('request', { uid: 'sam', state: 'outgoing' });
  assert.deepEqual(await workspace.confirm(handle, found(), ''), { done: 'sent', name: 'Sam' });
  assert.equal(snapshot().page, 'add', 'the request does not leave the page');
});

// ── Requests, leaving, blocking (D4, D10, D12) ──

test('answering a request, leaving a friendship and unblocking are single actions that resolve their error code', async context => {
  const { fake, workspace, snapshot } = friendsFixture(f => { f.cards = [card('bob'), card('carol', { state: 'incoming' }), card('dave', { state: 'outgoing' })]; f.blocks = [block('erin', 3)]; });
  context.after(() => workspace.close());
  await ready(snapshot);
  assert.equal(await workspace.respond('carol', 'accept'), '');
  assert.equal(await workspace.respond('carol', 'decline'), '');
  assert.equal(await workspace.respond('dave', 'cancel'), '');
  fake.fail('accept', 'FRIEND_LIMIT');
  assert.equal(await workspace.respond('carol', 'accept'), 'FRIEND_LIMIT');
  assert.equal(await workspace.unblock('erin'), '');
  assert.deepEqual(fake.commands.map(command => [command.action, (command as { otherUid: string }).otherUid]), [['accept', 'carol'], ['decline', 'carol'], ['cancel', 'dave'], ['accept', 'carol'], ['unblock', 'erin']]);

  workspace.openChat('bob');
  fake.fail('remove', 'SERVICE_UNAVAILABLE', 503);
  assert.equal(await workspace.leave('bob', 'remove'), 'SERVICE_UNAVAILABLE');
  assert.equal(snapshot().page, 'chat', 'a failed removal leaves the chat');
  fake.replies.delete('remove');
  assert.equal(await workspace.leave('bob', 'remove'), '');
  assert.equal(snapshot().page, 'home'); assert.equal(snapshot().chat, undefined);
  workspace.openChat('bob');
  assert.equal(await workspace.leave('bob', 'block'), '');
  assert.equal(fake.commands.at(-1)!.action, 'block');
});

test('pages: one at a time, leaving the chat closes it, and the requests link asks the root to scroll', async context => {
  const { workspace, snapshot } = friendsFixture(f => { f.cards = [card('bob')]; });
  context.after(() => workspace.close());
  await ready(snapshot);
  assert.equal(snapshot().page, 'home');
  workspace.openChat('bob'); assert.equal(snapshot().page, 'chat');
  for (const page of ['add', 'profile', 'blocked', 'home'] as const) { workspace.show(page); assert.equal(snapshot().page, page); assert.equal(snapshot().chat, undefined, page); }
  const before = snapshot().requestsFocus;
  workspace.openChat('bob'); workspace.showRequests();
  assert.equal(snapshot().page, 'home'); assert.equal(snapshot().requestsFocus, before + 1);
});

// ── The chat (D9) ──

const chatTeam = (id: string) => pairTeamId(id);
function withChat(f: Parameters<typeof friendsFixture>[0] = () => {}) {
  return friendsFixture((fake, teams) => {
    fake.cards = [card('bob', { lastSeq: 2, lastReadSeq: 2, lastMessage: pointer(2) })];
    teams.conversations.set(chatTeam('bob'), [conversation('friend_chat', { kind: 'dm', participants: ['user:alice', 'user:bob'], lastSeq: 2, lastReadSeq: 2 })]);
    teams.threads.set('friend_chat', [message(1, { text: 'Hi' }), message(2, { text: 'Hello' })]);
    f(fake, teams);
  });
}

test('a friend\'s chat is a workspace pinned to the pair team of the card, which opens the one conversation', async context => {
  const { teams, workspace, snapshot } = withChat();
  context.after(() => workspace.close());
  workspace.openChat('bob');
  assert.equal(snapshot().chat?.workspace, undefined, 'the card has not arrived: nothing to open yet'); assert.equal(snapshot().chat?.ended, false);
  await until(() => !!snapshot().chat?.workspace, 'the pair workspace');
  const pair = snapshot().chat!.workspace!;
  assert.equal(snapshot().chat?.name, 'Bob');
  await until(() => pair.getSnapshot().conversation?.id === 'friend_chat' && pair.getSnapshot().messages.length === 2, 'the conversation');
  const view = pair.getSnapshot();
  assert.equal(view.team?.id, chatTeam('bob')); assert.equal(view.team?.generation, 1); assert.equal(view.selectedConversationId, 'friend_chat');
  assert.deepEqual(view.directory.map(value => value.uid), [uid, 'bob']);
  assert.equal(teams.listCalls, 0, 'a pair team is never listed'); assert.deepEqual(teams.detailCalls, []);
  assert.deepEqual(teams.commands.filter(command => !['read'].includes(command.action)).map(command => command.action), ['listSharedBots'], 'only the Bot cards of the friendship are asked for (D21), no tasks');
  assert.ok(teams.watches.every(watch => watch.scope.teamId === chatTeam('bob') && watch.scope.generation === 1));
});

test('a person who is not a friend, or no longer one, gets the unavailable page; a request does not make a chat', async context => {
  const { fake, teams, workspace, snapshot } = withChat(f => { f.cards.push(card('carol', { state: 'incoming' }), card('dave', { state: 'outgoing' })); });
  context.after(() => workspace.close());
  await ready(snapshot);
  for (const other of ['zed', 'carol', 'dave']) {
    workspace.openChat(other);
    assert.equal(snapshot().chat?.ended, true, other); assert.equal(snapshot().chat?.workspace, undefined, other);
  }
  workspace.openChat('bob');
  await until(() => !!snapshot().chat?.workspace, 'the chat of a friend');
  const before = teams.openWatches;
  assert.ok(before > 0);
  fake.emitCards([card('carol', { state: 'incoming' })]);
  assert.equal(snapshot().chat?.ended, true, 'the friendship ended while the chat was open');
  assert.equal(snapshot().chat?.name, 'Bob', 'the header keeps the last known name'); assert.equal(snapshot().chat?.workspace, undefined);
  assert.equal(teams.openWatches, 0, 'and its listeners are released');
});

test('a new friendship waits for its card (the chat is not unavailable meanwhile) and greets once', async context => {
  const { fake, workspace, snapshot } = withChat();
  context.after(() => workspace.close());
  await ready(snapshot);
  workspace.openChat('newbie', true);
  assert.equal(snapshot().chat?.ended, false); assert.equal(snapshot().chat?.joined, true);
  fake.emitCards([card('bob'), card('newbie', { displayName: 'Newbie' })]);
  await until(() => !!snapshot().chat?.workspace, 'the new friend\'s chat');
  assert.equal(snapshot().chat?.name, 'Newbie'); assert.equal(snapshot().chat?.ended, false);
  workspace.openChat('newbie');
  assert.equal(snapshot().chat?.joined, true, 'reopening the same chat is not a new chat');
});

test('a new generation on the card replaces the pair workspace; the old one is released', async context => {
  const { fake, teams, workspace, snapshot } = withChat();
  context.after(() => workspace.close());
  workspace.openChat('bob');
  await until(() => !!snapshot().chat?.workspace, 'the first workspace');
  const first = snapshot().chat!.workspace!;
  await until(() => first.getSnapshot().conversation?.id === 'friend_chat', 'the conversation');
  fake.emitCards([card('bob', { generation: 2, lastSeq: 2, lastReadSeq: 2 })]);
  const second = snapshot().chat!.workspace!;
  assert.notEqual(second, first); assert.equal(second.getSnapshot().team?.generation, 2);
  await settle();
  assert.ok(teams.watches.filter(watch => watch.open).every(watch => watch.scope.generation === 2), 'nothing listens in the old generation');
  fake.emitCards([card('bob', { generation: 2, lastSeq: 3, lastReadSeq: 2, lastMessage: pointer(3) })]);
  assert.equal(snapshot().chat!.workspace, second, 'a new message on the card keeps the workspace');
});

test('leaving the chat or closing the workspace releases every listener of the pair team (D19)', async context => {
  const { fake, teams, workspace, snapshot } = withChat();
  context.after(() => workspace.close());
  workspace.openChat('bob');
  await until(() => teams.openWatches > 0 && !!snapshot().chat?.workspace, 'the chat\'s listeners');
  workspace.show('home');
  assert.equal(teams.openWatches, 0); assert.equal(snapshot().chat, undefined);
  workspace.openChat('bob');
  await until(() => teams.openWatches > 0, 'the chat again');
  workspace.close();
  assert.equal(teams.openWatches, 0); assert.deepEqual(fake.open, { cards: 0, blocks: 0, offers: 0 });
});

test('names are refreshed once from the friend\'s profile and kept for the session, in the list and in the chat (D10); the card is never written', async context => {
  const { fake, workspace, snapshot } = withChat(f => { f.names.set('bob', { displayName: 'Robert', handle: 'rob' }); f.names.set('ghost', null); });
  context.after(() => workspace.close());
  workspace.openChat('bob');
  await until(() => !!snapshot().chat?.workspace, 'the chat');
  const pair = snapshot().chat!.workspace!;
  await workspace.refreshFriend('bob');
  assert.deepEqual(fake.readProfiles, ['bob']);
  assert.equal(snapshot().cards[0]!.displayName, 'Robert'); assert.equal(snapshot().cards[0]!.handle, 'rob');
  assert.equal(snapshot().chat?.name, 'Robert'); assert.equal(snapshot().chat?.handle, 'rob');
  assert.equal(snapshot().chat!.workspace, pair, 'the same workspace learns the new name');
  assert.equal(pair.getSnapshot().directory.find(value => value.uid === 'bob')?.displayName, 'Robert');
  fake.emitCards([card('bob', { displayName: 'Bob', lastSeq: 3, lastReadSeq: 3 })]);
  assert.equal(snapshot().cards[0]!.displayName, 'Robert', 'a later card does not undo it');
  await workspace.refreshFriend('ghost'); await workspace.refreshFriend('bob');
  assert.equal(snapshot().cards.length, 1);
  assert.deepEqual(fake.commands, [], 'nothing was written');
});

test('a profile that cannot be read, or says what the card says, changes nothing', async context => {
  const { fake, workspace, snapshot } = friendsFixture(f => { f.cards = [card('bob')]; f.names.set('bob', { displayName: 'Bob' }); });
  context.after(() => workspace.close());
  await ready(snapshot);
  const before = snapshot().cards;
  await workspace.refreshFriend('bob'); await workspace.refreshFriend('nobody');
  assert.equal(snapshot().cards, before, 'no new snapshot of cards');
  fake.api.readProfile = async () => { throw new Error('permission-denied'); };
  await workspace.refreshFriend('bob');
  assert.equal(snapshot().cards[0]!.displayName, 'Bob');
});

// ── Team offers (F4, D26 to D30) ──

const T0 = Date.UTC(2026, 10, 9, 12), HOUR = 3_600_000, DAY = 24 * HOUR;
const flush = async () => { for (let turn = 0; turn < 12; turn++) await Promise.resolve(); };

test('the offers listener feeds the snapshot, live before expired and the newest first, and the badge adds the live ones (D27, D28)', async context => {
  const { fake, workspace, snapshot } = friendsFixture(f => {
    f.cards = [card('carol', { state: 'incoming' }), card('bob', { lastSeq: 4, lastReadSeq: 3, lastMessage: pointer(4) })];
    f.offers = [offer('t-old', T0 - DAY), offer('t-soon', T0 + HOUR), offer('t-late', T0 + 5 * DAY), offer('t-gone', T0 - 3 * DAY)];
  }, { now: () => T0 });
  context.after(() => workspace.close());
  await ready(snapshot);
  assert.deepEqual(snapshot().offers.map(value => value.teamId), ['t-late', 't-soon', 't-old', 't-gone']);
  assert.equal(snapshot().now, T0);
  assert.equal(snapshot().badge, 4, 'a request, an unread message and two live offers; the expired ones add nothing');

  fake.emitOffers([offer('t-late', T0 + 5 * DAY)]);
  assert.equal(snapshot().offers.length, 1); assert.equal(snapshot().badge, 3);
  fake.emitOffers([]);
  assert.deepEqual(snapshot().offers, []); assert.equal(snapshot().badge, 2);
});

test('a failed offers listener shows and counts nothing, and retry restarts it with the others (D27)', async context => {
  const { fake, workspace, snapshot } = friendsFixture(f => { f.cards = [card('bob')]; f.offers = [offer('t-1', T0 + DAY)]; }, { now: () => T0 });
  context.after(() => workspace.close());
  await ready(snapshot);
  assert.equal(snapshot().offers.length, 1); assert.equal(snapshot().badge, 1);
  fake.failOffers('SERVICE_UNAVAILABLE');
  assert.deepEqual(snapshot().offers, []); assert.equal(snapshot().badge, 0); assert.equal(snapshot().loadError, '', 'the friends list is not an error because of it');
  workspace.retry();
  assert.deepEqual(fake.open, { cards: 1, blocks: 1, offers: 1 }, 'the failed watch was replaced, not stacked');
  await until(() => snapshot().offers.length === 1, 'the offers again');
  assert.equal(snapshot().badge, 1);
  const [stale] = fake.offerWatches;
  stale!.failed('SERVICE_UNAVAILABLE'); stale!.next([]);
  assert.equal(snapshot().offers.length, 1, 'a listener that was replaced cannot report into the new one');
});

test('the badge follows the clock: one timer at the earliest live expiry, and recount when the view is shown (D28)', async context => {
  context.mock.timers.enable({ apis: ['setTimeout'] });
  let now = T0;
  const { fake, workspace, snapshot } = friendsFixture(f => { f.offers = [offer('a', T0 + 2 * HOUR), offer('b', T0 + 5 * HOUR)]; }, { now: () => now });
  context.after(() => workspace.close());
  await flush();
  assert.equal(snapshot().badge, 2);
  now = T0 + 2 * HOUR; context.mock.timers.tick(2 * HOUR);
  assert.equal(snapshot().badge, 1, 'the first offer expired at its own expiry');
  assert.deepEqual(snapshot().offers.map(value => value.teamId), ['b', 'a']); assert.equal(snapshot().now, now);
  const woken = snapshot().now;
  now = T0 + 4 * HOUR; context.mock.timers.tick(HOUR);
  assert.equal(snapshot().now, woken, 'nothing wakes the workspace before the next expiry');
  now = T0 + 5 * HOUR; context.mock.timers.tick(2 * HOUR);
  assert.equal(snapshot().badge, 0); assert.equal(snapshot().now, now);

  fake.emitOffers([offer('c', now + HOUR)]);
  assert.equal(snapshot().badge, 1);
  now += 30 * 60_000; workspace.recount();
  assert.equal(snapshot().now, now, 'showing the view works the state out again');
  now += 31 * 60_000; workspace.recount();
  assert.equal(snapshot().badge, 0, 'without waiting for the timer');
  workspace.close();
  now += DAY; context.mock.timers.tick(DAY);
  assert.equal(snapshot().now, T0 + 5 * HOUR + 61 * 60_000, 'a closed workspace has no timer');
});

test('an expiry too far ahead for one timer is waited for in steps', async context => {
  context.mock.timers.enable({ apis: ['setTimeout'] });
  let now = T0;
  const { workspace, snapshot } = friendsFixture(f => { f.offers = [offer('far', T0 + 40 * DAY)]; }, { now: () => now });
  context.after(() => workspace.close());
  await flush();
  assert.equal(snapshot().badge, 1);
  now = T0 + 24 * DAY; context.mock.timers.tick(24 * DAY);
  assert.equal(snapshot().badge, 1, 'woken early, still live');
  now = T0 + 40 * DAY; context.mock.timers.tick(16 * DAY);
  assert.equal(snapshot().badge, 0);
});

test('offerTeam, acceptOffer and declineOffer send one command each and resolve an error code; the request id lives until the outcome is known (D30)', async context => {
  const { fake, workspace } = friendsFixture();
  context.after(() => workspace.close());
  const ids = (action: 'offerTeam' | 'acceptOffer') => fake.executed(action).map(command => (command as { requestId: string }).requestId);

  assert.equal(await workspace.offerTeam('team-a', 'bob'), '');
  assert.deepEqual(fake.executed('offerTeam'), [{ action: 'offerTeam', teamId: 'team-a', friendUid: 'bob', requestId: ids('offerTeam')[0] }]);
  assert.match(ids('offerTeam')[0]!, /^[A-Za-z0-9_-]{8,64}$/u);

  fake.fail('offerTeam', 'SERVICE_UNAVAILABLE', 503);
  assert.equal(await workspace.offerTeam('team-a', 'carol'), 'SERVICE_UNAVAILABLE');
  assert.equal(await workspace.offerTeam('team-a', 'carol'), 'SERVICE_UNAVAILABLE');
  assert.equal(ids('offerTeam')[1], ids('offerTeam')[2], 'an unknown outcome is retried with the same id');
  assert.notEqual(ids('offerTeam')[1], ids('offerTeam')[0], 'for that friend and team only');
  fake.replies.delete('offerTeam');
  assert.equal(await workspace.offerTeam('team-a', 'carol'), '');
  assert.equal(ids('offerTeam')[3], ids('offerTeam')[1], 'the retry that got through still carries it');
  assert.equal(await workspace.offerTeam('team-a', 'carol'), '');
  assert.notEqual(ids('offerTeam')[4], ids('offerTeam')[1], 'an offer after a success is a new one');

  fake.fail('offerTeam', 'OFFER_LIMIT');
  assert.equal(await workspace.offerTeam('team-a', 'dave'), 'OFFER_LIMIT');
  fake.replies.delete('offerTeam');
  await workspace.offerTeam('team-a', 'dave');
  assert.notEqual(ids('offerTeam')[6], ids('offerTeam')[5], 'a definitive answer ends the id');

  fake.fail('acceptOffer', 'SERVICE_UNAVAILABLE', 503);
  assert.equal(await workspace.acceptOffer('team-b'), 'SERVICE_UNAVAILABLE');
  fake.replies.delete('acceptOffer');
  assert.equal(await workspace.acceptOffer('team-b'), '');
  assert.deepEqual(fake.executed('acceptOffer'), ids('acceptOffer').map(requestId => ({ action: 'acceptOffer', offerId: 'team-b', requestId })));
  assert.equal(ids('acceptOffer')[0], ids('acceptOffer')[1]);
  fake.replies.set('acceptOffer', { teamId: 'team-b', generation: 1, alreadyMember: true });
  assert.equal(await workspace.acceptOffer('team-b'), '', 'a replay that finds the person a member is a success');
  fake.fail('acceptOffer', 'TEAM_LIMIT');
  assert.equal(await workspace.acceptOffer('team-c'), 'TEAM_LIMIT');

  assert.equal(await workspace.declineOffer('team-b'), '');
  assert.deepEqual(fake.executed('declineOffer'), [{ action: 'declineOffer', offerId: 'team-b' }], 'a decline has no request id');
  fake.fail('declineOffer', 'OFFER_UNAVAILABLE');
  assert.equal(await workspace.declineOffer('team-b'), 'OFFER_UNAVAILABLE');

  workspace.close();
  assert.equal(await workspace.offerTeam('team-a', 'bob'), 'INVALID_SESSION'); assert.equal(await workspace.acceptOffer('team-b'), 'INVALID_SESSION');
  assert.equal(await workspace.declineOffer('team-b'), 'INVALID_SESSION');
});

test('offers in flight side by side keep their own request ids', async context => {
  const { fake, workspace } = friendsFixture();
  context.after(() => workspace.close());
  const release: (() => void)[] = [];
  fake.replies.set('offerTeam', () => new Promise(resolve => { release.push(() => resolve({ offerId: 'x', expiresAt: '2026-11-16T00:00:00.000Z' })); }));
  const first = workspace.offerTeam('team-a', 'bob'), second = workspace.offerTeam('team-a', 'carol');
  await until(() => release.length === 2, 'both requests');
  const [one, two] = fake.executed('offerTeam') as { requestId: string; friendUid: string }[];
  assert.notEqual(one!.requestId, two!.requestId);
  release.forEach(resolve => resolve());
  assert.deepEqual(await Promise.all([first, second]), ['', '']);
});
