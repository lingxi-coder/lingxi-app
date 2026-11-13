import assert from 'node:assert/strict';
import { type TestContext } from 'node:test';
import { createElement } from 'react';
import type { MessageCommand, MessageMutation } from '../../../packages/teams/messages';
import { TeamsWorkspace, type TeamsSnapshot } from '../../../packages/teams/workspace';
import { FakeTeams, botCard, conversation, memoryStorage, message, team, uid, until } from './support/fakeTeams';
import { loadTeamsModule } from './support/loadTeamsModule';
import { classes, mount, scenario, textOf, withDom } from './support/workspacePage';

// These tests drive the components' event handlers and effects through react-test-renderer (see support/workspacePage).
const { TeamConversationView } = loadTeamsModule<typeof import('../../../packages/teams/TeamConversationView')>('TeamConversationView');
const { TeamDetailsDrawer } = loadTeamsModule<typeof import('../../../packages/teams/TeamDetailsDrawer')>('TeamDetailsDrawer');
const { TeamSidebar } = loadTeamsModule<typeof import('../../../packages/teams/TeamSidebar')>('TeamSidebar');

// ── Fixture ──

const at = (minute: number) => new Date(2026, 10, 2, 11, minute).toISOString();
async function open(context: TestContext) {
  withDom(context);
  const fake = new FakeTeams();
  const bot = botCard();
  fake.teams = [team('team-aaaa', { name: 'Acme', role: 'owner' }), team('team-bbbb', { name: 'Beta', generation: 2 })];
  fake.bots = [bot];
  fake.conversations.set('team-aaaa', [
    conversation('chan-general', { title: 'general', participants: ['user:alice', 'user:bob', `shared_bot:${bot.sharedBotId}`], lastSeq: 3, lastReadSeq: 1, lastMessage: { id: 'message-3', seq: 3, at: at(3), preview: 'Three', author: { kind: 'user', uid: 'bob' } } }),
    conversation('dm-bob', { kind: 'dm', title: 'Alice, Bob', lastSeq: 0, lastReadSeq: 0 }),
  ]);
  fake.conversations.set('team-bbbb', [conversation('chan-b', { lastSeq: 2, lastReadSeq: 0 })]);
  fake.threads.set('chan-general', [
    message(1, { text: 'One', createdAt: at(1) }),
    message(2, { text: 'Mine', author: { kind: 'user', uid: 'alice' }, createdAt: at(2) }),
    message(3, { text: 'Three', createdAt: at(3) }),
  ]);
  const workspace = new TeamsWorkspace(fake.api, uid, { storage: memoryStorage, environment: new EventTarget(), refreshDelay: 5 });
  context.after(() => workspace.close());
  const snapshot = (): TeamsSnapshot => workspace.getSnapshot();
  await until(() => snapshot().conversations.length === 2 && snapshot().botsLoaded, 'the team to load');
  return { fake, workspace, snapshot, bot };
}


const conversationView = (workspace: TeamsWorkspace, more: Record<string, unknown> = {}) => (snapshot: TeamsSnapshot) => createElement(TeamConversationView, { workspace, snapshot, zh: false, now: new Date(2026, 10, 2, 12).getTime(), ...more } as never);
const mutations = (fake: FakeTeams): MessageMutation[] => fake.commands.filter((command): command is MessageMutation => command.action === 'mutate');
const commandsOf = (fake: FakeTeams, action: MessageCommand['action']) => fake.commands.filter(command => command.action === action);

// ── Composer ──

