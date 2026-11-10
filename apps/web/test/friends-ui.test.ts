import assert from 'node:assert/strict';
import { after, type TestContext } from 'node:test';
import { createElement } from 'react';
import { friendsErrorText } from '../../../packages/teams/errors';
import type { TeamMember } from '../../../packages/teams';
import type { FriendsSnapshot, FriendsWorkspace } from '../../../packages/teams/friends-workspace';
import { TeamsWorkspace } from '../../../packages/teams/workspace';
import { block, card, friendsFixture, pairTeamId, pointer, profile, uid, until, type FakeFriends } from './support/fakeFriends';
import { FakeTeams, conversation, memoryStorage, message, team } from './support/fakeTeams';
import { loadTeamsModule } from './support/loadTeamsModule';
import { classes, mount, scenario, textOf, withDom } from './support/workspacePage';

const { FriendsSidebar } = loadTeamsModule<typeof import('../../../packages/teams/FriendsSidebar')>('FriendsSidebar');
const { AddFriendView } = loadTeamsModule<typeof import('../../../packages/teams/AddFriendView')>('AddFriendView');
const { FriendProfileSettings } = loadTeamsModule<typeof import('../../../packages/teams/FriendProfileSettings')>('FriendProfileSettings');
const { BlockedList } = loadTeamsModule<typeof import('../../../packages/teams/BlockedList')>('BlockedList');
const { FriendsMain } = loadTeamsModule<typeof import('../../../packages/teams/FriendsMain')>('FriendsMain');
const { FriendLinkDialog, MemberFriendDialog } = loadTeamsModule<typeof import('../../../packages/teams/friend-dialogs')>('friend-dialogs');
const { TeamDetailsDrawer } = loadTeamsModule<typeof import('../../../packages/teams/TeamDetailsDrawer')>('TeamDetailsDrawer');

// The fake browser stays for the whole file, so a view that is unmounted after its test still finds its document.
withDom({ after } as unknown as TestContext);

const NOW = new Date(2026, 10, 3, 12).getTime();
const view = (workspace: FriendsWorkspace, component: unknown, more: Record<string, unknown> = {}) => (snapshot: FriendsSnapshot) =>
  createElement(component as never, { workspace, snapshot, zh: false, locale: 'en-US', now: NOW, ...more } as never);
const ready = (snapshot: () => FriendsSnapshot) => until(() => !snapshot().loading && !snapshot().blocksLoading && !!snapshot().profile, 'the lists and the profile');
const at = (day: number, hour = 10) => new Date(2026, 10, day, hour).toISOString();
const dialogs = (page: { find(match: (node: any) => boolean): any[] }) => page.find(node => node.type === 'dialog');

// ── The root (D3, D4, D5) ──

scenario('the root: requests first, then friends by last activity with their preview and unread badge, and the filter', async context => {
  const { workspace, snapshot } = friendsFixture(f => {
    f.cards = [
      card('bob', { lastSeq: 5, lastReadSeq: 3, lastMessage: pointer(5, { at: at(2), preview: 'See you at 3' }) }),
      card('carol', { lastSeq: 7, lastReadSeq: 7, lastMessage: pointer(7, { at: at(3, 9), preview: 'Thanks!', author: { kind: 'user', uid } }) }),
      card('dave'), card('erin', { state: 'incoming', note: 'Hi, it is Erin', handle: 'erin_w' }), card('finn', { state: 'outgoing' }),
    ];
  });
  context.after(() => workspace.close());
  await ready(snapshot);
  const page = await mount(workspace, view(workspace, FriendsSidebar)); context.after(() => page.unmount());
  const text = page.text();
  for (const expected of ['Friend requests', 'Received', 'Erin', '@erin_w', 'Hi, it is Erin', 'Sent', 'Finn', 'Request sent']) assert.ok(text.includes(expected), expected);
  const rows = page.byClass('tw-row');
  assert.deepEqual(rows.map(row => textOf(row.findAll((node: any) => classes(node).includes('tw-row-title'))[0])), ['Carol', 'Bob', 'Dave'], 'the newest message first, the friend with none last');
  const preview = (row: any) => textOf(row.findAll((node: any) => classes(node).includes('tw-row-preview'))[0]);
  assert.deepEqual(rows.map(preview), ['You: Thanks!', 'See you at 3', 'No messages yet']);
  assert.deepEqual(rows.map(row => row.props['data-unread']), [false, true, false]);
  const badges = page.byClass('tw-badge'); assert.equal(badges.length, 1); assert.equal(textOf(badges[0]), '2'); assert.equal(badges[0].props['aria-label'], '2 unread');
  assert.ok(text.includes('Today') || /\d/.test(text), 'a time sits on each row with a message');

  const filter = page.oneByLabel('Search friends');
  await page.type(filter, 'bo');
  assert.deepEqual(page.byClass('tw-row').length, 1); assert.ok(page.text().includes('Friend requests'), 'requests are not filtered');
  await page.type(filter, '@ZZZ');
  assert.equal(page.byClass('tw-row').length, 0); assert.ok(page.text().includes('No matching friends'));
  await page.type(filter, '');
  assert.equal(page.byClass('tw-row').length, 3);
});

