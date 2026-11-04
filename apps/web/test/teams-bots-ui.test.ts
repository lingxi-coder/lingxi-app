import assert from 'node:assert/strict';
import { type TestContext } from 'node:test';
import { createElement } from 'react';
import type { TeamsApi, TeamDetail } from '../../../packages/teams';
import { teamBotTriggerId, type TeamBotCard, type TeamBotState } from '../../../packages/teams/bots';
import type { MessageCommand, TeamMessage } from '../../../packages/teams/messages';
import { TeamsWorkspace, type TeamsSnapshot } from '../../../packages/teams/workspace';
import { FakeTeams, botCard, conversation, memoryStorage, message, team as teamSummary, until } from './support/fakeTeams';
import { loadTeamsModule } from './support/loadTeamsModule';
import { classes, mount, scenario, textOf, withDom } from './support/workspacePage';

// The Bot cards of the Teams workspace, driven through the same views the apps show: the question and the run a
// Bot's message carries in the conversation, and the Bots tab of the details drawer.
const { TeamConversationView } = loadTeamsModule<typeof import('../../../packages/teams/TeamConversationView')>('TeamConversationView');
const { TeamDetailsDrawer } = loadTeamsModule<typeof import('../../../packages/teams/TeamDetailsDrawer')>('TeamDetailsDrawer');
const { TeamSidebar } = loadTeamsModule<typeof import('../../../packages/teams/TeamSidebar')>('TeamSidebar');

const botKey = `bot:owner:${'h'.repeat(22)}:${'b'.repeat(22)}`;
const team = teamSummary('test-team', { name: 'Fixture team', generation: 7, role: 'member' });
const members = ['alice', 'owner', 'reader'].map(uid => ({ uid, displayName: uid, generation: 7, role: 'member' as const }));
const detail: TeamDetail = { team, members, invites: [] };
const question: TeamMessage = { id: 'question-message', seq: 1, author: { kind: 'bot', uid: 'owner', botKey }, kind: 'question', text: 'Choose a value',
  triggerId: 'trigger01', origin: { uid: 'alice', generation: 7 }, question: { id: 'question01', questions: ['Which value?'] }, createdAt: null, changeSeq: 1, changeId: 'change001' };

/** A team with one Bot, opened by `viewer`. `extra` adds what only the Desktop's API has (local Bots). */
async function open(context: TestContext, viewer: string, setup: (fake: FakeTeams) => Partial<TeamsApi> | void) {
  withDom(context);
  const fake = new FakeTeams(viewer);
  fake.teams = [team];
  fake.details.set(team.id, detail);
  const api: TeamsApi = { ...fake.api, ...setup(fake) };
  const workspace = new TeamsWorkspace(api, viewer, { storage: memoryStorage, environment: new EventTarget(), refreshDelay: 5 });
  context.after(() => workspace.close());
  const snapshot = (): TeamsSnapshot => workspace.getSnapshot();
  await until(() => snapshot().team?.id === team.id && snapshot().botsLoaded, 'the team to load');
  return { fake, api, workspace, snapshot };
}
const commandsOf = (fake: FakeTeams, ...actions: string[]) => fake.commands.filter(command => actions.includes(command.action));
/** What the three views of the workspace show together; the sidebar only when a test needs its dialogs. */
const views = (workspace: TeamsWorkspace, withSidebar = false) => (snapshot: TeamsSnapshot) => createElement('div', null,
  withSidebar ? createElement(TeamSidebar, { workspace, snapshot, zh: false } as never) : null,
  createElement(TeamConversationView, { workspace, snapshot, zh: false } as never),
  createElement(TeamDetailsDrawer, { workspace, snapshot, zh: false, tab: 'bots', onClose() {} } as never));