scenario('composer: Enter sends and clears, Shift+Enter and an input method\'s Enter do not, and the unsent text is kept per conversation', async context => {
  const { fake, workspace, snapshot } = await open(context);
  workspace.selectConversation('chan-general'); await until(() => snapshot().messages.length === 3, 'the thread');
  const page = await mount(workspace, conversationView(workspace)); context.after(() => page.unmount());
  const box = () => page.find(node => node.type === 'textarea')[0];
  assert.equal(box().props.value, '');
  await page.type(box(), 'Hello team');
  assert.equal(box().props.value, 'Hello team'); assert.equal(workspace.draft('chan-general'), 'Hello team', 'the draft is kept by the workspace');
  assert.equal(await page.key(box(), 'Enter', { shiftKey: true }), false, 'Shift+Enter is a line break'); assert.equal(mutations(fake).length, 0);
  assert.equal(await page.key(box(), 'Enter', { nativeEvent: { isComposing: true }, keyCode: 229 }), false, 'Enter inside an input method is not a send'); assert.equal(mutations(fake).length, 0);
  assert.equal(await page.key(box(), 'Enter'), true);
  await until(() => mutations(fake).length === 1, 'the send');
  const sent = mutations(fake)[0]!;
  assert.deepEqual([sent.operation, sent.conversationId, sent.text, 'targets' in sent, sent.replyTo], ['create', 'chan-general', 'Hello team', false, undefined]);
  assert.equal(box().props.value, '', 'the box is cleared once the message is queued'); assert.equal(workspace.draft('chan-general'), '');
  assert.equal(await page.key(box(), 'Enter'), true, 'Enter on an empty box is swallowed, not sent'); assert.equal(mutations(fake).length, 1);
  await page.type(box(), 'x'.repeat(8193));
  assert.equal(page.oneByClass('tw-send').props.disabled, true, 'over the byte limit'); assert.ok(page.byClass('tw-count').length === 1);
});

scenario('composer: @ offers the Bots of the conversation, only a chosen Bot is asked, and the text it replaced is removed', async context => {
  const { fake, workspace, snapshot, bot } = await open(context);
  workspace.selectConversation('chan-general'); await until(() => snapshot().messages.length === 3, 'the thread');
  const page = await mount(workspace, conversationView(workspace)); context.after(() => page.unmount());
  const box = () => page.find(node => node.type === 'textarea')[0];
  assert.equal(page.byClass('tw-mentions').length, 0);
  await page.type(box(), 'hey @Fix', 8);
  const options = page.find(node => node.props.role === 'option');
  assert.deepEqual(options.map(textOf), ['FBFixture BotOnline'], 'the usable Bot of this channel matches the query (initials, name, presence)');
  assert.equal(box().props['aria-activedescendant'], options[0].props.id);
  assert.equal(await page.key(box(), 'Escape'), true, 'Escape closes the list'); assert.equal(page.byClass('tw-mentions').length, 0);
  await page.type(box(), 'hey @Fi', 7); assert.equal(page.byClass('tw-mentions').length, 1, 'typing again reopens it');
  assert.equal(await page.key(box(), 'Enter'), true); assert.equal(mutations(fake).length, 0, 'Enter picks the Bot instead of sending');
  assert.equal(box().props.value, 'hey ', 'the @ query is replaced by a chip'); assert.deepEqual(page.byClass('tw-target').map(textOf), ['@Fixture Bot']);
  await page.type(box(), 'hey please look');
  await page.key(box(), 'Enter'); await until(() => commandsOf(fake, 'submit').length === 1, 'the send');
  const submitted = commandsOf(fake, 'submit')[0]!; assert.equal(submitted.action, 'submit'); if (submitted.action !== 'submit') return; assert.deepEqual(submitted.payload.targets, [bot.sharedBotId]); assert.equal(submitted.payload.text, 'hey please look');
  assert.equal(page.byClass('tw-target').length, 0, 'targets are per message');
  await page.type(box(), 'just @Fixture Bot typed'); await page.key(box(), 'Enter'); await until(() => mutations(fake).length === 1, 'a second send');
  assert.equal('targets' in mutations(fake)[0]!, false, 'typing a name never asks a Bot');
});

scenario('a failed Bot request can be retried from its message after the host returns', async context => {
  const { fake, workspace, snapshot, bot } = await open(context);
  workspace.selectConversation('chan-general'); await until(() => snapshot().messages.length === 3);
  const page = await mount(workspace, conversationView(workspace)); context.after(() => page.unmount());
  const original = fake.messages; let ready = false; const submits: MessageCommand[] = [];
  fake.messages = async <T>(command: MessageCommand) => {
    if (command.action === 'submit') { submits.push(structuredClone(command)); if (!ready) throw new Error('HOST_UNAVAILABLE'); return {} as T; }
    return original<T>(command);
  };
  await snapshot().messaging!.queue('create', 'Please help', undefined, undefined, [bot.sharedBotId]);
  await until(() => snapshot().pending[0]?.error === 'HOST_UNAVAILABLE');
  await until(() => page.has('Retry'));
  ready = true; await page.click(page.button('Retry'));
  await until(() => submits.length === 2 && snapshot().pending.length === 0);
  assert.deepEqual(submits[1], submits[0]);
});