scenario('a friend opens its chat; the open friend is marked and its unread badge goes', async context => {
  const { workspace, snapshot } = friendsFixture(f => { f.cards = [card('bob', { lastSeq: 5, lastReadSeq: 3, lastMessage: pointer(5) }), card('carol')]; });
  context.after(() => workspace.close());
  await ready(snapshot);
  const page = await mount(workspace, view(workspace, FriendsSidebar)); context.after(() => page.unmount());
  const bob = page.byClass('tw-row-main').find(node => textOf(node).includes('Bob'))!;
  await page.click(bob);
  assert.equal(snapshot().page, 'chat'); assert.equal(snapshot().chat?.uid, 'bob');
  const selected = page.byClass('tw-row-main').find(node => textOf(node).includes('Bob'))!;
  assert.equal(selected.props['aria-current'], 'page'); assert.equal(page.byClass('tw-badge').length, 0, 'the chat on screen is read');
});

scenario('requests: Accept, Decline and Cancel act on their row only, a failure is told on the row, and 20 rows show at a time', async context => {
  const many = Array.from({ length: 25 }, (_, index) => card(`u${String(index).padStart(2, '0')}`, { state: 'incoming', since: 1000 + index }));
  const { fake, workspace, snapshot } = friendsFixture(f => { f.cards = [card('sent1', { state: 'outgoing' }), ...many]; });
  context.after(() => workspace.close());
  await ready(snapshot);
  const page = await mount(workspace, view(workspace, FriendsSidebar)); context.after(() => page.unmount());
  const received = () => page.oneByLabel('Received');
  assert.equal(received().findAll((node: any) => node.type === 'li').length, 20, 'at most 20 rows');
  assert.ok(page.has('Show all (25)'));
  await page.click(page.button('Show all (25)'));
  assert.equal(received().findAll((node: any) => node.type === 'li').length, 25); assert.ok(!page.has('Show all (25)'));

  const row = (name: string) => received().findAll((node: any) => node.type === 'li').find((node: any) => textOf(node).includes(name))!;
  fake.fail('accept', 'FRIEND_LIMIT');
  await page.click(row('U24').findAll((node: any) => node.type === 'button').find((node: any) => textOf(node) === 'Accept'));
  assert.ok(textOf(row('U24')).includes(friendsErrorText('FRIEND_LIMIT', false)), 'the error is under the row that failed');
  assert.ok(!textOf(row('U23')).includes(friendsErrorText('FRIEND_LIMIT', false)));
  await page.click(row('U23').findAll((node: any) => node.type === 'button').find((node: any) => textOf(node) === 'Decline'));
  await page.click(page.oneByLabel('Sent').findAll((node: any) => node.type === 'button').find((node: any) => textOf(node) === 'Cancel request'));
  assert.deepEqual(fake.commands.map(command => [command.action, (command as { otherUid: string }).otherUid]), [['accept', 'u24'], ['decline', 'u23'], ['cancel', 'sent1']]);
});

scenario('accepting a request says so once and the list follows the listener', async context => {
  const { fake, workspace, snapshot } = friendsFixture(f => { f.cards = [card('erin', { state: 'incoming' })]; });
  context.after(() => workspace.close());
  await ready(snapshot);
  const page = await mount(workspace, view(workspace, FriendsSidebar)); context.after(() => page.unmount());
  await page.click(page.button('Accept'));
  assert.ok(page.text().includes('You and Erin are now friends'));
  fake.emitCards([card('erin', { lastSeq: 0 })]);
  await page.during(() => undefined);
  assert.ok(!page.text().includes('Friend requests')); assert.equal(page.byClass('tw-row').length, 1);
});

