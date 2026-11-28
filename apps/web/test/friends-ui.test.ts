import assert from 'node:assert/strict';
import { after, type TestContext } from 'node:test';
import { createElement } from 'react';
import { FriendApiError } from '../../../packages/teams/friends';
import { friendsErrorText, offerErrorText } from '../../../packages/teams/errors';
import type { TeamMember } from '../../../packages/teams';
import type { MessageCommand } from '../../../packages/teams/messages';
import type { FriendsSnapshot, FriendsWorkspace } from '../../../packages/teams/friends-workspace';
import { TeamsWorkspace } from '../../../packages/teams/workspace';
import { block, card, friendsFixture, offer, pairTeamId, pointer, profile, uid, until, type FakeFriends } from './support/fakeFriends';
import { FakeTeams, botCard, conversation, memoryStorage, message, team } from './support/fakeTeams';
import { loadTeamsModule } from './support/loadTeamsModule';
import { classes, mount, reactTestRenderer, scenario, textOf, withDom } from './support/workspacePage';

const { FriendsSidebar } = loadTeamsModule<typeof import('../../../packages/teams/FriendsSidebar')>('FriendsSidebar');
const { AddFriendView } = loadTeamsModule<typeof import('../../../packages/teams/AddFriendView')>('AddFriendView');
const { MyFriendCard } = loadTeamsModule<typeof import('../../../packages/teams/MyFriendCard')>('MyFriendCard');
const { FriendProfileSettings } = loadTeamsModule<typeof import('../../../packages/teams/FriendProfileSettings')>('FriendProfileSettings');
const { BlockedList } = loadTeamsModule<typeof import('../../../packages/teams/BlockedList')>('BlockedList');
const { FriendsMain } = loadTeamsModule<typeof import('../../../packages/teams/FriendsMain')>('FriendsMain');
const { FriendLinkDialog, InviteFriendsDialog, InviteNotice, MemberFriendDialog } = loadTeamsModule<typeof import('../../../packages/teams/friend-dialogs')>('friend-dialogs');
const { TeamSettingsPage } = loadTeamsModule<typeof import('../../../packages/teams/TeamSettingsPage')>('TeamSettingsPage');

// The fake browser stays for the whole file, so a view that is unmounted after its test still finds its document.
withDom({ after } as unknown as TestContext);

const NOW = new Date(2026, 10, 3, 12).getTime();
const view = (workspace: FriendsWorkspace, component: unknown, more: Record<string, unknown> = {}) => (snapshot: FriendsSnapshot) =>
  createElement(component as never, { workspace, snapshot, locale: 'en-US', now: NOW, ...more } as never);
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