scenario('replying, editing and recalling go through the same queue as sending', async context => {
  const { fake, workspace, snapshot } = await open(context);
  workspace.selectConversation('chan-general'); await until(() => snapshot().messages.length === 3, 'the thread');
  const page = await mount(workspace, conversationView(workspace)); context.after(() => page.unmount());
  const box = () => page.find(node => node.type === 'textarea')[0];
  await page.type(box(), 'a draft');
  // Reply to message 3.
  const reply = page.find(node => node.props['aria-label'] === 'Reply'); assert.equal(reply.length, 3);
  await page.click(reply[2]);
  assert.match(page.byClass('tw-context').map(textOf).join(), /Replying to.*Bob.*Three/);
  await page.key(box(), 'Enter'); await until(() => mutations(fake).length === 1, 'the reply');
  assert.equal(mutations(fake)[0]!.replyTo, 'message-3'); assert.equal(mutations(fake)[0]!.text, 'a draft'); assert.equal(page.byClass('tw-context').length, 0, 'the reply bar goes with the send');
  // Edit message 2 (the only one that is the viewer\'s). The draft is set aside and comes back.
  await page.type(box(), 'second draft');
  await page.click(page.oneByLabel('Edit'));
  assert.equal(box().props.value, 'Mine'); assert.match(page.byClass('tw-context').map(textOf).join(), /Editing message/);
  assert.equal(page.byClass('tw-mentions').length, 0); assert.equal(page.byLabel('Ask a Bot (@)').length, 0, 'an edit asks no Bots');
  await page.type(box(), 'Mine, corrected');
  await page.key(box(), 'Enter'); await until(() => mutations(fake).length === 2, 'the edit');
  assert.deepEqual([mutations(fake)[1]!.operation, mutations(fake)[1]!.messageId, mutations(fake)[1]!.text], ['edit', 'message-2', 'Mine, corrected']);
  assert.equal(box().props.value, 'second draft', 'the draft returns after the edit');
  // Escape abandons an edit.
  await page.click(page.oneByLabel('Edit')); assert.equal(box().props.value, 'Mine');
  await page.key(box(), 'Escape'); assert.equal(box().props.value, 'second draft'); assert.equal(page.byClass('tw-context').length, 0);
  // Up arrow in an empty box edits the newest own message.
  await page.type(box(), ''); await page.key(box(), 'ArrowUp'); assert.equal(box().props.value, 'Mine');
  await page.key(box(), 'Escape');
  // Recall asks first.
  await page.click(page.oneByLabel('Recall'));
  const dialog = page.find(node => node.type === 'dialog')[0]; assert.equal(dialog.props.role, 'alertdialog');
  assert.match(textOf(dialog), /Recall this message for everyone\?/); assert.equal(mutations(fake).length, 2);
  await page.click(page.button('Cancel')); assert.equal(page.find(node => node.type === 'dialog').length, 0); assert.equal(mutations(fake).length, 2);
  await page.click(page.oneByLabel('Recall')); await page.click(page.button('Recall'));
  await until(() => mutations(fake).length === 3, 'the recall');
  assert.deepEqual([mutations(fake)[2]!.operation, mutations(fake)[2]!.messageId], ['recall', 'message-2']); assert.equal(page.find(node => node.type === 'dialog').length, 0);
});

scenario('marking read goes to the server and clears the unread divider and the button', async context => {
  const { fake, workspace, snapshot } = await open(context);
  workspace.selectConversation('chan-general'); await until(() => snapshot().messages.length === 3, 'the thread');
  const page = await mount(workspace, conversationView(workspace)); context.after(() => page.unmount());
  assert.equal(page.find(node => node.props['data-unread-divider'] !== undefined).length, 1);
  await page.click(page.button('Mark as read'));
  await until(() => snapshot().conversation?.lastReadSeq === 3, 'the read mark');
  assert.deepEqual(commandsOf(fake, 'read').map(command => (command as { lastReadSeq: number }).lastReadSeq), [3]);
  assert.equal(page.has('Mark as read'), false); assert.equal(page.find(node => node.props['data-unread-divider'] !== undefined).length, 0);
});