scenario('loading, a failed load with Retry, and no friends yet', async context => {
  const { fake, workspace, snapshot } = friendsFixture(f => { f.quiet = true; });
  context.after(() => workspace.close());
  const page = await mount(workspace, view(workspace, FriendsSidebar)); context.after(() => page.unmount());
  assert.ok(page.text().includes('Loading friends…'));
  fake.failCards('SERVICE_UNAVAILABLE'); await page.during(() => undefined);
  assert.ok(page.text().includes("Couldn't load your friends. Check your connection and try again.")); assert.equal(page.byClass('tw-row').length, 0);
  fake.quiet = false;
  await page.click(page.button('Retry'));
  await until(() => !snapshot().loading, 'the retry'); await page.during(() => undefined);
  assert.ok(page.text().includes('No friends yet')); assert.ok(page.text().includes('Add someone by @username, email or friend link to chat one-to-one.'));
  const empty = page.byClass('tw-empty-actions')[0]!;
  await page.click(empty.findAll((node: any) => node.type === 'button')[0]);
  assert.equal(snapshot().page, 'add');
});

scenario('the header: Add friend, a menu with exactly Profile settings and Blocked people, and the name banner for "Member"', async context => {
  const { fake, workspace, snapshot } = friendsFixture(f => { f.cards = [card('bob')]; f.profile = profile({ displayName: 'Member' }); });
  context.after(() => workspace.close());
  await ready(snapshot);
  const page = await mount(workspace, view(workspace, FriendsSidebar)); context.after(() => page.unmount());
  assert.ok(page.text().includes('Set your name so friends recognize you.'));
  await page.click(page.button('Set name')); assert.equal(snapshot().page, 'profile');
  await page.click(page.button('Add friend')); assert.equal(snapshot().page, 'add');
  await page.click(page.oneByLabel('More'));
  const items = page.find(node => node.props.role === 'menuitem');
  assert.deepEqual(items.map(textOf), ['Profile settings', 'Blocked people']);
  await page.click(items[1]!); assert.equal(snapshot().page, 'blocked');
  fake.profile = profile({ displayName: 'Alice' });
  await workspace.refreshProfile(); await page.during(() => undefined);
  assert.ok(!page.text().includes('Set your name so friends recognize you.'));
});

// ── Add friend (D6, D7) ──

const addPage = async (context: TestContext, setup: Parameters<typeof friendsFixture>[0] = () => {}) => {
  const fixture = friendsFixture(setup);
  context.after(() => fixture.workspace.close());
  await ready(fixture.snapshot);
  const page = await mount(fixture.workspace, view(fixture.workspace, AddFriendView)); context.after(() => page.unmount());
  return { ...fixture, page, field: () => page.find(node => node.type === 'input' && node.props['aria-label'] === 'Find someone')[0] };
};

scenario('Add friend: one field accepts a username, an email or a friend link, and anything else cannot be looked up', async context => {
  const { fake, page, field } = await addPage(context);
  fake.lookup({ uid: 'sam', handle: 'sam' });
  assert.equal(page.button('Look up').props.disabled, true, 'nothing to look up');
  await page.type(field(), 'not a person!');
  assert.equal(page.button('Look up').props.disabled, true); assert.ok(page.text().includes('Enter an exact @username, an email address or a friend link.'));
  await page.type(field(), '@sam');
  assert.equal(page.button('Look up').props.disabled, false);
  await page.submit(page.find(node => node.type === 'form')[0]);
  assert.deepEqual(fake.commands, [{ action: 'lookup', handle: 'sam' }]);
  await page.type(field(), '  Sam@Example.com ');
  await page.submit(page.find(node => node.type === 'form')[0]);
  const code = 'aB0_aB0_aB0_aB0_aB0_aB0_aB0_aB0-';
  await page.type(field(), `https://lingxios.net/open/friend/add?code=${code}`);
  await page.submit(page.find(node => node.type === 'form')[0]);
  assert.deepEqual(fake.commands.slice(1), [{ action: 'lookup', email: 'sam@example.com' }, { action: 'lookup', code }]);
});