scenario('a friend opens its chat; its unread badge survives navigation until validated exposure', async context => {
  const { workspace, snapshot } = friendsFixture(f => { f.cards = [card('bob', { lastSeq: 5, lastReadSeq: 3, lastMessage: pointer(5) }), card('carol')]; });
  context.after(() => workspace.close());
  await ready(snapshot);
  const page = await mount(workspace, view(workspace, FriendsSidebar)); context.after(() => page.unmount());
  const bob = page.byClass('tw-row-main').find(node => textOf(node).includes('Bob'))!;
  await page.click(bob);
  assert.equal(snapshot().page, 'chat'); assert.equal(snapshot().chat?.uid, 'bob');
  const selected = page.byClass('tw-row-main').find(node => textOf(node).includes('Bob'))!;
  assert.equal(selected.props['aria-current'], 'page'); assert.equal(page.byClass('tw-badge').length, 1, 'navigation cannot read unseen content');
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
  assert.ok(textOf(row('U24')).includes(friendsErrorText('FRIEND_LIMIT', 'en')), 'the error is under the row that failed');
  assert.ok(!textOf(row('U23')).includes(friendsErrorText('FRIEND_LIMIT', 'en')));
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

scenario('a pending chat shows Retry and Back after its first cards listener fails', async context => {
  const { fake, workspace, snapshot } = friendsFixture(f => { f.quiet = true; });
  context.after(() => workspace.close());
  const page = await mount(workspace, view(workspace, FriendsMain)); context.after(() => page.unmount());
  await page.during(() => workspace.openChat('bob'));
  await page.during(() => fake.failCards('SERVICE_UNAVAILABLE'));
  assert.ok(page.has('Retry'));
  await page.click(page.button('Retry'));
  assert.equal(fake.cardWatches.length, 2);
  await page.during(() => fake.failCards('SERVICE_UNAVAILABLE'));
  await page.click(page.button('Back'));
  assert.equal(snapshot().page, 'home');
});

scenario('the header: Add friend, a menu with exactly Profile settings and Blocked people, and the name banner for "Member"', async context => {
  const { fake, workspace, snapshot } = friendsFixture(f => { f.cards = [card('bob')]; f.profile = profile({ displayName: 'Member' }); });
  context.after(() => workspace.close());
  await ready(snapshot);
  const page = await mount(workspace, view(workspace, FriendsSidebar)); context.after(() => page.unmount());
  assert.ok(page.text().includes('Set your name so friends recognize you.')); assert.equal(snapshot().profileNameUnset, true, 'the workspace says it, the view does not compare text');
  await page.click(page.button('Set name')); assert.equal(snapshot().page, 'profile');
  await page.click(page.button('Add friend')); assert.equal(snapshot().page, 'add');
  await page.click(page.oneByLabel('More'));
  const items = page.find(node => node.props.role === 'menuitem');
  assert.deepEqual(items.map(textOf), ['Profile settings', 'Blocked people']);
  await page.click(items[1]!); assert.equal(snapshot().page, 'blocked');
  fake.profile = profile({ displayName: 'Alice' });
  await workspace.refreshProfile(); await page.during(() => undefined);
  assert.ok(!page.text().includes('Set your name so friends recognize you.')); assert.equal(snapshot().profileNameUnset, false);
});

// ── The friend rows: one tab stop, the arrow keys and a menu of their own (U6, U9) ──

/** The rows of the list as the browser has them: `matches` says an element is a row, `focus` says where the focus went. */
const rowList = (ids: string[]) => {
  const focused: string[] = [];
  const rows = ids.map(id => ({ id, matches: () => true, focus: () => { focused.push(id); } }));
  return { rows, focused, list: { querySelectorAll: () => rows } };
};
const sideAndMain = (workspace: FriendsWorkspace) => (snapshot: FriendsSnapshot) => createElement('div', null, view(workspace, FriendsSidebar)(snapshot), view(workspace, FriendsMain)(snapshot));

scenario('the friends are one tab stop: the arrow keys, Home and End move between the rows, and the stop follows the focus or the open chat', async context => {
  const { workspace, snapshot } = friendsFixture(f => {
    f.cards = [card('bob', { lastSeq: 1, lastReadSeq: 1, lastMessage: pointer(1, { at: at(2) }) }), card('carol', { lastSeq: 1, lastReadSeq: 1, lastMessage: pointer(1, { at: at(3) }) }), card('dave')];
  });
  context.after(() => workspace.close());
  await ready(snapshot);
  const page = await mount(workspace, view(workspace, FriendsSidebar)); context.after(() => page.unmount());
  const mains = () => page.byClass('tw-row-main'), mores = () => page.byClass('tw-row-more');
  assert.deepEqual(mains().map(node => node.props['data-row']), ['carol', 'bob', 'dave'], 'the roving items are the rows, in the order of the list');
  assert.deepEqual(mains().map(node => node.props.tabIndex), [0, -1, -1], 'one tab stop: the first friend'); assert.deepEqual(mores().map(node => node.props.tabIndex), [0, -1, -1], 'and the row\'s own button goes with it');
  const sections = page.byClass('tw-sections')[0]!, { rows, focused, list } = rowList(['carol', 'bob', 'dave']);
  const press = (key: string, from: number) => page.key(sections, key, { target: rows[from], currentTarget: list });
  assert.equal(await press('ArrowDown', 0), true, 'the key is the list\'s: the page does not scroll'); await press('ArrowDown', 1); await press('ArrowDown', 2); await press('ArrowUp', 2); await press('Home', 2); await press('End', 0);
  assert.deepEqual(focused, ['bob', 'dave', 'dave', 'bob', 'carol', 'dave'], 'the list stops at its ends');
  assert.equal(await press('a', 0), false, 'a key that is not for the list is left alone'); assert.equal(focused.length, 6);
  assert.equal(await page.key(sections, 'ArrowDown', { target: { matches: () => false }, currentTarget: list }), false, 'and so is a key typed in the filter or on a request');
  workspace.openChat('dave'); await page.during(() => undefined);
  assert.deepEqual(mains().map(node => node.props.tabIndex), [-1, -1, 0], 'with nothing focused the open chat is the stop');
  await page.during(() => mains()[1]!.props.onFocus());
  assert.deepEqual(mains().map(node => node.props.tabIndex), [-1, 0, -1], 'and the row that had the focus is the stop after it');
  await page.type(page.oneByLabel('Search friends'), 'zz'); assert.equal(mains().length, 0);
  await page.type(page.oneByLabel('Search friends'), ''); assert.equal(mains().filter(node => node.props.tabIndex === 0).length, 1, 'there is always exactly one stop');
});

scenario('a friend\'s menu opens from its more button, a right click, the Menu key or Shift+F10; Shared Bots opens the chat with its Bots', async context => {
  const { fake, workspace, snapshot } = friendsFixture((f, teams) => {
    chatSetup(f, teams); confirmBotDms(teams); f.names.set('bob', { displayName: 'Robert', handle: 'rob' });
    teams.bots = [botCard({ name: 'Bob Helper', maintainer: { uid: 'bob', generation: 1 } })];
  });
  context.after(() => workspace.close());
  await ready(snapshot);
  const page = await mount(workspace, sideAndMain(workspace)); context.after(() => page.unmount());
  const main = () => page.byClass('tw-row-main')[0]!, more = () => page.byClass('tw-row-more')[0]!;
  const menus = () => page.find(node => node.props.role === 'menu'), items = () => page.find(node => node.props.role === 'menuitem');
  assert.equal(more().props['aria-label'], 'More: Bob', 'the button says whose menu it is'); assert.equal(more().props['aria-haspopup'], 'menu'); assert.equal(menus().length, 0);
  // The browser raises `contextmenu` for a right click, the Menu key and Shift+F10 alike: the row opens its own menu and the browser's stays shut.
  let prevented = 0;
  await page.during(() => main().props.onContextMenu({ preventDefault() { prevented++; } }));
  assert.equal(prevented, 1); assert.equal(menus().length, 1); assert.equal(more().props['aria-expanded'], true);
  assert.deepEqual(items().map(textOf), ['View profile', 'Shared Bots', 'Remove friend']); assert.equal(menus()[0]!.props['aria-label'], 'Bob');
  await page.click(more()); assert.equal(menus().length, 0, 'the more button closes it again');
  await page.click(more()); assert.equal(menus().length, 1, 'and opens it');
  // View profile: the friend's own dialog, with the fresh name and the date they became friends.
  await page.click(items().find(node => textOf(node) === 'View profile')!);
  await until(() => fake.readProfiles.includes('bob'), 'the friend\'s profile'); await page.during(() => undefined);
  assert.equal(menus().length, 0); assert.ok(textOf(dialogs(page)[0]).includes('@rob') && textOf(dialogs(page)[0]).includes('Friends since'));
  await page.click(dialogs(page)[0].findAll((node: any) => node.type === 'button').find((node: any) => textOf(node) === 'Close')); assert.equal(dialogs(page).length, 0);
  // Shared Bots: the chat opens with the panel, and asking again opens it again after it was closed.
  await page.during(() => main().props.onContextMenu({ preventDefault() {} }));
  await page.click(items().find(node => textOf(node) === 'Shared Bots')!);
  assert.equal(snapshot().page, 'chat'); assert.equal(snapshot().chat?.uid, 'bob'); assert.equal(snapshot().chat?.botsRequest, 1);
  await until(() => !!snapshot().chat?.workspace?.getSnapshot().conversation, 'the chat'); await page.during(() => undefined);
  const opener = () => page.byLabel('Shared Bots').find(node => node.type === 'button')!;
  assert.equal(opener().props['aria-expanded'], true); assert.equal(page.byClass('tw-friends-bots').length, 1, 'the panel is open with the chat');
  assert.equal(page.byClass('tw-friends-bots')[0]!.props.id, opener().props['aria-controls'], 'the toggle controls the panel');
  await page.click(page.byLabel('Close').find(node => node.type === 'button')!); assert.equal(page.byClass('tw-friends-bots').length, 0);
  await page.during(() => main().props.onContextMenu({ preventDefault() {} }));
  await page.click(items().find(node => textOf(node) === 'Shared Bots')!);
  assert.equal(snapshot().chat?.botsRequest, 2); assert.equal(page.byClass('tw-friends-bots').length, 1, 'the open chat shows the panel again');
  // Remove friend: the same confirmation as in the chat, and Cancel is the way out.
  await page.during(() => main().props.onContextMenu({ preventDefault() {} }));
  await page.click(items().find(node => textOf(node) === 'Remove friend')!);
  const dialog = () => dialogs(page)[0], inDialog = (name: string) => dialog().findAll((node: any) => node.type === 'button').find((node: any) => textOf(node) === name);
  assert.equal(dialog().props.role, 'alertdialog'); assert.ok(textOf(dialog()).includes('Remove “Robert”?') && textOf(dialog()).includes('the chat is deleted for both of you'), 'the title names the friend as the list has them now');
  assert.equal(inDialog('Cancel').props['data-autofocus'], true, 'the safe choice has the focus'); assert.equal(fake.executed('remove').length, 0, 'asking is not doing');
  await page.click(inDialog('Cancel')); assert.equal(dialogs(page).length, 0); assert.equal(fake.executed('remove').length, 0);
  await page.during(() => main().props.onContextMenu({ preventDefault() {} }));
  await page.click(items().find(node => textOf(node) === 'Remove friend')!); await page.click(inDialog('Remove friend'));
  assert.equal(fake.executed('remove').length, 1); assert.equal(dialogs(page).length, 0); assert.equal(snapshot().page, 'home', 'the friend\'s chat ends with the friendship');
});

scenario('a confirmation about a friend can be cancelled while its change is in flight; only the confirming button waits', async context => {
  const { fake, workspace, snapshot } = friendsFixture((f, teams) => chatSetup(f, teams));
  context.after(() => workspace.close());
  await ready(snapshot);
  const page = await mount(workspace, view(workspace, FriendsSidebar)); context.after(() => page.unmount());
  await page.during(() => page.byClass('tw-row-main')[0]!.props.onContextMenu({ preventDefault() {} }));
  await page.click(page.find(node => node.props.role === 'menuitem' && textOf(node) === 'Remove friend')[0]);
  const inDialog = (name: string) => dialogs(page)[0].findAll((node: any) => node.type === 'button').find((node: any) => textOf(node) === name);
  let release = () => {}; fake.replies.set('remove', () => new Promise(resolve => { release = () => resolve({ ok: true }); }));
  await page.click(inDialog('Remove friend'));
  assert.equal(inDialog('Remove friend').props.disabled, true, 'it is not sent twice'); assert.notEqual(inDialog('Cancel').props.disabled, true, 'but the person is not held in the dialog');
  await page.click(inDialog('Cancel')); assert.equal(dialogs(page).length, 0);
  await page.during(() => { release(); });
});

scenario('the main area with nothing open says what Friends is and offers the one thing to do: Add friend', async context => {
  const none = friendsFixture(); context.after(() => none.workspace.close()); await ready(none.snapshot);
  const empty = await mount(none.workspace, view(none.workspace, FriendsMain)); context.after(() => empty.unmount());
  assert.equal(textOf(empty.find(node => node.type === 'h2')[0]), 'No friends yet'); assert.ok(empty.text().includes('Add someone by @username, email or friend link to chat one-to-one.'));
  await empty.click(empty.button('Add friend')); assert.equal(none.snapshot().page, 'add', 'the way to add one is in the area itself, not only in the header');
  const some = friendsFixture(f => { f.cards = [card('bob')]; }); context.after(() => some.workspace.close()); await ready(some.snapshot);
  const filled = await mount(some.workspace, view(some.workspace, FriendsMain)); context.after(() => filled.unmount());
  assert.equal(textOf(filled.find(node => node.type === 'h2')[0]), 'Friends', 'with friends it is not "No friends yet"');
  await filled.click(filled.button('Add friend')); assert.equal(some.snapshot().page, 'add');
  some.workspace.show('home');
  const zh = await mount(some.workspace, view(some.workspace, FriendsMain, { locale: 'zh-CN' })); context.after(() => zh.unmount());
  assert.equal(textOf(zh.find(node => node.type === 'h2')[0]), '好友'); assert.ok(zh.text().includes('添加好友'));
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
  fake.searchHandles({ uid: 'sam', handle: 'sam' });
  fake.lookup({ uid: 'sam' });
  assert.equal(page.button('Look up').props.disabled, true, 'nothing to look up');
  await page.type(field(), 'not a person!');
  assert.equal(page.button('Look up').props.disabled, true); assert.ok(page.text().includes('Enter at least 3 username characters, an email address or a friend link.'));
  await page.type(field(), '@sam');
  assert.equal(page.button('Look up').props.disabled, false);
  await page.submit(page.find(node => node.type === 'form')[0]);
  assert.deepEqual(fake.commands, [{ action: 'searchHandles', query: 'sam' }]);
  await page.type(field(), '  Sam@Example.com ');
  await page.submit(page.find(node => node.type === 'form')[0]);
  const code = 'aB0_aB0_aB0_aB0_aB0_aB0_aB0_aB0-';
  await page.type(field(), `https://lingxios.net/open/friend/add?code=${code}`);
  await page.submit(page.find(node => node.type === 'form')[0]);
  assert.deepEqual(fake.commands.slice(1), [{ action: 'lookup', email: 'sam@example.com' }, { action: 'lookup', code }]);
});

scenario('username search runs after typing and drops results for a superseded query', async context => {
  const { fake, page, field } = await addPage(context);
  let resolveOld!: (result: unknown) => void;
  let obsoleteSignal: AbortSignal | undefined;
  fake.replies.set('searchHandles', (_command: unknown, signal: AbortSignal) => new Promise(resolve => {
    resolveOld = resolve; obsoleteSignal = signal;
  }));
  await page.type(field(), '@lic');
  await page.during(() => until(() => fake.commands.length === 1, 'debounced username search'));
  assert.deepEqual(fake.commands, [{ action: 'searchHandles', query: 'lic' }]);

  await page.type(field(), '@smi');
  assert.equal(obsoleteSignal?.aborted, true);
  fake.searchHandles({ uid: 'will', handle: 'willsmith' });
  await page.submit(page.find(node => node.type === 'form')[0]);
  await page.during(() => resolveOld({ results: [{ uid: 'alice', displayName: 'Alice', handle: 'alice', relation: 'none' }] }));
  assert.ok(page.text().includes('@willsmith'));
  assert.ok(!page.text().includes('@alice'));
});

scenario('equivalent username edits, Enter, and returning to results reuse the current result list', async context => {
  const { fake, page, field } = await addPage(context);
  fake.searchHandles({ uid: 'sam', handle: 'sami' }, { uid: 'sam2', handle: 'sam_2' });
  const submit = () => page.submit(page.find(node => node.type === 'form')[0]);
  await page.type(field(), 'sam'); await submit();
  await page.type(field(), ' @SAM '); await submit();
  assert.equal(page.button('Look up').props.disabled, true);
  await page.click(page.byClass('tw-lookup-who').find(node => node.type === 'button')!);
  await page.click(page.button('Back to results'));
  assert.ok(page.text().includes('@sami') && page.text().includes('@sam_2'));
  assert.equal(fake.executed('searchHandles').length, 1);
});

scenario('live search waits for the rate-limit cooldown before auto-searching new input', async context => {
  const { fake, page, field, snapshot } = await addPage(context);
  fake.replies.set('searchHandles', new FriendApiError('RATE_LIMITED', 429, 700));
  await page.type(field(), 'sam'); await page.submit(page.find(node => node.type === 'form')[0]);
  await page.type(field(), 'alex');
  assert.equal(page.button('Look up').props.disabled, true);
  fake.searchHandles({ uid: 'alex', handle: 'alex' });
  await page.during(() => new Promise(resolve => setTimeout(resolve, 420)));
  assert.equal(fake.executed('searchHandles').length, 1);
  await page.during(() => until(() => !snapshot().handleSearchRetryUntil, 'the cooldown'));
  await page.during(() => until(() => fake.executed('searchHandles').length === 2, 'the debounce'));
  assert.ok(page.text().includes('@alex'));
});

scenario('an empty result can be submitted again without changing the username', async context => {
  const { fake, page, field } = await addPage(context);
  fake.searchHandles();
  await page.type(field(), 'alice'); await page.submit(page.find(node => node.type === 'form')[0]);
  assert.equal(page.button('Look up').props.disabled, false);
  fake.searchHandles({ uid: 'alice', handle: 'alice' });
  await page.submit(page.find(node => node.type === 'form')[0]);
  assert.ok(page.text().includes('@alice'));
  assert.equal(fake.executed('searchHandles').length, 2);
});

scenario('reopening Add Friend in one account preserves the search cooldown', async context => {
  const { fake, workspace, snapshot } = friendsFixture();
  context.after(() => workspace.close());
  await ready(snapshot);
  workspace.show('add');
  const page = await mount(workspace, view(workspace, FriendsMain)); context.after(() => page.unmount());
  const field = () => page.find(node => node.type === 'input' && node.props['aria-label'] === 'Find someone')[0];
  fake.replies.set('searchHandles', new FriendApiError('RATE_LIMITED', 429, 5_000));
  await page.type(field(), 'sam'); await page.submit(page.find(node => node.type === 'form')[0]);
  await page.during(() => workspace.show('profile'));
  await page.during(() => workspace.show('add'));
  await page.type(field(), 'alice');
  assert.equal(page.button('Look up').props.disabled, true);
  await page.during(() => new Promise(resolve => setTimeout(resolve, 420)));
  assert.equal(fake.executed('searchHandles').length, 1);
});

scenario('a pending friend confirmation keeps the selected search from being replaced', async context => {
  const { fake, page, field } = await addPage(context);
  fake.searchHandles({ uid: 'sam', handle: 'sam' });
  await page.type(field(), 'sam');
  await page.submit(page.find(node => node.type === 'form')[0]);
  await page.click(page.byClass('tw-lookup-who').find(node => node.type === 'button')!);

  let complete!: (result: unknown) => void;
  fake.replies.set('request', () => new Promise(resolve => { complete = resolve; }));
  await page.click(page.button('Add friend'));
  assert.equal(field().props.disabled, true);
  assert.equal(page.button('Back to results').props.disabled, true);
  await page.type(field(), 'alice');
  assert.equal(field().props.value, 'sam');
  await page.during(() => complete({ state: 'outgoing', uid: 'sam' }));
  assert.equal(field().props.disabled, false);
});

scenario('the result card offers the one action of the relation: add with a note, accept, request sent, or open the chat', async context => {
  const { fake, workspace, snapshot, page, field } = await addPage(context);
  const search = async (query: string) => {
    await page.type(field(), '');
    await page.type(field(), query);
    await page.submit(page.find(node => node.type === 'form')[0]);
    await page.click(page.byClass('tw-lookup-who').find(node => node.type === 'button')!);
  };
  const card_ = () => page.oneByLabel('Sam');

  fake.searchHandles({ uid: 'sam', handle: 'sam', relation: 'none' });
  await search('@sam');
  assert.ok(textOf(card_()).includes('@sam'));
  assert.equal(page.find(node => node.type === 'textarea').length, 1, 'a note can ride a request');
  await page.type(page.find(node => node.type === 'textarea')[0], '  Met at the meetup ');
  await page.click(page.button('Add friend'));
  const request = fake.executed('request')[0] as { via: string; handle: string; expectedUid: string; note: string; requestId: string };
  assert.deepEqual([request.via, request.handle, request.expectedUid, request.note], ['handle', 'sam', 'sam', 'Met at the meetup']); assert.ok(request.requestId);
  assert.ok(page.text().includes('Friend request sent to Sam')); assert.equal(page.find(node => node.props.role === 'group').length, 0, 'the card is done');
  assert.equal(page.find(node => node.type === 'input' && node.props['aria-label'] === 'Find someone')[0].props.value, '', 'and the field is clear');

  fake.searchHandles({ uid: 'sam', handle: 'sam', relation: 'outgoing' });
  await search('@sam');
  const sent = page.button('Request sent'); assert.equal(sent.props.disabled, true);

  fake.searchHandles({ uid: 'sam', handle: 'sam', relation: 'incoming' });
  await search('@sam');
  await page.click(page.button('Accept'));
  assert.equal(fake.commands.at(-1)!.action, 'accept'); assert.equal(snapshot().page, 'chat'); assert.equal(snapshot().chat?.uid, 'sam', 'accepting opens the chat');

  workspace.show('add'); await page.during(() => undefined);
  fake.searchHandles({ uid: 'sam', handle: 'sam', relation: 'friends' });
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

  fake.fail('searchHandles', 'NOT_FOUND', 404);
  await page.type(field(), '@nobody'); await page.submit(page.find(node => node.type === 'form')[0]);
  assert.ok(page.text().includes(friendsErrorText('NOT_FOUND', 'en')));
  await page.type(field(), '@nobody2');
  assert.ok(!page.text().includes(friendsErrorText('NOT_FOUND', 'en')), 'a new query starts clean');
});

scenario('a failed confirmation is told on the card, which stays, and retrying an unknown outcome reuses the request id', async context => {
  const { fake, page, field } = await addPage(context);
  fake.searchHandles({ uid: 'sam', handle: 'sam' });
  await page.type(field(), '@sam'); await page.submit(page.find(node => node.type === 'form')[0]);
  await page.click(page.byClass('tw-lookup-who').find(node => node.type === 'button')!);
  fake.fail('request', 'SERVICE_UNAVAILABLE', 503);
  await page.click(page.button('Add friend'));
  assert.ok(textOf(page.oneByLabel('Sam')).includes(friendsErrorText('SERVICE_UNAVAILABLE', 'en')));
  await page.click(page.button('Add friend'));
  const [first, second] = fake.executed('request') as { requestId: string }[];
  assert.equal(first!.requestId, second!.requestId);
  fake.fail('request', 'REQUEST_LIMIT');
  await page.click(page.button('Add friend'));
  assert.ok(textOf(page.oneByLabel('Sam')).includes(friendsErrorText('REQUEST_LIMIT', 'en')));
});

scenario('my friend link: shown with copy, reset and turn off behind confirmations, and created again when off', async context => {
  const { fake, workspace, snapshot } = friendsFixture();
  context.after(() => workspace.close());
  await ready(snapshot);
  const page = await mount(workspace, view(workspace, FriendsMain)); context.after(() => page.unmount());
  const sidebar = await mount(workspace, view(workspace, FriendsSidebar)); context.after(() => sidebar.unmount());
  await page.click(page.button('Add friend'));
  assert.equal(page.find(node => node.type === 'input' && node.props.type === 'search').length, 1);
  assert.ok(!page.has('My friend link'));
  await sidebar.click(sidebar.byClass('tw-me-main')[0]!);
  assert.equal(snapshot().page, 'me');
  assert.ok(page.has('Profile settings') && page.has('Blocked people'));
  const qr = () => page.find(node => node.type === 'img' && node.props.alt === 'Friend invite QR code')[0]?.props.src;
  const firstQr = qr();
  assert.match(firstQr, /^data:image\/gif;base64,/);
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
  assert.notEqual(qr(), firstQr, 'the QR follows the current link');
  await page.click(page.button('Turn off link'));
  await page.click(dialogs(page)[0].findAll((node: any) => node.type === 'button').find((node: any) => textOf(node) === 'Turn off link'));
  assert.equal(fake.executed('revokeCode').length, 1); assert.ok(page.text().includes('Your friend link is off.'));
  assert.equal(qr(), undefined);
  await page.click(page.button('Create link'));
  assert.equal(fake.executed('rotateCode').length, 2); assert.equal(snapshot().profile?.friendCode?.code, 'b'.repeat(32));
  await page.click(page.button('Profile settings'));
  assert.equal(snapshot().page, 'profile');
  await sidebar.click(sidebar.byClass('tw-me-main')[0]!);
  await page.click(page.button('Blocked people'));
  assert.equal(snapshot().page, 'blocked');
});

scenario('opening My card reads the profile again, and a profile that cannot be read can be retried', async context => {
  const { fake, workspace, snapshot } = friendsFixture(f => { f.profileError = 'SERVICE_UNAVAILABLE'; });
  context.after(() => workspace.close());
  await until(() => snapshot().profileError !== '', 'the profile error');
  const reads = fake.profileCalls;
  const page = await mount(workspace, view(workspace, MyFriendCard)); context.after(() => page.unmount());
  await until(() => fake.profileCalls > reads, 'the page to read the profile'); await page.during(() => undefined);
  assert.ok(page.text().includes(friendsErrorText('SERVICE_UNAVAILABLE', 'en')));
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
  assert.equal(inputs().filter(node => node.props.type === 'checkbox').map(node => node.props.checked).join(), 'true,false,false');

  await page.type(inputs()[0], '   '); assert.equal(save().props.disabled, true, 'a name cannot be blank');
  await page.type(inputs()[0], 'x'.repeat(81)); assert.equal(save().props.disabled, true, 'or longer than 80');
  await page.type(inputs()[0], '  Ally  ');
  await page.type(inputs()[1], 'ab'); assert.equal(save().props.disabled, true, 'a username needs 3 to 24 letters, digits or underscores');
  await page.type(inputs()[1], 'ally_w');
  await page.check(inputs().filter(node => node.props.type === 'checkbox')[1], true);
  await page.check(inputs().filter(node => node.props.type === 'checkbox')[2], true);
  fake.fail('profile', 'HANDLE_TAKEN');
  await page.submit();
  assert.deepEqual(fake.commands, [{ action: 'profile', displayName: 'Ally', handle: 'ally_w', findByHandlePart: true, findByEmail: true }]);
  assert.ok(page.find(node => node.props.role === 'alert' && textOf(node) === friendsErrorText('HANDLE_TAKEN', 'en')).length === 1, 'told next to the username');
  fake.replies.delete('profile'); fake.profile = profile({ displayName: 'Ally', handle: 'ally_w', findByHandlePart: true, findByEmail: true });
  await page.submit();
  assert.equal(fake.commands.length, 2); assert.equal(snapshot().profile?.displayName, 'Ally');
  assert.equal(page.find(node => node.props.role === 'alert').length, 0);
});

scenario('saving a name after a profile refresh preserves untouched discovery switches', async context => {
  const { fake, workspace, snapshot } = friendsFixture(f => {
    f.profile = profile({ displayName: 'Alice', findByEmail: true, findByHandlePart: true });
  });
  context.after(() => workspace.close());
  await ready(snapshot);
  const page = await mount(workspace, view(workspace, FriendProfileSettings)); context.after(() => page.unmount());
  const inputs = () => page.find(node => node.type === 'input');
  await page.type(inputs()[0], 'Ally');
  fake.profile = profile({ displayName: 'Alice', findByEmail: false, findByHandlePart: false });
  await page.during(() => workspace.refreshProfile());
  assert.equal(inputs()[0].props.value, 'Ally');
  assert.deepEqual(inputs().filter(node => node.props.type === 'checkbox').map(node => node.props.checked), [true, false, false]);
  await page.submit();
  assert.deepEqual(fake.executed('profile'), [{ action: 'profile', displayName: 'Ally' }]);
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
  assert.ok(page.text().includes(friendsErrorText('SERVICE_UNAVAILABLE', 'en')));
  fake.replies.delete('unblock');
  await page.click(unblock());
  assert.deepEqual(fake.executed('unblock').map(command => (command as { otherUid: string }).otherUid), ['new', 'new']);
  fake.emitBlocks([]); await page.during(() => undefined);
  assert.ok(page.text().includes('No blocked people'), 'an empty list says so with a heading');
  assert.ok(page.text().includes('People you block can’t find you or send you requests. They will appear here.'));
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
  assert.ok(page.messageEditor(), 'the composer is the Teams composer');
  assert.ok(!text.includes('You and Bob are now friends'), 'no greeting for an old friendship');
  assert.deepEqual(fake.commands, [], 'opening a chat writes nothing to Friends');
});

scenario('a friend chat mounted without a visible DOM does not emit a read command', async context => {
  const { workspace, snapshot, teams } = friendsFixture(chatSetup);
  context.after(() => workspace.close());
  await ready(snapshot);
  workspace.openChat('bob');
  const page = await mount(workspace, view(workspace, FriendsMain)); context.after(() => page.unmount());
  await until(()=>!!snapshot().chat?.workspace?.getSnapshot().conversation,'the chat');
  assert.equal(teams.commands.some(command=>command.action==='read'),false);

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
  await page.click(page.oneByLabel('View profile'));
  assert.ok(textOf(dialogs(page)[0]).includes('Robert'), 'the header opens the same profile');
  await page.click(dialogs(page)[0].findAll((node: any) => node.type === 'button').find((node: any) => textOf(node) === 'Close'));


  await page.click(page.oneByLabel('More'));
  await page.click(page.find(node => node.props.role === 'menuitem' && textOf(node) === 'Remove friend')[0]);
  assert.equal(fake.executed('remove').length, 0, 'asking is not doing'); assert.ok(textOf(dialogs(page)[0]).includes('the chat is deleted for both of you'));
  fake.fail('remove', 'SERVICE_UNAVAILABLE', 503);
  await page.click(dialogs(page)[0].findAll((node: any) => node.type === 'button').find((node: any) => textOf(node) === 'Remove friend'));
  assert.ok(textOf(dialogs(page)[0]).includes(friendsErrorText('SERVICE_UNAVAILABLE', 'en'))); assert.equal(snapshot().page, 'chat');
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
  workspace.show('add'); await page.during(() => undefined); assert.equal(page.find(node => node.type === 'input' && node.props['aria-label'] === 'Find someone').length, 1);
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
  await page.unmount();
  const reopened = await mount(workspace, view(workspace, FriendLinkDialog, { code, onClose: () => { closed++; } }));
  context.after(() => reopened.unmount());
  await until(() => fake.commands.length === 2, 'the second lookup');
  await reopened.during(() => undefined);
  await reopened.click(reopened.button('Add friend'));
  assert.equal(fake.executed('redeem').length, 1); assert.equal((fake.executed('redeem')[0] as { code: string }).code, code);
  assert.equal(closed, 2); assert.equal(snapshot().page, 'chat');
});

scenario('dismissing a pending friend link keeps its completion from opening a chat', async context => {
  const { fake, workspace, snapshot } = friendsFixture();
  context.after(() => workspace.close());
  await ready(snapshot);
  fake.lookup({ uid: 'sam', displayName: 'Sam', relation: 'none' });
  let finish!: (result: unknown) => void;
  fake.replies.set('redeem', () => new Promise(resolve => { finish = resolve; }));
  let closed = 0;
  const page = await mount(workspace, view(workspace, FriendLinkDialog, {
    code: 'f'.repeat(32), onClose: () => { closed++; },
  }));
  context.after(() => page.unmount());
  await page.click(page.button('Add friend'));
  await page.click(page.button('Cancel'));
  await page.during(() => finish({ uid: 'sam', state: 'friends', teamId: pairTeamId('sam') }));
  assert.equal(closed, 1);
  assert.equal(snapshot().page, 'home');
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
  const page = await mount(workspace, snapshot => createElement(TeamSettingsPage as never, { workspace, snapshot, locale: 'en', initialSection: 'members', onClose() {}, onOpenChat() {}, onAddFriend: (member: TeamMember) => added.push(member) } as never));
  context.after(() => page.unmount());
  const buttons = page.find(node => node.type === 'button' && textOf(node) === 'Add friend');
  assert.equal(buttons.length, 2, 'every member but me');
  await page.click(buttons[0]!);
  assert.deepEqual(added.map(member => member.uid), ['bob']);
  const without = await mount(workspace, snapshot => createElement(TeamSettingsPage as never, { workspace, snapshot, locale: 'en', initialSection: 'members', onClose() {}, onOpenChat() {} } as never));
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

// ── Team invitations (D27, D30) ──

const HOUR = 3_600_000, DAY = 24 * HOUR;
const rowsOf = (page: { oneByLabel(label: string): any }, label: string): any[] => page.oneByLabel(label).findAll((node: any) => node.type === 'li');
const buttonOf = (row: any, text: string) => row.findAll((node: any) => node.type === 'button').find((node: any) => textOf(node) === text);
const buttonsOf = (row: any) => row.findAll((node: any) => node.type === 'button').map(textOf);

scenario('Team invitations sit above the requests: the team, who invited, when it expires, and Join and Decline, or Dismiss once it expired', async context => {
  const { fake, workspace, snapshot } = friendsFixture(f => {
    f.cards = [card('bob'), card('erin', { state: 'incoming' })];
    f.offers = [offer('t-gone', NOW - DAY, { teamName: 'Old Team' }), offer('t-hours', NOW + 5 * HOUR, { teamName: 'Soon Team', fromName: 'Carol' }), offer('t-days', NOW + 3 * DAY - HOUR, { teamName: 'Later Team' })];
  }, { now: () => NOW });
  context.after(() => workspace.close());
  await ready(snapshot);
  const page = await mount(workspace, view(workspace, FriendsSidebar)); context.after(() => page.unmount());
  const text = page.text();
  assert.ok(text.indexOf('Team invitations') >= 0 && text.indexOf('Team invitations') < text.indexOf('Friend requests'), 'the section is above the requests');
  const rows = rowsOf(page, 'Team invitations');
  assert.deepEqual(rows.map(row => textOf(row.findAll((node: any) => node.type === 'b')[0])), ['Later Team', 'Soon Team', 'Old Team'], 'the newest first');
  assert.ok(textOf(rows[0]).includes('Invited by Bob') && textOf(rows[0]).includes('Expires in 3 days'));
  assert.ok(textOf(rows[1]).includes('Invited by Carol') && textOf(rows[1]).includes('Expires in 5 hours'));
  assert.ok(textOf(rows[2]).includes('Expired') && !textOf(rows[2]).includes('Expires'));
  assert.deepEqual(buttonsOf(rows[0]), ['Join', 'Decline']); assert.deepEqual(buttonsOf(rows[2]), ['Dismiss'], 'an expired offer cannot be accepted');
  assert.equal(page.byClass('tw-team-mark').length, 3, 'each row has the team\'s mark');
  assert.equal(fake.commands.length, 0, 'showing offers sends nothing');
});

scenario('Join says so once and leaves the row to the listener; a failure is told under its row and Join stays; Decline and Dismiss send one decline', async context => {
  const { fake, workspace, snapshot } = friendsFixture(f => {
    f.cards = [card('bob')];
    f.offers = [offer('t-1', NOW + 2 * DAY, { teamName: 'Alpha' }), offer('t-2', NOW + DAY, { teamName: 'Beta' }), offer('t-3', NOW - HOUR, { teamName: 'Gamma' })];
  }, { now: () => NOW });
  context.after(() => workspace.close());
  await ready(snapshot);
  const page = await mount(workspace, view(workspace, FriendsSidebar)); context.after(() => page.unmount());
  const row = (name: string) => rowsOf(page, 'Team invitations').find(item => textOf(item).includes(name))!;

  fake.fail('acceptOffer', 'TEAM_LIMIT');
  await page.click(buttonOf(row('Alpha'), 'Join'));
  assert.ok(textOf(row('Alpha')).includes(offerErrorText('acceptOffer', 'TEAM_LIMIT', 'en')), 'told under the row that failed');
  assert.ok(!textOf(row('Beta')).includes(offerErrorText('acceptOffer', 'TEAM_LIMIT', 'en')));
  assert.equal(buttonOf(row('Alpha'), 'Join').props.disabled, false, 'the offer is kept: Join stays available');
  assert.ok(!page.text().includes('You joined'));
  fake.fail('acceptOffer', 'OFFER_UNAVAILABLE');
  await page.click(buttonOf(row('Alpha'), 'Join'));
  assert.ok(textOf(row('Alpha')).includes('This invitation is no longer available.'));

  fake.replies.delete('acceptOffer');
  await page.click(buttonOf(row('Alpha'), 'Join'));
  assert.ok(page.text().includes('You joined “Alpha”')); assert.equal(rowsOf(page, 'Team invitations').length, 3, 'the row goes through the listener');
  fake.emitOffers([offer('t-2', NOW + DAY, { teamName: 'Beta' }), offer('t-3', NOW - HOUR, { teamName: 'Gamma' })]); await page.during(() => undefined);
  assert.equal(rowsOf(page, 'Team invitations').length, 2);
  const joins = fake.executed('acceptOffer') as { offerId: string; requestId: string }[];
  assert.deepEqual(joins.map(command => command.offerId), ['t-1', 't-1', 't-1']);
  assert.equal(joins[0]!.requestId === joins[1]!.requestId, false, 'a definitive answer ended the id');

  await page.click(buttonOf(row('Beta'), 'Decline')); await page.click(buttonOf(row('Gamma'), 'Dismiss'));
  assert.deepEqual(fake.executed('declineOffer'), [
    { action: 'declineOffer', offerId: 't-2', expectedVersion: 'legacy-v1' },
    { action: 'declineOffer', offerId: 't-3', expectedVersion: 'legacy-v1' },
  ]);
  fake.emitOffers([]); await page.during(() => undefined);
  assert.ok(!page.has('Join') && !page.text().includes('Team invitations'), 'no offers, no section');
});

scenario('invitations alone are not an empty Friends list; 20 rows show at a time', async context => {
  const many = Array.from({ length: 25 }, (_, index) => offer(`t${String(index).padStart(2, '0')}`, NOW + DAY + index * HOUR, { teamName: `Team ${index}` }));
  const { fake, workspace, snapshot } = friendsFixture(f => { f.offers = many; }, { now: () => NOW });
  context.after(() => workspace.close());
  await ready(snapshot);
  const page = await mount(workspace, view(workspace, FriendsSidebar)); context.after(() => page.unmount());
  assert.ok(!page.text().includes('No friends yet'), 'there is something to show');
  assert.equal(rowsOf(page, 'Team invitations').length, 20); assert.ok(page.has('Show all (25)'));
  await page.click(page.button('Show all (25)'));
  assert.equal(rowsOf(page, 'Team invitations').length, 25);
  fake.emitOffers([]); await page.during(() => undefined);
  assert.ok(page.text().includes('No friends yet'), 'no friends, no requests and no invitations is the empty state');
});

scenario('showing the Friends root works the badge out against the clock again, and an expiry reaches the rows by itself (D28)', async context => {
  let clock = NOW;
  const { workspace, snapshot } = friendsFixture(f => { f.cards = [card('bob')]; f.offers = [offer('t-1', NOW + HOUR, { teamName: 'Alpha' })]; }, { now: () => clock });
  context.after(() => workspace.close());
  await ready(snapshot);
  const page = await mount(workspace, view(workspace, FriendsSidebar)); context.after(() => page.unmount());
  assert.equal(snapshot().badge, 1); assert.ok(textOf(rowsOf(page, 'Team invitations')[0]).includes('Expires in 1 hour'));
  clock = NOW + 2 * HOUR;
  workspace.recount(); await page.during(() => undefined);
  assert.equal(snapshot().badge, 0); assert.deepEqual(buttonsOf(rowsOf(page, 'Team invitations')[0]), ['Dismiss']);
});

// ── The friendship's Bots (D21, D22, D25) ──

/** A Bot DM created through the fake is confirmed at once: the directory gets it and every listener of the pair team is told. */
const confirmBotDms = (teams: FakeTeams) => {
  const base = teams.messages;
  teams.messages = async <T>(command: MessageCommand) => {
    if (command.action === 'createConversation') {
      const { teamId, payload } = command;
      teams.conversations.set(teamId, [...(teams.conversations.get(teamId) ?? []), conversation(payload.conversationId, { kind: 'bot_dm', title: payload.title, participants: ['user:alice', ...payload.sharedBotIds.map(id => `shared_bot:${id}`)] })]);
      for (const watch of teams.open(teamId)) teams.deliver(watch);
    }
    return base<T>(command);
  };
};
const botsChat = async (context: TestContext, setup: (teams: FakeTeams) => void = () => {}) => {
  const fixture = friendsFixture((fake, teams) => { chatSetup(fake, teams); confirmBotDms(teams); setup(teams); });
  context.after(() => fixture.workspace.close());
  await ready(fixture.snapshot);
  fixture.workspace.openChat('bob');
  const page = await mount(fixture.workspace, view(fixture.workspace, FriendsMain)); context.after(() => page.unmount());
  await until(() => !!fixture.snapshot().chat?.workspace?.getSnapshot().conversation, 'the chat'); await page.during(() => undefined);
  const pair = () => fixture.snapshot().chat!.workspace!;
  return { ...fixture, page, pair, panel: () => page.oneByClass('tw-friends-bots') };
};

scenario('the Bots action opens the friendship\'s Bots over the chat: Teams cards without an audience or quota, the note, and no sharing on Web (D21)', async context => {
  const { page, pair, panel, teams } = await botsChat(context, t => { t.bots = [botCard({ name: 'Bob Helper', description: 'Answers about trips', maintainer: { uid: 'bob', generation: 1 }, availability: 'ready' })]; });
  await until(() => pair().getSnapshot().botsLoaded, 'the Bot cards');
  assert.equal(page.byClass('tw-friends-bots').length, 0, 'closed until asked for');
  const opener = () => page.byLabel('Shared Bots').find(node => node.type === 'button')!;
  assert.equal(opener().props['aria-expanded'], false);
  await page.click(opener());
  assert.equal(opener().props['aria-expanded'], true);
  const text = textOf(panel());
  for (const expected of ['Shared Bots', 'Bob Helper', 'Answers about trips', 'Maintainer: Bob', 'Ready', 'Message Bot',
    'A Bot runs on its maintainer’s Desktop. The maintainer can read every conversation with it.', 'To share one of your Bots with this friend, open LingXi Desktop.']) assert.ok(text.includes(expected), expected);
  for (const absent of ['Daily requests', 'All current and future members', 'Find Bots in the market', 'Team Bots', 'Publish a Desktop Bot', 'Stop sharing', 'Pause']) assert.ok(!text.includes(absent), absent);
  assert.equal(teams.commands.filter(command => command.action === 'listSharedBots').length, 1, 'the cards came from the chat\'s own workspace');
  await page.click(page.byLabel('Close').find(node => node.type === 'button')!);
  assert.equal(page.byClass('tw-friends-bots').length, 0);
});

scenario('with no Bots the panel says so, and a Bot list that could not be read is not taken for an empty one', async context => {
  const { page, pair, teams } = await botsChat(context);
  await until(() => pair().getSnapshot().botsLoaded, 'the Bot cards');
  await page.click(page.oneByLabel('Shared Bots'));
  assert.ok(page.text().includes('No Bots are shared in this chat yet.')); assert.ok(page.text().includes('A Bot runs on its maintainer’s Desktop.'), 'the note is always there');
  await page.click(page.byLabel('Close').find(node => node.type === 'button')!);
  teams.messages = async () => { throw new Error('SERVICE_UNAVAILABLE'); };
  await page.during(() => pair().refreshBots());
  await page.click(page.oneByLabel('Shared Bots'));
  assert.ok(page.text().includes('Bot availability could not be verified.')); assert.ok(!page.text().includes('No Bots are shared in this chat yet.'));
  assert.ok(!page.text().includes('This chat is no longer available.'), 'a failure that is not the end of the friendship leaves the chat alone');
});

scenario('Message opens a Bot DM in the plain Teams view with a way back; the friend\'s header and menu belong to the friend chat alone (D22)', async context => {
  const { page, pair, teams } = await botsChat(context, t => { t.bots = [botCard({ name: 'Bob Helper', maintainer: { uid: 'bob', generation: 1 } })]; });
  await until(() => pair().getSnapshot().botsLoaded, 'the Bot cards');
  assert.equal(page.byLabel('Back').length, 0, 'the friend chat has no Back'); assert.equal(page.byLabel('More').length, 1);
  await page.click(page.oneByLabel('Shared Bots'));
  await page.click(page.button('Message Bot'));
  await until(() => pair().getSnapshot().conversation?.kind === 'bot_dm', 'the Bot DM'); await page.during(() => undefined);
  const created = teams.commands.find(command => command.action === 'createConversation')!;
  assert.ok(created.action === 'createConversation' && created.teamId === pairTeamId('bob') && created.payload.kind === 'bot_dm' && created.payload.sharedBotIds[0] === 'assistant-fixture');
  assert.equal(page.byClass('tw-friends-bots').length, 0, 'the panel closed');
  assert.equal(page.byLabel('Back').length, 1); assert.equal(page.byLabel('More').length, 0, 'no friend menu in a Bot DM');
  assert.ok(!page.text().includes('@bobby'), 'nor the friend\'s header');
  assert.ok(page.messageEditor(), 'the composer is the Teams composer');
  assert.equal(page.byLabel('Shared Bots').length, 0, 'the Bots action is the friend chat\'s');
  await page.click(page.oneByLabel('Back'));
  assert.equal(pair().getSnapshot().selectedConversationId, 'friend_chat');
  assert.ok(page.text().includes('@bobby')); assert.equal(page.byLabel('Back').length, 0);
  // Message again finds the Bot DM that is there and creates no second one.
  await page.click(page.oneByLabel('Shared Bots')); await page.click(page.button('Message Bot'));
  await until(() => pair().getSnapshot().conversation?.kind === 'bot_dm', 'the same Bot DM');
  assert.equal(teams.commands.filter(command => command.action === 'createConversation').length, 1);
});

scenario('a Bot DM that is being created shows its banner with a way back instead of "unavailable", and a failed creation can be discarded', async context => {
  let attempts = 0;
  const { page, pair } = await botsChat(context, t => {
    t.bots = [botCard({ name: 'Bob Helper', maintainer: { uid: 'bob', generation: 1 } })];
    const base = t.messages;
    t.messages = async <U>(command: MessageCommand) => { if (command.action === 'createConversation') { attempts++; throw new Error('CONVERSATION_REJECTED'); } return base<U>(command); };
  });
  await until(() => pair().getSnapshot().botsLoaded, 'the Bot cards');
  await page.click(page.oneByLabel('Shared Bots')); await page.click(page.button('Message Bot'));
  await until(() => !!pair().getSnapshot().creation?.error, 'the rejected creation'); await page.during(() => undefined);
  assert.ok(!page.text().includes('This chat is no longer available.')); assert.ok(page.text().includes('Creating conversation…'));
  assert.equal(attempts, 1);
  await page.click(page.button('Back'));
  assert.equal(pair().getSnapshot().selectedConversationId, 'friend_chat'); assert.ok(page.text().includes('@bobby'));
});

scenario('every Bot command that finds the friendship over ends the whole friend area, an open Bots panel included (D25)', async context => {
  for (const code of ['MEMBERSHIP_REQUIRED', 'STALE_MEMBERSHIP', 'CONVERSATION_REQUIRED']) {
    const { page, pair, teams } = await botsChat(context, t => { t.bots = [botCard({ name: 'Bob Helper', maintainer: { uid: 'bob', generation: 1 } })]; });
    await until(() => pair().getSnapshot().botsLoaded, 'the Bot cards');
    await page.click(page.oneByLabel('Shared Bots'));
    assert.equal(page.byClass('tw-friends-bots').length, 1);
    teams.messages = async () => { throw new Error(code); };
    await page.click(page.button('Message Bot'));
    await until(() => page.text().includes('This chat is no longer available.'), `the end (${code})`);
    assert.equal(page.byClass('tw-friends-bots').length, 0, 'the panel is gone with the chat'); assert.equal(page.byLabel('Shared Bots').length, 0);
    await page.click(page.button('Back'));
  }
  const { page, pair, teams } = await botsChat(context, t => { t.bots = [botCard()]; });
  await until(() => pair().getSnapshot().botsLoaded, 'the Bot cards');
  teams.messages = async () => { throw new Error('MEMBERSHIP_REQUIRED'); };
  await page.during(() => pair().refreshBots());
  assert.ok(page.text().includes('This chat is no longer available.'), 'listing the Bots ends it too');
});

scenario('Stop sharing: your own published Bot, behind a confirmation, as pauseSharedBot with the revision; a friendship that ended meanwhile ends the chat', async context => {
  const mine = botCard({ sharedBotId: 'my-helper-bot', name: 'My Helper', instanceId: 'local-1', maintainer: { uid, generation: 1 }, revision: 3 });
  const { page, pair, panel, teams } = await botsChat(context, t => { t.bots = [mine, botCard({ name: 'Bob Helper', maintainer: { uid: 'bob', generation: 1 } })]; });
  await until(() => pair().getSnapshot().botsLoaded, 'the Bot cards');
  await page.click(page.oneByLabel('Shared Bots'));
  assert.equal(page.find(node => node.type === 'button' && textOf(node) === 'Stop sharing').length, 1, 'only on the viewer\'s own Bot');
  await page.click(page.button('Stop sharing'));
  const dialog = () => dialogs(page)[0]!;
  assert.ok(textOf(dialog()).includes('Stop sharing “My Helper”?'), 'the title names the Bot');
  assert.ok(textOf(dialog()).includes('New requests to this Bot stop. Existing conversations stay. You can share it again from LingXi Desktop.'));
  assert.equal(dialog().props.role, 'alertdialog');
  assert.equal(buttonOf(dialog(), 'Cancel').props['data-autofocus'], true, 'the safe choice has the focus');
  assert.equal(teams.commands.some(command => command.action === 'pauseSharedBot'), false, 'asking is not doing');
  await page.click(buttonOf(dialog(), 'Cancel'));
  assert.equal(dialogs(page).length, 0); assert.equal(teams.commands.some(command => command.action === 'pauseSharedBot'), false);

  teams.bots = [botCard({ name: 'Bob Helper', maintainer: { uid: 'bob', generation: 1 } }), { ...mine, state: 'paused', revision: 4 }];
  await page.click(page.button('Stop sharing')); await page.click(buttonOf(dialog(), 'Stop sharing'));
  const pause = teams.commands.find(command => command.action === 'pauseSharedBot')!;
  assert.ok(pause.action === 'pauseSharedBot' && pause.teamId === pairTeamId('bob') && pause.generation === 1 && pause.payload.sharedBotId === 'my-helper-bot' && pause.payload.expectedRevision === 3);
  assert.equal(dialogs(page).length, 0);
  await until(() => pair().getSnapshot().bots.some(bot => bot.state === 'paused'), 'the Bot cards after stopping'); await page.during(() => undefined);
  assert.ok(textOf(panel()).includes('Sharing stopped'), 'the row shows that sharing has stopped'); assert.equal(page.find(node => node.type === 'button' && textOf(node) === 'Stop sharing').length, 0);

  teams.bots = [{ ...mine, state: 'published' }];
  await page.during(() => pair().refreshBots());
  const original = teams.messages;
  teams.messages = async <T>(command: MessageCommand) => { if (command.action === 'pauseSharedBot') throw new Error('MEMBERSHIP_REQUIRED'); return original<T>(command); };
  await page.click(page.button('Stop sharing')); await page.click(buttonOf(dialog(), 'Stop sharing'));
  await until(() => page.text().includes('This chat is no longer available.'), 'the end');
});

// ── Invite friends (D26, D30) ──

const inviteSetup = (f: FakeFriends) => {
  f.cards = [card('bob'), card('carol'), card('dave', { handle: 'dave_d' }), card('erin', { state: 'incoming' }), card('finn', { state: 'outgoing' }), card('zed', { lastSeq: 1, lastMessage: pointer(1) })];
};
const invitePage = async (context: TestContext, setup: (fake: FakeFriends) => void = inviteSetup, more: Record<string, unknown> = {}) => {
  const fixture = friendsFixture(setup);
  context.after(() => fixture.workspace.close());
  if (!fixture.fake.quiet) await ready(fixture.snapshot);
  const result = { closed: undefined as string[] | undefined, refreshed: 0 };
  const members = [{ uid }, { uid: 'bob' }];
  const page = await mount(fixture.workspace, view(fixture.workspace, InviteFriendsDialog, {
    team: { id: 'team-aaaa', name: 'Crew' }, members, refreshMembers: () => { result.refreshed++; }, onClose: (names: string[]) => { result.closed = names; }, ...more,
  }));
  context.after(() => page.unmount());
  const rows = (): any[] => page.byLabel('Friends')[0]?.findAll((node: any) => node.type === 'li') ?? [];
  const row = (name: string) => rows().find(item => textOf(item.findAll((node: any) => node.type === 'b')[0]) === name)!;
  return { ...fixture, page, result, rows, row, names: () => rows().map(item => textOf(item.findAll((node: any) => node.type === 'b')[0])) };
};

scenario('Invite friends lists the friends who are not in the team, in the Friends order, and never a request; the filter narrows it', async context => {
  const { page, names } = await invitePage(context);
  const dialog = () => dialogs(page)[0]!;
  assert.ok(textOf(dialog()).includes('Invite friends to “Crew”'));
  assert.deepEqual(names(), ['Zed', 'Carol', 'Dave'], 'newest message first, then by name; Bob is a member and the requests are not friends');
  assert.ok(textOf(dialog()).includes('@dave_d'));
  assert.ok(textOf(dialog()).includes('Your friend gets an invitation they can accept or decline. They see the team’s name and your name, and nothing else is shared until they join. Invitations expire after 7 days.'));
  const filter = page.oneByLabel('Search friends');
  await page.type(filter, 'DAV'); assert.deepEqual(names(), ['Dave']);
  await page.type(filter, '@dave_d'); assert.deepEqual(names(), ['Dave']);
  await page.type(filter, 'carol'); assert.deepEqual(names(), ['Carol']);
  await page.type(filter, 'erin'); assert.deepEqual(names(), [], 'a request is not listed even when it matches');
  assert.ok(page.text().includes('No matching friends'));
  await page.type(filter, ''); assert.equal(names().length, 3);
});

scenario('nobody to invite says so, and the list waits for the friends or says it could not load them', async context => {
  const none = await invitePage(context, f => { f.cards = [card('bob'), card('erin', { state: 'incoming' })]; });
  assert.ok(none.page.text().includes('No friends to invite. Friends who are already in this team are not listed.'));
  assert.equal(none.rows().length, 0);

  const loading = await invitePage(context, f => { f.quiet = true; });
  assert.ok(loading.page.text().includes('Loading friends…'));
  loading.fake.failCards('SERVICE_UNAVAILABLE'); await loading.page.during(() => undefined);
  assert.ok(loading.page.text().includes("Couldn't load your friends. Check your connection and try again."));
  loading.fake.quiet = false;
  await loading.page.click(loading.page.button('Retry'));
  await until(() => !loading.snapshot().loading, 'the retry'); await loading.page.during(() => undefined);
  assert.ok(loading.page.text().includes('No friends to invite.'));
});

scenario('Invite sends an offer for that row, which then says Invited until the dialog closes; closing reports who was invited', async context => {
  const { fake, page, row, result } = await invitePage(context);
  const invite = (name: string) => buttonOf(row(name), 'Invite');
  await page.click(invite('Carol'));
  const [sent] = fake.executed('offerTeam') as { teamId: string; friendUid: string; requestId: string }[];
  assert.deepEqual([sent!.teamId, sent!.friendUid], ['team-aaaa', 'carol']); assert.ok(sent!.requestId);
  assert.deepEqual(buttonsOf(row('Carol')), ['Invited']); assert.equal(buttonOf(row('Carol'), 'Invited').props.disabled, true);
  assert.deepEqual(buttonsOf(row('Dave')), ['Invite']); assert.equal(invite('Dave').props.disabled, false, 'the others are not disabled by it');
  await page.click(invite('Dave'));
  assert.equal(result.closed, undefined, 'the dialog stays open for more');
  await page.click(page.button('Close'));
  assert.deepEqual(result.closed, ['Carol', 'Dave']);
});

scenario('a failed Invite is told under its row and can be tried again; an unknown outcome is retried with the same request id', async context => {
  const { fake, page, row } = await invitePage(context);
  fake.fail('offerTeam', 'OFFER_LIMIT');
  await page.click(buttonOf(row('Carol'), 'Invite'));
  assert.ok(textOf(row('Carol')).includes(offerErrorText('offerTeam', 'OFFER_LIMIT', 'en'))); assert.ok(!textOf(row('Dave')).includes('20 invitations'));
  assert.deepEqual(buttonsOf(row('Carol')), ['Invite'], 'not invited');
  fake.fail('offerTeam', 'SERVICE_UNAVAILABLE', 503);
  await page.click(buttonOf(row('Dave'), 'Invite')); await page.click(buttonOf(row('Dave'), 'Invite'));
  const [, first, second] = fake.executed('offerTeam') as { requestId: string }[];
  assert.equal(first!.requestId, second!.requestId);
  assert.ok(textOf(row('Dave')).includes(offerErrorText('offerTeam', 'SERVICE_UNAVAILABLE', 'en')));
  fake.replies.delete('offerTeam');
  await page.click(buttonOf(row('Dave'), 'Invite'));
  assert.deepEqual(buttonsOf(row('Dave')), ['Invited']); assert.ok(!textOf(row('Dave')).includes(offerErrorText('offerTeam', 'SERVICE_UNAVAILABLE', 'en')));
});

scenario('ALREADY_MEMBER drops the row and has the host refresh its members; FRIEND_REQUIRED leaves the list through the cards listener; both are still told', async context => {
  const { fake, page, row, names, result } = await invitePage(context);
  fake.fail('offerTeam', 'ALREADY_MEMBER');
  await page.click(buttonOf(row('Carol'), 'Invite'));
  assert.deepEqual(names(), ['Zed', 'Dave']); assert.equal(result.refreshed, 1);
  assert.ok(page.text().includes('They are already in this team.'), 'the row is gone but what happened is not silent');

  fake.fail('offerTeam', 'FRIEND_REQUIRED');
  await page.click(buttonOf(row('Dave'), 'Invite'));
  assert.ok(textOf(row('Dave')).includes('You can only invite people who are still your friends.'));
  fake.emitCards(fake.cards.filter(value => value.uid !== 'dave')); await page.during(() => undefined);
  assert.deepEqual(names(), ['Zed']); assert.ok(page.text().includes('You can only invite people who are still your friends.'));
  assert.equal(result.refreshed, 1, 'only ALREADY_MEMBER means the member list is stale');
});

scenario('the member list the host holds decides who is offered, and a person who joins meanwhile leaves the list', async context => {
  const { page, names, workspace } = await invitePage(context);
  assert.deepEqual(names(), ['Zed', 'Carol', 'Dave']);
  await page.unmount();
  const grown = await mount(workspace, view(workspace, InviteFriendsDialog, { team: { id: 'team-aaaa', name: 'Crew' }, members: [{ uid }, { uid: 'bob' }, { uid: 'carol' }], refreshMembers() {}, onClose() {} }));
  context.after(() => grown.unmount());
  assert.deepEqual(rowsOf(grown, 'Friends').map(item => textOf(item.findAll((node: any) => node.type === 'b')[0])), ['Zed', 'Dave']);
});

scenario('the notice after the picker names who was invited, in the person\'s language, and goes away by itself', async context => {
  const { create, act } = reactTestRenderer();
  context.mock.timers.enable({ apis: ['setTimeout'] });
  let done = 0, tree: any;
  const invited = ['Carol', 'Dave'];
  const notice = (locale: string, onDone: () => void) => createElement(InviteNotice as never, { names: invited, locale, onDone } as never);
  await act(() => { tree = create(notice('en', () => { done++; })); });
  const text = () => textOf({ children: [].concat(tree.toJSON()).map(function walk(node: any): any { return typeof node === 'string' ? node : { children: (node.children ?? []).map(walk) }; }) });
  assert.equal(text(), 'Invitation sent to Carol, Dave');
  context.mock.timers.tick(3000);
  await act(() => { tree.update(notice('zh-Hans', () => { done += 10; })); });
  assert.equal(text(), '已向 Carol、Dave 发送邀请');
  context.mock.timers.tick(2999); assert.equal(done, 0, 'a new callback does not restart the six seconds');
  context.mock.timers.tick(1); assert.equal(done, 10, 'and the newest callback is the one called');
  await act(() => { tree.unmount(); });
});

scenario('the Invites tab has the Invite friends button for an owner or an admin only, and only when the host offers it', async context => {
  const roles: [string, boolean][] = [['owner', true], ['admin', true], ['member', false]];
  for (const [role, offered] of roles) {
    const teamsFake = new FakeTeams();
    teamsFake.teams = [team('team-aaaa', { role: role as 'owner' })];
    const workspace = new TeamsWorkspace(teamsFake.api, uid, { storage: memoryStorage, environment: new EventTarget(), refreshDelay: 5 });
    context.after(() => workspace.close());
    await until(() => workspace.getSnapshot().detail?.members.length === 3, 'the team');
    let invited = 0;
    const page = await mount(workspace, snapshot => createElement(TeamSettingsPage as never, { workspace, snapshot, locale: 'en', initialSection: 'invites', onClose() {}, onOpenChat() {}, onInviteFriends: () => { invited++; } } as never));
    context.after(() => page.unmount());
    const button = page.find(node => node.type === 'button' && textOf(node) === 'Invite friends');
    assert.equal(button.length, offered ? 1 : 0, role);
    if (offered) {
      assert.ok(page.text().indexOf('Invite friends') < page.text().indexOf('Create invite link'), 'above Create invite link');
      await page.click(button[0]!); assert.equal(invited, 1);
    }
    const without = await mount(workspace, snapshot => createElement(TeamSettingsPage as never, { workspace, snapshot, locale: 'en', initialSection: 'invites', onClose() {}, onOpenChat() {} } as never));
    context.after(() => without.unmount());
    assert.equal(without.find(node => node.type === 'button' && textOf(node) === 'Invite friends').length, 0, `${role}: a host that has no Friends offers nothing`);
  }
});