// ── Sidebar ──

scenario('sidebar: choosing a row opens it, the team menu switches teams, the row menu marks read', async context => {
  const { fake, workspace, snapshot } = await open(context);
  const chosen: string[] = [];
  const page = await mount(workspace, snapshot => createElement(TeamSidebar, { workspace, snapshot, zh: false, now: new Date(2026, 10, 2, 12).getTime(), onConversationSelected: (id: string) => chosen.push(id) } as never)); context.after(() => page.unmount());
  const row = (id: string) => page.find(node => node.props['data-row'] === id)[0];
  await page.click(row('dm-bob'));
  assert.equal(snapshot().selectedConversationId, 'dm-bob'); assert.deepEqual(chosen, ['dm-bob']);
  assert.equal(row('dm-bob').props['aria-current'], 'page'); assert.equal(row('dm-bob').props.tabIndex, 0);
  // The row menu.
  await page.click(page.oneByLabel('Conversation actions'));
  const menu = page.find(node => node.props.role === 'menu'); assert.equal(menu.length, 1);
  await page.click(page.find(node => node.props.role === 'menuitem' && textOf(node) === 'Mark as read')[0]);
  await until(() => commandsOf(fake, 'read').length === 1, 'the read command');
  assert.equal((commandsOf(fake, 'read')[0] as { conversationId: string }).conversationId, 'chan-general', 'the menu\'s own row, not the open one');
  assert.equal(page.find(node => node.props.role === 'menu').length, 0, 'choosing closes the menu');
  // The team menu.
  const switcher = page.oneByClass('tw-switcher');
  await page.click(switcher); assert.equal(switcher.props['aria-expanded'], true);
  const teamItems = page.find(node => node.props.role === 'menuitemradio'); assert.deepEqual(teamItems.map(node => node.props['aria-checked']), [true, false]);
  await page.click(teamItems[1]);
  await until(() => snapshot().selectedTeamId === 'team-bbbb', 'team B');
  assert.equal(page.find(node => node.props.role === 'menu').length, 0);
  await until(() => snapshot().conversations.some(value => value.id === 'chan-b'), 'its conversations');
  assert.ok(page.find(node => node.props['data-row'] === 'chan-b').length === 1);
});

scenario('sidebar: the team menu is the way into the details panel, and the sidebar has no button of its own for it', async context => {
  const { workspace } = await open(context);
  const opened: Array<string | undefined> = [];
  const page = await mount(workspace, snapshot => createElement(TeamSidebar, { workspace, snapshot, zh: false, onOpenSettings: (tab?: string) => { opened.push(tab); } } as never)); context.after(() => page.unmount());
  assert.equal(page.byLabel('Team members & settings').length, 0, 'the conversation header has the panel\'s button; the sidebar does not repeat it');
  await page.click(page.oneByClass('tw-switcher'));
  const entry = page.find(node => node.props.role === 'menuitem' && textOf(node) === 'Team members & settings');
  assert.equal(entry.length, 1, 'the team menu offers the panel, so it can be reached where no conversation (and no header) is open');
  await page.click(entry[0]);
  assert.deepEqual(opened, [undefined], 'it opens on the tab it was last on');
  assert.equal(page.find(node => node.props.role === 'menu').length, 0, 'choosing closes the menu');
});