scenario('the result card offers the one action of the relation: add with a note, accept, request sent, or open the chat', async context => {
  const { fake, workspace, snapshot, page, field } = await addPage(context);
  const search = async (query: string) => { await page.type(field(), query); await page.submit(page.find(node => node.type === 'form')[0]); };
  const card_ = () => page.oneByLabel('Sam');

  fake.lookup({ uid: 'sam', handle: 'sam', relation: 'none' });
  await search('@sam');
  assert.ok(textOf(card_()).includes('@sam'));
  assert.equal(page.find(node => node.type === 'textarea').length, 1, 'a note can ride a request');
  await page.type(page.find(node => node.type === 'textarea')[0], '  Met at the meetup ');
  await page.click(page.button('Add friend'));
  const request = fake.executed('request')[0] as { via: string; handle: string; note: string; requestId: string };
  assert.deepEqual([request.via, request.handle, request.note], ['handle', 'sam', 'Met at the meetup']); assert.ok(request.requestId);
  assert.ok(page.text().includes('Friend request sent to Sam')); assert.equal(page.find(node => node.props.role === 'group').length, 0, 'the card is done');
  assert.equal(page.find(node => node.type === 'input' && node.props['aria-label'] === 'Find someone')[0].props.value, '', 'and the field is clear');

  fake.lookup({ uid: 'sam', relation: 'outgoing' });
  await search('@sam');
  const sent = page.button('Request sent'); assert.equal(sent.props.disabled, true);

  fake.lookup({ uid: 'sam', relation: 'incoming' });
  await search('@sam');
  await page.click(page.button('Accept'));
  assert.equal(fake.commands.at(-1)!.action, 'accept'); assert.equal(snapshot().page, 'chat'); assert.equal(snapshot().chat?.uid, 'sam', 'accepting opens the chat');

  workspace.show('add'); await page.during(() => undefined);
  fake.lookup({ uid: 'sam', relation: 'friends' });
  await search('@sam');
  assert.ok(page.text().includes('Already friends'));
  await page.click(page.button('Open chat'));
  assert.equal(snapshot().page, 'chat');
});

scenario('a friend link adds without a note; a lookup that finds nothing says so and editing clears it', async context => {
  const { fake, page, field } = await addPage(context);
  const code = 'aB0_aB0_aB0_aB0_aB0_aB0_aB0_aB0-';
  fake.lookup({ uid: 'sam' });
  await page.type(field(), code); await page.submit(page.find(node => node.type === 'form')[0]);
  assert.equal(page.find(node => node.type === 'textarea').length, 0, 'a link takes no note');
  await page.click(page.button('Add friend'));
  const redeem = fake.executed('redeem')[0] as { code: string; requestId: string };
  assert.equal(redeem.code, code); assert.ok(redeem.requestId); assert.ok(!('note' in redeem));

  fake.fail('lookup', 'NOT_FOUND', 404);
  await page.type(field(), '@nobody'); await page.submit(page.find(node => node.type === 'form')[0]);
  assert.ok(page.text().includes(friendsErrorText('NOT_FOUND', false)));
  await page.type(field(), '@nobody2');
  assert.ok(!page.text().includes(friendsErrorText('NOT_FOUND', false)), 'a new query starts clean');
});

scenario('a failed confirmation is told on the card, which stays, and retrying an unknown outcome reuses the request id', async context => {
  const { fake, page, field } = await addPage(context);
  fake.lookup({ uid: 'sam' });
  await page.type(field(), '@sam'); await page.submit(page.find(node => node.type === 'form')[0]);
  fake.fail('request', 'SERVICE_UNAVAILABLE', 503);
  await page.click(page.button('Add friend'));
  assert.ok(textOf(page.oneByLabel('Sam')).includes(friendsErrorText('SERVICE_UNAVAILABLE', false)));
  await page.click(page.button('Add friend'));
  const [first, second] = fake.executed('request') as { requestId: string }[];
  assert.equal(first!.requestId, second!.requestId);
  fake.fail('request', 'REQUEST_LIMIT');
  await page.click(page.button('Add friend'));
  assert.ok(textOf(page.oneByLabel('Sam')).includes(friendsErrorText('REQUEST_LIMIT', false)));
});

scenario('my friend link: shown with copy, reset and turn off behind confirmations, and created again when off', async context => {
  const { fake, snapshot, page } = await addPage(context);
  const link = profile().friendCode!.link;
  assert.ok(page.find(node => node.props.title === link).length === 1, 'the whole link is in the title of the shortened text');
  assert.ok(page.text().includes('My friend link')); assert.ok(page.text().includes('without your approval'));
  assert.ok(!page.has('Share link'), 'no share sheet in a test browser');
  await page.click(page.button('Reset link'));
  assert.equal(dialogs(page).length, 1); assert.ok(textOf(dialogs(page)[0]).includes('The old link stops working immediately.'));
  assert.equal(fake.executed('rotateCode').length, 0, 'asking is not doing');
  await page.click(dialogs(page)[0].findAll((node: any) => node.type === 'button').find((node: any) => textOf(node) === 'Reset link'));
  assert.equal(fake.executed('rotateCode').length, 1); assert.equal(dialogs(page).length, 0);
  assert.equal(snapshot().profile?.friendCode?.code, 'b'.repeat(32));
  await page.click(page.button('Turn off link'));
  await page.click(dialogs(page)[0].findAll((node: any) => node.type === 'button').find((node: any) => textOf(node) === 'Turn off link'));
  assert.equal(fake.executed('revokeCode').length, 1); assert.ok(page.text().includes('Your friend link is off.'));
  await page.click(page.button('Create link'));
  assert.equal(fake.executed('rotateCode').length, 2); assert.equal(snapshot().profile?.friendCode?.code, 'b'.repeat(32));
});