for (const viewer of ['alice', 'reader']) scenario(`a Bot's question gives answer controls only to its original requester (${viewer})`, async context => {
  const { fake, workspace, snapshot } = await open(context, viewer, fake => {
    fake.bots = [botCard({ key: botKey, ownerUid: 'owner', use: 'listed', uids: ['alice'], permitted: viewer === 'alice', online: false })];
    fake.conversations.set(team.id, [conversation('test-chat', { kind: 'group', title: 'Shared thread', participants: ['user:alice', 'user:reader', botKey], lastSeq: 1, lastReadSeq: 0,
      lastMessage: { id: question.id, seq: 1, at: null, author: question.author, preview: 'Choose a value' } })]);
    fake.threads.set('test-chat', [question]);
    const settled = fake.messages;
    fake.messages = async <T>(command: MessageCommand) => command.action === 'botStates'
      ? (fake.commands.push(structuredClone(command)), { states: Object.fromEntries(command.triggerIds.map(id => [id, { state: 'running', ownerUid: 'owner', botKey, origin: question.origin,
        questionId: question.question!.id, leaseUntil: Date.now() + 100000, answered: false }])) } as T)
      : settled<T>(command);
  });
  workspace.selectConversation('test-chat'); await until(() => snapshot().messages.length === 1, 'the thread');
  const page = await mount(workspace, views(workspace)); context.after(() => page.unmount());
  // The directory: who owns the Bot, whether it is reachable, and whether this person may message it.
  const card = page.oneByClass('team-bot-grid');
  assert.match(textOf(card), /Offline/); assert.match(textOf(card), /owner/);
  assert.equal(Boolean(card.findByType('button').props.disabled), viewer !== 'alice');
  assert.match(page.text(), /Open Teams in LingXi Desktop/, 'an API without local Bots points to the Desktop for sharing');
  // The question card in the conversation.
  const prompt = () => page.oneByClass('team-bot-question');
  if (viewer === 'alice') {
    const answer = prompt().findByType('textarea'); assert.equal(Boolean(answer.props.disabled), false);
    await page.type(answer, '42');
    await page.submit(prompt().findByType('form'));
    await until(() => commandsOf(fake, 'botAnswer').length === 1, 'the answer');
    await page.during(() => undefined);
    assert.match(textOf(prompt()), /Answer received/);
  } else {
    assert.equal(prompt().findAllByType('textarea').length, 0); assert.equal(prompt().findAllByType('form').length, 0);
    assert.match(textOf(prompt()), /Only the original requester/);
  }
  assert.equal(commandsOf(fake, 'claimBot', 'startBot', 'finishBot', 'botPoll').length, 0, 'a member can read and answer, never run or approve');
});

for (const viewer of ['owner', 'alice']) scenario(`a Bot run card says where a request stands; only the Bot's owner sees the Desktop's status and can settle an interrupted run (${viewer})`, async context => {
  const ask = message(1, { id: 'ask-message', author: { kind: 'user', uid: 'alice' }, text: '@Fixture Bot summarize', mentions: [botKey] });
  const triggerId = await teamBotTriggerId(ask.id, botKey);
  const state: TeamBotState = { state: 'recovery', ownerUid: 'owner', botKey, origin: { uid: 'alice', generation: 7 }, answered: false, hostStatus: 'recovery_required' };
  const { fake, workspace, snapshot } = await open(context, viewer, fake => {
    fake.bots = [botCard({ key: botKey, ownerUid: 'owner' })];
    fake.conversations.set(team.id, [conversation('run-chat', { participants: ['user:alice', 'user:owner', botKey], lastSeq: 1, lastReadSeq: 1,
      lastMessage: { id: ask.id, seq: 1, at: null, author: ask.author, preview: ask.text } })]);
    fake.threads.set('run-chat', [ask]);
    const base = fake.messages;
    fake.messages = async <T>(command: MessageCommand) => command.action === 'botStates' || command.action === 'botState'
      ? (fake.commands.push(structuredClone(command)), (command.action === 'botStates' ? { states: { [triggerId]: state } } : state) as T)
      : base<T>(command);
  });
  workspace.selectConversation('run-chat'); await until(() => snapshot().messages.length === 1, 'the thread');
  const page = await mount(workspace, snapshot => createElement(TeamConversationView, { workspace, snapshot, zh: false } as never)); context.after(() => page.unmount());
  const card = () => page.oneByClass('team-bot-run');
  assert.match(textOf(card()), /Bot · Owner recovery required/);
  if (viewer === 'alice') {
    assert.doesNotMatch(textOf(card()), /Desktop stopped/, 'the Desktop\'s status is for the owner');
    assert.equal(card().findAllByType('form').length, 0); assert.equal(card().findAllByType('textarea').length, 0);
    assert.equal(commandsOf(fake, 'recoverBot').length, 0);
    return;
  }
  assert.match(textOf(card()), /Desktop stopped this interrupted run/);
  assert.match(textOf(card()), /Resolve interrupted request/);
  const settle = () => card().findByType('form').findAllByType('button').find((node: any) => textOf(node) === 'Settle request')!;
  assert.equal(settle().props.disabled, true, 'nothing to settle with yet');
  await page.type(card().findByType('textarea'), 'Done by hand'); assert.equal(settle().props.disabled, true, 'the owner confirms first');
  await page.check(card().findByType('form').findAllByType('input').find((node: any) => node.props.type === 'checkbox')!);
  assert.equal(settle().props.disabled, false);
  await page.submit(card().findByType('form'));
  await until(() => commandsOf(fake, 'recoverBot').length === 1, 'the settlement'); await page.during(() => undefined);
  assert.equal(commandsOf(fake, 'botState').length, 1, 'a new settlement checks the run and its lease first');
  assert.deepEqual(commandsOf(fake, 'recoverBot')[0], { action: 'recoverBot', teamId: team.id, generation: 7, conversationId: 'run-chat', triggerId, botKey, text: 'Done by hand' });
  assert.match(textOf(card()), /Bot · Recovered by owner/); assert.equal(card().findAllByType('form').length, 0);
});