scenario('sidebar: arrow keys move between rows; the + buttons open dialogs that create things', async context => {
  const { fake, workspace, snapshot } = await open(context);
  const page = await mount(workspace, snapshot => createElement(TeamSidebar, { workspace, snapshot, zh: false } as never)); context.after(() => page.unmount());
  // Keyboard movement uses the DOM; give the container the rows to move between.
  const focused: string[] = [];
  const rows = ['chan-general', 'dm-bob'].map(id => ({ id, focus: () => focused.push(id), matches: () => true, props: {} }));
  const container = { querySelectorAll: () => rows };
  const sections = page.oneByClass('tw-sections');
  let prevented = false;
  await page.during(() => sections.props.onKeyDown({ key: 'ArrowDown', target: rows[0], currentTarget: container, preventDefault() { prevented = true; } }));
  assert.deepEqual(focused, ['dm-bob']); assert.equal(prevented, true);
  // A channel.
  await page.click(page.oneByLabel('New channel'));
  const dialog = () => page.find(node => node.type === 'dialog')[0];
  assert.match(textOf(dialog()), /New channel/); assert.equal(page.button('Create channel').props.disabled, true, 'a channel needs a name and someone in it');
  const name = page.find(node => node.type === 'input' && node.props.maxLength === 120)[0];
  await page.type(name, 'planning');
  assert.equal(page.button('Create channel').props.disabled, true, 'a name alone is not enough');
  await page.check(page.find(node => node.type === 'input' && node.props.type === 'checkbox')[0]);
  assert.equal(page.button('Create channel').props.disabled, false);
  await page.submit();
  await until(() => commandsOf(fake, 'create').length === 1, 'the create command');
  const created = commandsOf(fake, 'create')[0] as { kind: string; title: string; participantUids: string[] };
  assert.deepEqual([created.kind, created.title, created.participantUids], ['channel', 'planning', ['alice', 'bob']]);
  await until(() => page.find(node => node.type === 'dialog').length === 0, 'the dialog to close');
  // A person you already talk to: the existing dm opens, no second one is made.
  const radios = () => page.find(node => node.type === 'input' && node.props.type === 'radio' && node.props.name === 'tw-people');
  await page.click(page.oneByLabel('New message'));
  assert.match(textOf(dialog()), /New message/); assert.equal(page.button('Open message').props.disabled, true);
  await page.check(radios()[0]);
  await page.submit();
  await until(() => snapshot().selectedConversationId === 'dm-bob', 'the existing dm');
  assert.equal(commandsOf(fake, 'create').length, 1); await until(() => page.find(node => node.type === 'dialog').length === 0, 'the dialog to close');
  // A person you do not: a new dm with exactly the two of you.
  await page.click(page.oneByLabel('New message'));
  await page.check(radios()[1]);
  await page.submit();
  await until(() => commandsOf(fake, 'create').length === 2, 'the new dm');
  const dm = commandsOf(fake, 'create')[1] as { kind: string; participantUids: string[] };
  assert.deepEqual([dm.kind, dm.participantUids], ['dm', ['alice', 'owner']]);
});

scenario('sidebar: an invitation link opens the join dialog without joining; a bad code cannot be confirmed', async context => {
  const { fake, workspace, snapshot } = await open(context);
  let handled = 0;
  const code = 'B'.repeat(32);
  const page = await mount(workspace, snapshot => createElement(TeamSidebar, { workspace, snapshot, zh: false, initialInvite: code, onInviteHandled: () => { handled++; } } as never)); context.after(() => page.unmount());
  assert.equal(handled, 1); assert.equal(fake.teamCommands.length, 0, 'opening a link never joins');
  const dialog = page.find(node => node.type === 'dialog')[0]; assert.match(textOf(dialog), /Join a team/);
  const field = page.find(node => node.type === 'input')[0]; assert.equal(field.props.value, code);
  await page.type(field, 'not an invite'); assert.equal(page.button('Confirm and join').props.disabled, true);
  await page.type(field, `https://lingxios.net/open/team/join?code=${code}`); assert.equal(page.button('Confirm and join').props.disabled, false);
  fake.execute = async command => { fake.teamCommands.push(command); fake.invalidate(); return { teamId: 'team-aaaa', generation: 1, alreadyMember: true }; };
  fake.list = () => new Promise(() => {});
  await page.submit();
  await until(() => fake.teamCommands.length === 1, 'the join');
  assert.equal(fake.teamCommands[0]!.action, 'join'); assert.equal((fake.teamCommands[0] as { code: string }).code, code);
  await until(() => page.find(node => node.type === 'dialog').length === 0, 'the dialog to close');
  assert.equal(snapshot().detail?.members.find(member => member.uid === uid)?.generation, 1);
  assert.equal(snapshot().detail?.team.id, 'team-aaaa');
  assert.equal(snapshot().loading, true);
  assert.match(page.text(), /Loading teams/);
});