scenario('opening Add friend reads the profile again, and a profile that cannot be read can be retried', async context => {
  const { fake, workspace, snapshot } = friendsFixture(f => { f.profileError = 'SERVICE_UNAVAILABLE'; });
  context.after(() => workspace.close());
  await until(() => snapshot().profileError !== '', 'the profile error');
  const reads = fake.profileCalls;
  const page = await mount(workspace, view(workspace, AddFriendView)); context.after(() => page.unmount());
  await until(() => fake.profileCalls > reads, 'the page to read the profile'); await page.during(() => undefined);
  assert.ok(page.text().includes(friendsErrorText('SERVICE_UNAVAILABLE', false)));
  fake.profileError = undefined;
  await page.click(page.button('Retry'));
  assert.ok(page.text().includes('My friend link')); assert.ok(page.find(node => node.props.title === profile().friendCode!.link).length === 1);
});

// ── Profile settings (D11) and Blocked people (D12) ──

scenario('profile settings send only what changed, keep an unset username as it is, and say when the username is taken', async context => {
  const { fake, workspace, snapshot } = friendsFixture(f => { f.profile = profile({ handle: undefined, displayName: 'Alice' }); });
  context.after(() => workspace.close());
  await ready(snapshot);
  const page = await mount(workspace, view(workspace, FriendProfileSettings)); context.after(() => page.unmount());
  const save = () => page.find(node => node.type === 'button' && node.props.type === 'submit')[0];
  const inputs = () => page.find(node => node.type === 'input');
  assert.equal(save().props.disabled, true, 'nothing changed');
  assert.equal(inputs()[0].props.value, 'Alice'); assert.equal(inputs()[1].props.value, '');
  assert.equal(inputs().filter(node => node.props.type === 'checkbox').map(node => node.props.checked).join(), 'true,false');

  await page.type(inputs()[0], '   '); assert.equal(save().props.disabled, true, 'a name cannot be blank');
  await page.type(inputs()[0], 'x'.repeat(81)); assert.equal(save().props.disabled, true, 'or longer than 80');
  await page.type(inputs()[0], '  Ally  ');
  await page.type(inputs()[1], 'ab'); assert.equal(save().props.disabled, true, 'a username needs 3 to 24 letters, digits or underscores');
  await page.type(inputs()[1], 'ally_w');
  await page.check(inputs().filter(node => node.props.type === 'checkbox')[1], true);
  fake.fail('profile', 'HANDLE_TAKEN');
  await page.submit();
  assert.deepEqual(fake.commands, [{ action: 'profile', displayName: 'Ally', handle: 'ally_w', findByEmail: true }]);
  assert.ok(page.find(node => node.props.role === 'alert' && textOf(node) === friendsErrorText('HANDLE_TAKEN', false)).length === 1, 'told next to the username');
  fake.replies.delete('profile'); fake.profile = profile({ displayName: 'Ally', handle: 'ally_w', findByEmail: true });
  await page.submit();
  assert.equal(fake.commands.length, 2); assert.equal(snapshot().profile?.displayName, 'Ally');
  assert.equal(page.find(node => node.props.role === 'alert').length, 0);
});

scenario('blocked people: newest first with Unblock, the list follows the listener, and nothing blocked says so', async context => {
  const { fake, workspace, snapshot } = friendsFixture(f => { f.blocks = [block('old', 1000, { displayName: 'Old One' }), block('new', 9000, { handle: 'newbie' })]; });
  context.after(() => workspace.close());
  await ready(snapshot);
  const page = await mount(workspace, view(workspace, BlockedList)); context.after(() => page.unmount());
  const names = () => page.find(node => node.type === 'b').map(textOf);
  assert.deepEqual(names(), ['New', 'Old One']); assert.ok(page.text().includes('@newbie'));
  assert.ok(page.text().includes("Unblocking doesn't restore the friendship."));
  fake.fail('unblock', 'SERVICE_UNAVAILABLE', 503);
  const unblock = () => page.find(node => node.type === 'button' && textOf(node) === 'Unblock')[0]!;
  await page.click(unblock());
  assert.ok(page.text().includes(friendsErrorText('SERVICE_UNAVAILABLE', false)));
  fake.replies.delete('unblock');
  await page.click(unblock());
  assert.deepEqual(fake.executed('unblock').map(command => (command as { otherUid: string }).otherUid), ['new', 'new']);
  fake.emitBlocks([]); await page.during(() => undefined);
  assert.ok(page.text().includes("You haven't blocked anyone."));
});