scenario('an owner unbind keeps the tombstone revision for rebind, and the Bot is no longer a way to start or continue a conversation', async context => {
  let card: TeamBotCard = botCard({ key: botKey, ownerUid: 'owner', revision: 7, use: 'all', permitted: true, online: true, enabled: true });
  const binds: { expectedRevision: number }[] = [];
  const { fake, workspace, snapshot } = await open(context, 'owner', fake => {
    fake.conversations.set(team.id, [conversation('test-chat', { kind: 'bot_dm', title: 'Existing Bot thread', participants: ['user:owner', botKey] })]);
    fake.messages = async <T>(command: MessageCommand) => {
      fake.commands.push(structuredClone(command));
      if (command.action === 'bots') return { bots: [{ ...card }] } as T;
      if (command.action === 'unbindBot') { assert.equal(command.expectedRevision, 7); card = { ...card, revision: 8, enabled: false, permitted: false }; }
      return {} as T;
    };
    return {
      async localBots() { return [{ botId: card.botId, name: card.name, description: '' }]; },
      async bindLocalBot(input) { binds.push(input); card = { ...card, revision: 9, enabled: true, permitted: true }; return { botKey, revision: 9 }; },
    };
  });
  workspace.selectConversation('test-chat'); await until(() => snapshot().selectedConversationId === 'test-chat', 'the conversation');
  const page = await mount(workspace, views(workspace, true)); context.after(() => page.unmount());
  assert.equal(page.find(node => node.type === 'textarea').length, 1, 'while the Bot is usable the Bot DM takes messages');
  await page.click(page.button('Unbind'));
  await page.click(page.oneByClass('teams-modal').findAll((node: any) => node.type === 'button' && textOf(node) === 'Unbind')[0]);
  await until(() => card.revision === 8 && snapshot().bots[0]?.enabled === false, 'the unbind and the reload of the cards');
  await page.during(() => undefined);
  const grid = page.oneByClass('team-bot-grid');
  assert.match(textOf(grid), /Disabled/); assert.doesNotMatch(textOf(grid), /Online|All team members may use/); assert.equal(page.has('Unbind'), false);
  const disabled = grid.findByType('button'); assert.equal(disabled.props.disabled, true);
  // The conversation with it is read-only and says why; nothing else offers the Bot.
  assert.equal(page.find(node => node.type === 'textarea').length, 0);
  assert.match(textOf(page.oneByClass('tw-readonly')), /This Bot is not available to you right now/);
  await page.during(() => disabled.props.onClick());
  assert.equal(commandsOf(fake, 'create', 'mutate').length, 0, 'even a press that gets past the disabled button starts nothing');
  await page.click(page.oneByLabel('New channel'));
  const dialog = page.find(node => node.type === 'dialog')[0];
  assert.match(textOf(dialog), /New channel/); assert.deepEqual(dialog.findAllByType('legend').map(textOf), ['People'], 'a disabled Bot cannot be added to a channel');
  await page.click(dialog.findAll((node: any) => node.type === 'button' && textOf(node) === 'Cancel')[0]);
  // Rebinding continues from the tombstone's revision.
  await page.click(page.button('Load local Bots'));
  const directory = page.oneByClass('team-bots');
  await page.during(() => directory.findAllByType('select')[0].props.onChange({ target: { value: card.botId } }));
  assert.ok(page.has('Rebind Bot'));
  await page.submit(directory.findByType('form'));
  await until(() => binds.length === 1, 'the rebind');
  assert.equal(binds[0]!.expectedRevision, 8);
  await until(() => snapshot().bots[0]?.enabled === true, 'the reload of the cards'); await page.during(() => undefined);
  assert.equal(Boolean(page.button('Message Bot').props.disabled), false);
  assert.equal(page.find(node => node.type === 'textarea').length, 1, 'the conversation takes messages again');
});