// ── Drawer ──

scenario('drawer: tabs follow the arrow keys, Escape closes, the handle resizes, and leaving or deleting asks first', async context => {
  const { fake, workspace, snapshot } = await open(context);
  await until(() => !!snapshot().detail, 'detail');
  const events: string[] = [];
  const widths: number[] = [];
  const page = await mount(workspace, snapshot => createElement(TeamDetailsDrawer, { workspace, snapshot, zh: false, onClose: () => events.push('close'), onTabChange: (tab: string) => events.push(`tab:${tab}`), onWidthChange: (width: number) => widths.push(width) } as never)); context.after(() => page.unmount());
  const aside = () => page.find(node => node.type === 'aside')[0];
  const tablist = page.oneByClass('tw-tabs');
  assert.deepEqual(page.find(node => node.props.role === 'tab').map(node => node.props['aria-selected']), [true, false, false]);
  await page.key(tablist, 'ArrowRight'); assert.deepEqual(events, ['tab:invites']);
  assert.deepEqual(page.find(node => node.props.role === 'tab').map(node => node.props['aria-selected']), [false, true, false]); assert.ok(page.has('Create invite link'));
  await page.key(tablist, 'End'); assert.deepEqual(events.slice(-1), ['tab:bots']); await page.key(tablist, 'ArrowRight'); assert.deepEqual(events.slice(-1), ['tab:members'], 'tabs wrap');
  // Resize.
  const handle = page.find(node => node.props.role === 'separator')[0];
  assert.equal(aside().props.style.width, 360);
  await page.key(handle, 'ArrowLeft'); assert.deepEqual(widths, [376]); assert.equal(aside().props.style.width, 376);
  await page.key(handle, 'End'); await page.key(handle, 'ArrowLeft'); assert.deepEqual(widths, [376, 480], 'the limit is not passed, and a change to nothing is not reported');
  await page.key(handle, 'Home'); assert.equal(aside().props.style.width, 280);
  const pointer = (clientX: number) => ({ clientX, pointerId: 1, currentTarget: { setPointerCapture() {}, releasePointerCapture() {} } });
  await page.during(() => { handle.props.onPointerDown(pointer(800)); handle.props.onPointerMove(pointer(700)); handle.props.onPointerUp(pointer(700)); handle.props.onPointerMove(pointer(0)); });
  assert.equal(aside().props.style.width, 380, 'dragging left by 100 widens by 100; after release the pointer does nothing');
  // Escape closes, but not from under a confirmation.
  await page.key(aside(), 'Escape'); assert.deepEqual(events.slice(-1), ['close']);
  events.length = 0;
  await page.click(page.button('Delete team'));
  const dialog = page.find(node => node.type === 'dialog')[0]; assert.equal(dialog.props.role, 'alertdialog'); assert.match(textOf(dialog), /Delete Acme for everyone\?/);
  await page.key(aside(), 'Escape'); assert.deepEqual(events, [], 'Escape belongs to the confirmation');
  assert.equal(fake.teamCommands.length, 0, 'nothing happens until it is confirmed');
  await page.click(page.button('Cancel')); assert.equal(page.find(node => node.type === 'dialog').length, 0);
  await page.click(page.button('Delete team'));
  await page.click(page.find(node => node.type === 'dialog')[0].findAll((node: any) => node.type === 'button' && textOf(node) === 'Delete team')[0]);
  await until(() => fake.teamCommands.length === 1, 'the delete');
  assert.deepEqual(fake.teamCommands[0], { action: 'delete', teamId: 'team-aaaa' }); assert.deepEqual(events, ['close'], 'the drawer was about the team that is gone');
});