// ── The main area and its chat (D9, D10) ──

const chatSetup = (fake: FakeFriends, teams: FakeTeams) => {
  fake.cards = [card('bob', { handle: 'bobby', lastSeq: 2, lastReadSeq: 0, lastMessage: pointer(2, { preview: 'Hello' }) })];
  teams.conversations.set(pairTeamId('bob'), [conversation('friend_chat', { kind: 'dm', title: '', participants: ['user:alice', 'user:bob'], lastSeq: 2, lastReadSeq: 0 })]);
  teams.threads.set('friend_chat', [message(1, { text: 'Hi Alice' }), message(2, { text: 'Hello there', createdAt: '2026-11-03T10:00:00.000Z' })]);
};

scenario('a friend\'s chat is the Team conversation with the friend in the header and none of the Team around it', async context => {
  const { fake, workspace, snapshot } = friendsFixture(chatSetup);
  context.after(() => workspace.close());
  await ready(snapshot);
  workspace.openChat('bob');
  const page = await mount(workspace, view(workspace, FriendsMain)); context.after(() => page.unmount());
  await until(() => !!snapshot().chat?.workspace?.getSnapshot().messages.length, 'the thread'); await page.during(() => undefined);
  const text = page.text();
  assert.ok(text.includes('Bob') && text.includes('@bobby')); assert.ok(text.includes('Hi Alice') && text.includes('Hello there'));
  assert.equal(page.byLabel('Team details').length, 0, 'no details panel'); assert.equal(page.byClass('tw-platform').length, 0, 'no platform chip');
  assert.ok(!page.has('New conversation') && !text.includes('Channels'), 'one conversation, nothing to add to');
  assert.equal(page.find(node => node.type === 'textarea').length, 1, 'the composer is the Teams composer');
  assert.ok(!text.includes('You and Bob are now friends'), 'no greeting for an old friendship');
  assert.deepEqual(fake.commands, [], 'opening a chat writes nothing to Friends');
});

scenario('the chat reads itself while its newest message is in view, with the Teams read command on the pair team', async context => {
  const { workspace, snapshot, teams } = friendsFixture(chatSetup);
  context.after(() => workspace.close());
  await ready(snapshot);
  workspace.openChat('bob');
  const page = await mount(workspace, view(workspace, FriendsMain)); context.after(() => page.unmount());
  await until(() => teams.commands.some(command => command.action === 'read'), 'the read mark');
  const read = teams.commands.find(command => command.action === 'read') as { teamId: string; conversationId: string; lastReadSeq: number };
  assert.deepEqual([read.teamId, read.conversationId, read.lastReadSeq], [pairTeamId('bob'), 'friend_chat', 2]);
});

scenario('the friend menu: View profile, Remove friend and Block, the last two behind a confirmation that tells a failure', async context => {
  const { fake, workspace, snapshot } = friendsFixture((f, teams) => { chatSetup(f, teams); f.names.set('bob', { displayName: 'Robert', handle: 'rob' }); });
  context.after(() => workspace.close());
  await ready(snapshot);
  workspace.openChat('bob');
  const page = await mount(workspace, view(workspace, FriendsMain)); context.after(() => page.unmount());
  await until(() => !!snapshot().chat?.workspace?.getSnapshot().conversation, 'the chat'); await page.during(() => undefined);
  await page.click(page.oneByLabel('More'));
  assert.deepEqual(page.find(node => node.props.role === 'menuitem').map(textOf), ['View profile', 'Remove friend', 'Block']);
  await page.click(page.find(node => node.props.role === 'menuitem' && textOf(node) === 'View profile')[0]);
  await until(() => fake.readProfiles.includes('bob'), 'the friend\'s profile'); await page.during(() => undefined);
  assert.ok(textOf(dialogs(page)[0]).includes('@rob') && textOf(dialogs(page)[0]).includes('Friends since'), 'a fresh name and the date they became friends');
  assert.equal(snapshot().chat?.name, 'Robert');
  await page.click(dialogs(page)[0].findAll((node: any) => node.type === 'button').find((node: any) => textOf(node) === 'Close'));

  await page.click(page.oneByLabel('More'));
  await page.click(page.find(node => node.props.role === 'menuitem' && textOf(node) === 'Remove friend')[0]);
  assert.equal(fake.executed('remove').length, 0, 'asking is not doing'); assert.ok(textOf(dialogs(page)[0]).includes('the chat is deleted for both of you'));
  fake.fail('remove', 'SERVICE_UNAVAILABLE', 503);
  await page.click(dialogs(page)[0].findAll((node: any) => node.type === 'button').find((node: any) => textOf(node) === 'Remove friend'));
  assert.ok(textOf(dialogs(page)[0]).includes(friendsErrorText('SERVICE_UNAVAILABLE', false))); assert.equal(snapshot().page, 'chat');
  fake.replies.delete('remove');
  await page.click(dialogs(page)[0].findAll((node: any) => node.type === 'button').find((node: any) => textOf(node) === 'Remove friend'));
  assert.equal(snapshot().page, 'home'); assert.equal(fake.executed('remove').length, 2);
});