scenario('two owner windows keep their audience draft revision when the cards refresh, and a stale regrant is refused', async context => {
  let card: TeamBotCard = botCard({ key: botKey, ownerUid: 'owner', revision: 7, use: 'listed', uids: ['alice'], permitted: true, online: true, enabled: true });
  const revisions: number[] = [];
  const { fake, api, workspace: first, snapshot: firstSnapshot } = await open(context, 'owner', fake => {
    fake.messages = async <T>(command: MessageCommand) => (command.action === 'bots' ? { bots: [structuredClone(card)] } : {}) as T;
    return {
      async localBots() { return [{ botId: card.botId, name: card.name, description: '' }]; },
      async bindLocalBot(input) {
        revisions.push(input.expectedRevision);
        if (input.expectedRevision !== card.revision) throw new Error('BOT_REVISION_CONFLICT');
        card = { ...card, revision: card.revision + 1, use: input.use, uids: [...input.uids] }; return { botKey, revision: card.revision };
      },
    };
  });
  const second = new TeamsWorkspace(api, 'owner', { storage: memoryStorage, environment: new EventTarget(), refreshDelay: 5 });
  context.after(() => second.close());
  await until(() => second.getSnapshot().botsLoaded, 'the second window');
  const windows = [await mount(first, views(first)), await mount(second, views(second))];
  context.after(async () => { for (const page of windows) await page.unmount(); });
  const directory = (page: (typeof windows)[number]) => page.oneByClass('team-bots');
  const alice = (page: (typeof windows)[number]) => directory(page).findAllByType('label').find((node: any) => textOf(node) === 'alice')!.findByType('input');
  for (const page of windows) {
    await page.click(page.button('Load local Bots'));
    await page.during(() => directory(page).findAllByType('select')[0].props.onChange({ target: { value: card.botId } }));
  }
  assert.equal(alice(windows[0]!).props.checked, true);
  // Window B takes Alice out and saves.
  await windows[1]!.check(alice(windows[1]!), false);
  await windows[1]!.submit(directory(windows[1]!).findByType('form'));
  await until(() => card.revision === 8, 'the first save'); assert.deepEqual(card.uids, []);
  // Window A's cards refresh to revision 8, while its loaded permission draft still contains Alice.
  fake.signal();
  await until(() => firstSnapshot().bots[0]?.revision === 8, 'the refreshed cards'); await windows[0]!.during(() => undefined);
  assert.equal(alice(windows[0]!).props.checked, true, 'a refresh does not rewrite what the owner was editing');
  await windows[0]!.submit(directory(windows[0]!).findByType('form'));
  await until(() => revisions.length === 2, 'the stale save');
  assert.deepEqual(revisions, [7, 7]); assert.equal(card.revision, 8); assert.deepEqual(card.uids, []);
  await windows[0]!.during(() => undefined);
  assert.match(textOf(windows[0]!.find(node => node.props.role === 'alert' && classes(node).includes('tw-error'))[0]), /Bot settings changed/);
  // Reloading the permissions takes the saved audience and the saved revision; saving then goes through.
  await windows[0]!.click(windows[0]!.button('Reload current permissions'));
  assert.equal(alice(windows[0]!).props.checked, false);
  await windows[0]!.submit(directory(windows[0]!).findByType('form'));
  await until(() => revisions.length === 3, 'the reloaded save');
  assert.deepEqual(revisions, [7, 7, 8]); assert.equal(card.revision, 9); assert.deepEqual(card.uids, []);
});