scenario('drawer: the owner gives the team to a member after reading what does and does not change (M3-3)', async context => {
  const { fake, workspace, snapshot } = await open(context);
  await until(() => !!snapshot().detail, 'detail');
  const page = await mount(workspace, snapshot => createElement(TeamDetailsDrawer, { workspace, snapshot, zh: false, onClose() {} } as never)); context.after(() => page.unmount());
  const offers = page.find(node => node.type === 'button' && textOf(node) === 'Make owner');
  assert.equal(offers.length, snapshot().detail!.members.length - 1, 'every other member can be given the team, and the owner cannot give it to themselves');
  await page.click(offers[0]);
  const dialog = page.find(node => node.type === 'dialog')[0]; assert.equal(dialog.props.role, 'alertdialog');
  assert.match(textOf(dialog), /Make (Bob|Owner) the owner of Acme\?/); assert.match(textOf(dialog), /You stay in the team as an admin/); assert.match(textOf(dialog), /Each Bot stays with the member who maintains it/);
  assert.equal(fake.teamCommands.length, 0, 'nothing happens until it is confirmed');
  await page.click(dialog.findAll((node: any) => node.type === 'button' && textOf(node) === 'Make owner')[0]);
  await until(() => fake.teamCommands.length === 1, 'the handover');
  const target = snapshot().detail!.members.find(member => member.uid === (fake.teamCommands[0] as { uid: string }).uid)!;
  assert.deepEqual(fake.teamCommands[0], { action: 'handOver', teamId: 'team-aaaa', uid: target.uid, generation: target.generation });
});

scenario('drawer: members can be promoted and removed after a confirmation, and invitations are created and revoked', async context => {
  const { fake, workspace, snapshot } = await open(context);
  await until(() => !!snapshot().detail, 'detail');
  const page = await mount(workspace, snapshot => createElement(TeamDetailsDrawer, { workspace, snapshot, zh: false, onClose() {} } as never)); context.after(() => page.unmount());
  await page.click(page.find(node => node.type === 'button' && textOf(node) === 'Make admin')[0]);
  assert.match(textOf(page.find(node => node.type === 'dialog')[0]), /Change Bob’s role\?|Change Owner’s role\?/);
  await page.click(page.button('Change role'));
  await until(() => fake.teamCommands.length === 1, 'the role command');
  assert.equal(fake.teamCommands[0]!.action, 'role'); assert.equal((fake.teamCommands[0] as { role: string }).role, 'admin');
  await page.click(page.find(node => node.type === 'button' && textOf(node) === 'Remove' && classes(node).includes('tw-danger-btn'))[0]);
  assert.match(textOf(page.find(node => node.type === 'dialog')[0]), /Remove (Bob|Owner)\?/);
  await page.click(page.find(node => node.type === 'dialog')[0].findAll((node: any) => node.type === 'button' && textOf(node) === 'Remove')[0]);
  await until(() => fake.teamCommands.length === 2, 'the remove command'); assert.equal(fake.teamCommands[1]!.action, 'remove');
  fake.execute = async command => { fake.teamCommands.push(command); return command.action === 'invite' ? { code: 'C'.repeat(32) } : {}; };
  await page.click(page.find(node => node.props.role === 'tab' && textOf(node) === 'Invites')[0]);
  const uses = page.find(node => node.type === 'input' && node.props.type === 'number')[0];
  await page.during(() => uses.props.onChange({ target: { value: '5' } }));
  await page.during(() => page.find(node => node.type === 'form')[0].props.onSubmit({ preventDefault() {} }));
  await until(() => fake.teamCommands.length === 3, 'the invite command');
  const invite = fake.teamCommands[2] as { role: string; maxUses: number; ttlSeconds: number };
  assert.deepEqual([invite.role, invite.maxUses, invite.ttlSeconds], ['member', 5, 86400]);
  await until(() => !!snapshot().inviteLink, 'the link');
  assert.equal(page.find(node => node.type === 'input' && node.props.readOnly === true)[0].props.value, `https://lingxios.net/open/team/join?code=${'C'.repeat(32)}`);
});

scenario('the conversation view toggles the details drawer through its header button', async context => {
  const { workspace, snapshot } = await open(context);
  workspace.selectConversation('chan-general'); await until(() => snapshot().messages.length === 3, 'the thread');
  let toggled = 0;
  const page = await mount(workspace, conversationView(workspace, { detailsOpen: true, onToggleDetails: () => { toggled++; } })); context.after(() => page.unmount());
  const button = page.oneByLabel('Team details'); assert.equal(button.props['aria-pressed'], true);
  await page.click(button); assert.equal(toggled, 1);
});