scenario('a chat whose friendship is over says so with a way back, and a person who was never a friend gets the same', async context => {
  const { fake, workspace, snapshot } = friendsFixture(chatSetup);
  context.after(() => workspace.close());
  await ready(snapshot);
  workspace.openChat('bob');
  const page = await mount(workspace, view(workspace, FriendsMain)); context.after(() => page.unmount());
  await until(() => !!snapshot().chat?.workspace?.getSnapshot().conversation, 'the chat'); await page.during(() => undefined);
  fake.emitCards([]); await page.during(() => undefined);
  assert.ok(page.text().includes('This chat is no longer available.'));
  await page.click(page.button('Back')); assert.equal(snapshot().page, 'home');
  workspace.openChat('stranger'); await page.during(() => undefined);
  assert.ok(page.text().includes('This chat is no longer available.'));
});

scenario('a new friendship greets once and the main area shows the page the workspace names', async context => {
  const { workspace, snapshot } = friendsFixture(chatSetup);
  context.after(() => workspace.close());
  await ready(snapshot);
  const page = await mount(workspace, view(workspace, FriendsMain)); context.after(() => page.unmount());
  assert.ok(page.text().includes('Friends') && page.text().includes('Add someone by @username'), 'the root shows a short welcome');
  workspace.show('profile'); await page.during(() => undefined); assert.ok(page.text().includes('Profile settings'));
  workspace.show('blocked'); await page.during(() => undefined); assert.ok(page.text().includes('Blocked people'));
  workspace.show('add'); await page.during(() => undefined); assert.ok(page.text().includes('Find someone'));
  workspace.openChat('bob', true);
  await until(() => !!snapshot().chat?.workspace?.getSnapshot().conversation, 'the chat'); await page.during(() => undefined);
  assert.ok(page.text().includes('You and Bob are now friends'));
});

// ── Links and Team members (D8, D15) ──

scenario('a friend link is looked up, never redeemed on open, and confirmed in a dialog', async context => {
  const { fake, workspace, snapshot } = friendsFixture();
  context.after(() => workspace.close());
  await ready(snapshot);
  const code = 'aB0_aB0_aB0_aB0_aB0_aB0_aB0_aB0-';
  fake.lookup({ uid: 'sam', displayName: 'Sam', relation: 'none' });
  let closed = 0;
  const page = await mount(workspace, view(workspace, FriendLinkDialog, { code, onClose: () => { closed++; } })); context.after(() => page.unmount());
  await until(() => fake.commands.length === 1, 'the lookup'); await page.during(() => undefined);
  assert.deepEqual(fake.commands, [{ action: 'lookup', code }]);
  assert.ok(textOf(dialogs(page)[0]).includes('Add “Sam” as a friend?')); assert.ok(textOf(dialogs(page)[0]).includes('Adding them makes you friends right away.'));
  assert.equal(fake.executed('redeem').length, 0, 'opening is not adding');
  await page.click(page.button('Cancel')); assert.equal(closed, 1); assert.equal(fake.executed('redeem').length, 0);
  await page.click(page.button('Add friend'));
  assert.equal(fake.executed('redeem').length, 1); assert.equal((fake.executed('redeem')[0] as { code: string }).code, code);
  assert.equal(closed, 2); assert.equal(snapshot().page, 'chat', 'the new friend\'s chat opens');
});

scenario('a friend link that no longer works says so; one for an existing friend offers the chat', async context => {
  const { fake, workspace, snapshot } = friendsFixture();
  context.after(() => workspace.close());
  await ready(snapshot);
  const code = 'aB0_aB0_aB0_aB0_aB0_aB0_aB0_aB0-';
  fake.fail('lookup', 'NOT_FOUND', 404);
  const gone = await mount(workspace, view(workspace, FriendLinkDialog, { code, onClose: () => undefined })); context.after(() => gone.unmount());
  await until(() => fake.commands.length === 1, 'the lookup'); await gone.during(() => undefined);
  assert.ok(gone.text().includes('This friend link no longer works. Ask them for a new one.')); assert.ok(!gone.has('Add friend'));
  await gone.unmount();

  fake.lookup({ uid: 'bob', displayName: 'Bob', relation: 'friends' });
  const known = await mount(workspace, view(workspace, FriendLinkDialog, { code, onClose: () => undefined })); context.after(() => known.unmount());
  await until(() => fake.commands.length === 2, 'the lookup'); await known.during(() => undefined);
  assert.ok(known.text().includes('Already friends'));
  await known.click(known.button('Open chat')); assert.equal(snapshot().chat?.uid, 'bob');
  assert.equal(fake.executed('redeem').length, 0);
});

scenario('a Team member can be added as a friend from their row, with the proof being the team and the member; nobody else is offered', async context => {
  const teamsFake = new FakeTeams();
  teamsFake.teams = [team('team-aaaa', { role: 'owner' })];
  const workspace = new TeamsWorkspace(teamsFake.api, uid, { storage: memoryStorage, environment: new EventTarget(), refreshDelay: 5 });
  context.after(() => workspace.close());
  await until(() => workspace.getSnapshot().detail?.members.length === 3, 'the team');
  const added: TeamMember[] = [];
  const page = await mount(workspace, snapshot => createElement(TeamDetailsDrawer as never, { workspace, snapshot, zh: false, tab: 'members', onTabChange() {}, onClose() {}, onAddFriend: (member: TeamMember) => added.push(member) } as never));
  context.after(() => page.unmount());
  const buttons = page.find(node => node.type === 'button' && textOf(node) === 'Add friend');
  assert.equal(buttons.length, 2, 'every member but me');
  await page.click(buttons[0]!);
  assert.deepEqual(added.map(member => member.uid), ['bob']);
  const without = await mount(workspace, snapshot => createElement(TeamDetailsDrawer as never, { workspace, snapshot, zh: false, tab: 'members', onTabChange() {}, onClose() {} } as never));
  context.after(() => without.unmount());
  assert.equal(without.find(node => node.type === 'button' && textOf(node) === 'Add friend').length, 0, 'a host that has no Friends offers nothing');
});

scenario('the member dialog looks the member up by team and uid and requests through the team; a member who is a friend says so', async context => {
  const { fake, workspace, snapshot } = friendsFixture();
  context.after(() => workspace.close());
  await ready(snapshot);
  fake.lookup({ uid: 'bob', displayName: 'Bob', relation: 'none' });
  let closed = 0;
  const page = await mount(workspace, view(workspace, MemberFriendDialog, { teamId: 'team-aaaa', member: { uid: 'bob', displayName: 'Bob' }, onClose: () => { closed++; } })); context.after(() => page.unmount());
  await until(() => fake.commands.length === 1, 'the lookup'); await page.during(() => undefined);
  assert.deepEqual(fake.commands, [{ action: 'lookup', teamId: 'team-aaaa', uid: 'bob' }]);
  await page.type(page.find(node => node.type === 'textarea')[0], 'We are on the same team');
  await page.click(page.find(node => node.type === 'button' && textOf(node) === 'Add friend' && classes(node).includes('tw-primary'))[0]);
  const request = fake.executed('request')[0] as { via: string; teamId: string; uid: string; note: string };
  assert.deepEqual([request.via, request.teamId, request.uid, request.note], ['team', 'team-aaaa', 'bob', 'We are on the same team']);
  assert.ok(page.text().includes('Friend request sent to Bob')); assert.equal(closed, 0, 'the dialog says what happened and waits to be closed');
  await page.click(page.button('Close')); assert.equal(closed, 1);

  fake.lookup({ uid: 'bob', relation: 'friends' });
  const known = await mount(workspace, view(workspace, MemberFriendDialog, { teamId: 'team-aaaa', member: { uid: 'bob', displayName: 'Bob' }, onClose: () => undefined })); context.after(() => known.unmount());
  await until(() => fake.commands.filter(command => command.action === 'lookup').length === 2, 'the second lookup'); await known.during(() => undefined);
  assert.ok(known.text().includes('Already friends'));
});
