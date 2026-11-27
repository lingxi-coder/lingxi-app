import assert from 'node:assert/strict';
import test from 'node:test';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import vectors from '../../../packages/teams/contracts/execution-vectors.json';
import { TeamsWorkspace } from '../../../packages/teams/workspace';
import type { SharedBotCard, TaskDelegation, TeamTaskView, TriageLogItem } from '../../../packages/teams/execution';
import type { MessageCommand } from '../../../packages/teams/messages';
import { FakeTeams, botCard, conversation, memoryStorage, team, until } from './support/fakeTeams';
import { loadTeamsModule } from './support/loadTeamsModule';
import { mount, scenario, withDom } from './support/workspacePage';

const delegation = loadTeamsModule<typeof import('../../../packages/teams/delegation')>('delegation');
const log = loadTeamsModule<typeof import('../../../packages/teams/triage-log')>('triage-log');
const { BotRun } = loadTeamsModule<typeof import('../../../packages/teams/BotRunCards')>('BotRunCards');
const { TeamConversationView } = loadTeamsModule<typeof import('../../../packages/teams/TeamConversationView')>('TeamConversationView');
const { BotMemberDetails } = loadTeamsModule<typeof import('../../../packages/teams/BotMemberDetails')>('BotMemberDetails');
const { teamsText } = loadTeamsModule<typeof import('../../../packages/teams/i18n')>('i18n');
const { isMemberExecutionCommand } = loadTeamsModule<typeof import('../../../packages/teams/execution-validation')>('execution-validation');

// Bot v3 B8b: what the run card, the Bot's card and the auto-reply log say of Bots asking Bots. The service decides (B8a); these say it to the person.

const ALICE = { uid: 'alice', generation: 1 }, BOB = { uid: 'bob', generation: 1 }, CAROL = { uid: 'carol', generation: 1 }, DAN = { uid: 'dan', generation: 1 };
const SCOPE = { teamId: 'team-test', generation: 1 };
/** Alice asked Planner; Planner (Bob's) asked Travel (Carol's); Dan only reads. */
const planner = botCard({ sharedBotId: 'sharedbot_planner', name: 'Planner', maintainer: BOB });
const travel = botCard({ sharedBotId: 'sharedbot_travel', name: 'Travel', maintainer: CAROL });
const bots: SharedBotCard[] = [planner, travel];
const link = (bot: SharedBotCard) => ({ sharedBotId: bot.sharedBotId, name: bot.name });
const delegated = (more: Partial<TaskDelegation> = {}): TaskDelegation => ({
  delegationId: 'delegation-1', rootTaskId: 'task-root', parentTaskId: 'task-root', parentBotId: planner.sharedBotId, depth: 1, chain: [link(planner), link(travel)], expectReply: true, ...more,
});
const task = (more: Partial<TeamTaskView> = {}): TeamTaskView => ({
  taskId: 'task-child', conversationId: 'chat', sharedBotId: travel.sharedBotId, requester: ALICE, createdAt: 1, deadline: 2, state: 'running', delegation: delegated(), ...more,
} as TeamTaskView);
/** The card of a task as `viewer` sees it, in words. */
function draw(value: TeamTaskView, viewer: { uid: string }, options: { locale?: string; cards?: SharedBotCard[]; tasks?: TeamTaskView[] } = {}) {
  const snapshot = { uid: viewer.uid, bots: options.cards ?? bots, directory: [{ uid: 'alice', displayName: 'Alice' }, { uid: 'bob', displayName: 'Bob' }], pending: [] } as never;
  return renderToStaticMarkup(createElement(BotRun, { task: value, api: new FakeTeams(viewer.uid).api, scope: SCOPE, locale: options.locale ?? 'en', snapshot, tasks: options.tasks })).replace(/<!-- -->/g, '');
}
const buttons = (markup: string) => [...markup.matchAll(/<button[^>]*>([^<]*)<\/button>/g)].map(match => match[1]);

// ───────────────────────────── the chain, "A → B" ─────────────────────────────

test('a task another Bot asked for says whose work it is part of: the Bots from the one asked to the one that runs it; a task nobody delegated says nothing', () => {
  assert.equal(delegation.delegationChain(task(), bots, 'Bot'), 'Planner → Travel');
  assert.equal(delegation.delegationChain(task({ delegation: undefined }), bots, 'Bot'), undefined, 'a task nobody delegated');
  assert.equal(delegation.delegationChain(task({ delegation: delegated({ chain: [link(travel)] }) }), bots, 'Bot'), undefined, 'a chain of one Bot is no chain');
  const three = delegated({ depth: 2, chain: [link(planner), link(travel), { sharedBotId: 'sharedbot_chef', name: 'Chef' }] });
  assert.equal(delegation.delegationChain(task({ delegation: three }), bots, 'Bot'), 'Planner → Travel → Chef', 'at most two hops, so at most three Bots');
  // A Bot is named by the card the Team lists now; the name it had when it was asked is for a Bot that has no card; an id is never a name.
  const renamed = bots.map(bot => bot.sharedBotId === travel.sharedBotId ? { ...bot, name: 'Travel 2' } : bot);
  assert.equal(delegation.delegationChain(task(), renamed, 'Bot'), 'Planner → Travel 2');
  assert.equal(delegation.delegationChain(task(), [], 'Bot'), 'Planner → Travel', 'no card of either: the names of the chain');
  assert.equal(delegation.delegationChain(task({ delegation: delegated({ chain: [{ sharedBotId: 'sharedbot_planner', name: 'sharedbot_planner' }, link(travel)] }) }), [], 'Bot'), 'Bot → Travel', 'a handoff-made chain names a Bot by its id: the Bot, not the id');
});

test('the run card of a delegated task shows the chain on its own line, in every language, and none for a task nobody delegated', () => {
  const line = (locale: string, more: Partial<TeamTaskView> = {}) => draw(task(more), ALICE, { locale }).match(/<p class="tw-run-detail tw-run-chain">([^<]*)<\/p>/)?.[1];
  assert.equal(line('en'), 'Delegated: Planner → Travel');
  assert.equal(line('zh-Hans'), '委托：Planner → Travel'); assert.equal(line('zh-Hant'), '委派：Planner → Travel');
  assert.equal(line('ja'), '委任：Planner → Travel'); assert.equal(line('ko'), '위임: Planner → Travel');
  assert.equal(line('en', { delegation: undefined }), undefined, 'a task nobody delegated has no such line');
  assert.equal(line('en', { state: 'completed' }), 'Delegated: Planner → Travel', 'it stays once the task is done');
  assert.doesNotMatch(draw(task({ delegation: undefined }), ALICE), /Delegated/);
});

// ───────────────────────────── waiting for a Bot ─────────────────────────────

const waiting = (more: Partial<NonNullable<TeamTaskView['wait']>> = {}, taskMore: Partial<TeamTaskView> = {}): TeamTaskView => task({
  taskId: 'task-root', sharedBotId: planner.sharedBotId, delegation: undefined, state: 'waiting',
  wait: { waitId: 'wait-1', kind: 'delegation', recipient: ALICE, expiresAt: Date.now() + 120_000, messageId: 'message-1', delegation: { sharedBotId: travel.sharedBotId, taskId: 'task-child' }, ...more },
  ...taskMore,
});

test('a task that waits for another Bot says which Bot, not "waiting for you": the person who asked is the recipient of the wait and has nothing to answer', () => {
  const markup = draw(waiting(), ALICE);
  assert.ok(markup.includes('<span>Waiting for a reply</span>'), 'the state'); assert.ok(markup.includes('<p class="tw-run-detail">Waiting for Travel</p>'), 'the Bot it waits for, from the card');
  assert.doesNotMatch(markup, /Waiting for you|expires/, 'it is not a question to the person, so no "you" and no answer to give');
  assert.equal(buttons(markup).includes('Reply'), false, 'nothing to reply to');
  assert.ok(draw(waiting(), ALICE, { locale: 'zh-Hans' }).includes('<p class="tw-run-detail">等待 Travel 回复</p>'));
  assert.ok(draw(waiting(), ALICE, { locale: 'zh-Hant' }).includes('<p class="tw-run-detail">等待 Travel 回覆</p>'));
  assert.ok(draw(waiting(), ALICE, { locale: 'ja' }).includes('<p class="tw-run-detail">Travel の返信待ち</p>'));
  assert.ok(draw(waiting(), ALICE, { locale: 'ko' }).includes('<p class="tw-run-detail">Travel의 답장 대기 중</p>'));
  // Whoever else reads the card reads the same line: the wait is the Bot's, not a person's.
  assert.ok(draw(waiting(), DAN).includes('<p class="tw-run-detail">Waiting for Travel</p>'));
  // A question to a person keeps its own line.
  const question = draw(waiting({ kind: 'human_input', delegation: undefined }), ALICE);
  assert.match(question, /Waiting for you · expires/); assert.doesNotMatch(question, /Waiting for Travel/);
});

test('the Bot a task waits for is named by its card, else by the name the task it asked has for it, else it is "Bot"; a task that waits for something else names none', () => {
  const asked = task({ taskId: 'task-child', delegation: delegated({ chain: [link(planner), { sharedBotId: travel.sharedBotId, name: 'Trip desk' }] }) });
  assert.equal(delegation.waitingForBot(waiting(), bots), 'Travel', 'by its card');
  assert.equal(delegation.waitingForBot(waiting(), [planner], [asked]), 'Trip desk', 'no card of it (it was unpublished, or the member cannot see it): the name it had when it was asked');
  assert.equal(delegation.waitingForBot(waiting(), [planner], [asked, task({ taskId: 'other' })]), 'Trip desk');
  assert.equal(delegation.waitingForBot(waiting(), [planner], []), undefined, 'no name to give');
  assert.equal(delegation.waitingForBot(waiting(), [planner], [task({ taskId: 'task-child', delegation: delegated({ chain: [link(planner), { sharedBotId: travel.sharedBotId, name: travel.sharedBotId }] }) })]), undefined, 'an id is not a name');
  assert.equal(delegation.waitingForBot(waiting({ kind: 'human_input', delegation: undefined }), bots), undefined);
  assert.equal(delegation.waitingForBot({ state: 'running', wait: waiting().wait }, bots), undefined, 'a wait that is over is not shown');
  assert.ok(draw(waiting(), ALICE, { cards: [planner], tasks: [asked] }).includes('<p class="tw-run-detail">Waiting for Trip desk</p>'), 'on the card too');
  assert.ok(draw(waiting(), ALICE, { cards: [planner] }).includes('<p class="tw-run-detail">Waiting for Bot</p>'), 'a Bot with no name to give is the Bot');
});

test('a task that waits for another Bot can be cancelled by the person who asked, from its card, as any other task', () => {
  assert.deepEqual(buttons(draw(waiting(), ALICE)), ['Cancel']);
});

// ───────────────────────────── who may cancel ─────────────────────────────

test('who may cancel: the person who asked, the maintainer of the Bot that runs the task and, for a delegated task, the maintainer of the Bot that asked; nobody else', () => {
  const asked = task();
  assert.equal(delegation.mayCancel(asked, ALICE, bots), true, 'the person who asked');
  assert.equal(delegation.mayCancel(asked, CAROL, bots), true, 'the maintainer of the Bot that runs it');
  assert.equal(delegation.mayCancel(asked, BOB, bots), true, 'the maintainer of the Bot that asked for it');
  assert.equal(delegation.mayCancel(asked, DAN, bots), false, 'anybody else');
  assert.equal(delegation.mayCancel(asked, { uid: 'bob', generation: 0 }, bots), false, 'the membership they held before is not the one they hold now');
  assert.equal(delegation.mayCancel(asked, BOB, [travel]), false, 'a Bot that is not in the list is nobody\'s to maintain');
  // Planner\'s maintainer may stop what Planner asked for, not what somebody else asked Travel for.
  assert.equal(delegation.mayCancel(task({ delegation: undefined }), BOB, bots), false, 'a task nobody delegated is not the business of the maintainer of a Bot that did not ask');
  assert.equal(delegation.mayCancel(task({ delegation: undefined }), CAROL, bots), true);
  assert.equal(delegation.mayCancel(task({ delegation: delegated({ parentBotId: 'sharedbot_other' }) }), BOB, bots), false, 'the Bot that asked is the one in the delegation, not any Bot of the chain');
});

test('the run card offers Cancel to exactly the people who may, for a task that is still going', () => {
  for (const [who, name, expected] of [[ALICE, 'the person who asked', ['Cancel']], [CAROL, 'the maintainer of the Bot that runs it', ['Cancel']], [BOB, 'the maintainer of the Bot that asked', ['Cancel']], [DAN, 'a member with no part in it', []]] as const) {
    assert.deepEqual(buttons(draw(task(), who)), expected, name);
  }
  assert.deepEqual(buttons(draw(task({ delegation: undefined }), BOB)), [], 'a task nobody delegated: the maintainer of a Bot that did not run it has nothing to cancel');
  for (const state of ['completed', 'failed', 'cancelled', 'declined', 'expired'] as const) assert.deepEqual(buttons(draw(task({ state }), BOB)), [], `${state}: nothing is going on`);
  for (const state of ['queued', 'running', 'waiting', 'awaiting_approval'] as const) assert.deepEqual(buttons(draw(task({ state }), BOB)), ['Cancel'], `${state}: it can still be stopped`);
});

// ───────────────────────────── a target that is offline ─────────────────────────────

test('a delegated task whose Bot is offline says so in the Chat, with the chain: it answers when the Desktop is back', () => {
  const markup = draw(task({ state: 'queued', hostWait: true }), ALICE);
  assert.ok(markup.includes('<p class="tw-run-detail">Travel is offline and will reply when it’s back online</p>'), 'the line a task sent while the Desktop was off has always had');
  assert.ok(markup.includes('<p class="tw-run-detail tw-run-chain">Delegated: Planner → Travel</p>'), 'and whose work it is');
  assert.deepEqual(buttons(markup), ['Cancel']);
});

// ───────────────────────────── in a Chat ─────────────────────────────

scenario('In a Chat the two tasks of a delegation are two cards: the one that waits names the Bot it waits for, also when the member has no card of it, and the one that was asked says whose work it is and that its Bot is offline', async context => {
  withDom(context);
  const fake = new FakeTeams('alice'); fake.teams = [team('team-test', { role: 'owner' })]; fake.bots = [planner]; // Travel is not in the list the Team gives this member.
  fake.conversations.set('team-test', [conversation('test-chat')]);
  const root = waiting({}, { taskId: 'task-root', conversationId: 'test-chat' });
  const child = task({ taskId: 'task-child', conversationId: 'test-chat', state: 'queued', hostWait: true });
  fake.messages = async <T>(command: MessageCommand) => (command.action === 'listTasks' ? { tasks: [root, child] } : command.action === 'listSharedBots' ? { bots: fake.bots } : {}) as T;
  const workspace = new TeamsWorkspace(fake.api, 'alice', { storage: memoryStorage, environment: new EventTarget() }); context.after(() => workspace.close());
  await until(() => workspace.getSnapshot().botsLoaded && !!workspace.getSnapshot().detail, 'the Team');
  const page = await mount(workspace, snapshot => createElement(TeamConversationView, { workspace, snapshot, locale: 'en' })); context.after(() => page.unmount());
  await until(() => page.text().includes('Waiting for Travel'), 'the waiting card');
  assert.match(page.text(), /Waiting for Travel/, 'the name is the one the task it asked has in its chain');
  assert.match(page.text(), /Delegated: Planner → Travel/, 'the card of the task that was asked');
  assert.match(page.text(), /Bot is offline and will reply when it’s back online/, 'its Bot is offline: it answers when it is back');
  assert.doesNotMatch(page.text(), /Waiting for you/);
});

// ───────────────────────────── the Auto-reply log ─────────────────────────────

const row = (more: Partial<TriageLogItem> = {}): TriageLogItem => ({
  taskId: 'task-child', conversationId: 'chat', messageId: 'message-1', createdAt: 1_700_000_000_000, state: 'completed',
  triage: { action: 'run', source: 'rule', reason: 'addressed', at: 1_700_000_001_000 }, ...more,
});

test('a row of the log whose task another Bot asked for says "Delegated by" that Bot, by the name in the chain; any other row says nothing of it', () => {
  const en = teamsText('en');
  assert.equal(log.triageDelegatedText(en, delegated()), 'Delegated by Planner');
  assert.equal(log.triageDelegatedText(en, undefined), '', 'a message a person sent');
  assert.equal(log.delegatedBy(delegated(), 'Bot'), 'Planner'); assert.equal(log.delegatedBy(undefined, 'Bot'), undefined);
  assert.equal(log.delegatedBy(delegated({ chain: [{ sharedBotId: 'sharedbot_planner', name: 'sharedbot_planner' }, link(travel)] }), 'Bot'), 'Bot', 'a Bot named by its id is the Bot');
  assert.equal(log.delegatedBy(delegated({ parentBotId: 'sharedbot_gone' }), 'Bot'), 'Bot', 'a parent that is not in the chain has no name to give');
  assert.equal(log.triageDelegatedText(teamsText('zh-Hans'), delegated()), '由 Planner 委托'); assert.equal(log.triageDelegatedText(teamsText('zh-Hant'), delegated()), '由 Planner 委派');
  assert.equal(log.triageDelegatedText(teamsText('ja'), delegated()), 'Planner からの委任'); assert.equal(log.triageDelegatedText(teamsText('ko'), delegated()), '위임한 Bot: Planner');
});

test('the log reads a delegation as the service sent it; one it cannot read is left out of the row, which is still shown', () => {
  const sent = delegated();
  assert.deepEqual(log.triageItem(row({ delegation: sent }))!.delegation, sent);
  assert.equal('delegation' in log.triageItem(row())!, false);
  for (const bad of [{ ...sent, chain: [] }, { ...sent, chain: [{ sharedBotId: 'a' }] }, { ...sent, chain: 'a → b' }, { ...sent, parentBotId: 7 }, { ...sent, expectReply: 'yes' }, { ...sent, depth: 'one' }, { ...sent, delegationId: '' }, 'planner', 7, null, []]) {
    const item = log.triageItem(row({ delegation: bad as never }));
    assert.ok(item, `the row is still read: ${JSON.stringify(bad)}`); assert.equal('delegation' in item!, false, JSON.stringify(bad));
  }
});

// ───────────────────────────── the Bot's card ─────────────────────────────

const details = (bot: SharedBotCard, locale = 'en', list: SharedBotCard[] = bots) => renderToStaticMarkup(createElement(BotMemberDetails, { bot, bots: list, locale })).replace(/<!-- -->/g, '');
const lineOf = (markup: string) => markup.match(/<p class="teams-hint tw-bot-delegation">([^<]*)<\/p>/)?.[1];

test('a published Bot\'s card says whom it takes requests from and whom it may ask: read only, by name', () => {
  assert.equal(lineOf(details(botCard({ sharedBotId: 'sharedbot_desk' }))), 'Accepts delegation from: No Bots', 'a Bot published before delegation takes none');
  assert.equal(lineOf(details(botCard({ acceptDelegation: 'none' }))), 'Accepts delegation from: No Bots');
  assert.equal(lineOf(details(botCard({ acceptDelegation: 'team_bots' }))), 'Accepts delegation from: Any Bot of the Team');
  assert.equal(lineOf(details(botCard({ acceptDelegation: 'listed', acceptFrom: [planner.sharedBotId, travel.sharedBotId] }))), 'Accepts delegation from: Planner, Travel');
  assert.equal(lineOf(details(botCard({ acceptDelegation: 'listed', acceptFrom: ['sharedbot_gone', planner.sharedBotId] }))), 'Accepts delegation from: Planner', 'a Bot that is gone from the Team is not listed');
  assert.equal(lineOf(details(botCard({ acceptDelegation: 'listed', acceptFrom: ['sharedbot_gone'] }))), 'Accepts delegation from: No Bots', 'a list of Bots that are all gone takes nobody');
  assert.equal(lineOf(details(botCard({ acceptDelegation: 'listed' }))), 'Accepts delegation from: No Bots', 'a list that is not there is no one');
  assert.equal(lineOf(details(botCard({ acceptDelegation: 'team_bots', delegateTo: [travel.sharedBotId] }))), 'Accepts delegation from: Any Bot of the Team · May delegate to: Travel');
  assert.equal(lineOf(details(botCard({ delegateTo: ['sharedbot_gone', planner.sharedBotId, travel.sharedBotId] }))), 'Accepts delegation from: No Bots · May delegate to: Planner, Travel');
  assert.equal(lineOf(details(botCard({ delegateTo: ['sharedbot_gone'] }))), 'Accepts delegation from: No Bots', 'nothing it may ask is nothing to say');
  assert.doesNotMatch(details(botCard({ delegation: { daily: 9 }, delegateTo: [travel.sharedBotId] })), /\b9\b/, 'the limit of its maintainer is not for a member');
  assert.equal(lineOf(details(botCard({ state: 'paused', acceptDelegation: 'team_bots' }))), undefined, 'a Bot that is not published is not asked');
  assert.doesNotMatch(details(botCard({ acceptDelegation: 'team_bots' })), /<button|<select|<input/, 'read only: it is chosen where the Bot is published');
});

test('the Bot\'s card says it in every language, with the words of the language', () => {
  const asked = botCard({ acceptDelegation: 'listed', acceptFrom: [planner.sharedBotId], delegateTo: [travel.sharedBotId] });
  assert.equal(lineOf(details(asked, 'zh-Hans')), '接受谁的委托: Planner · 可委托给: Travel'); assert.equal(lineOf(details(asked, 'zh-Hant')), '接受誰的委派: Planner · 可委派給: Travel');
  assert.equal(lineOf(details(asked, 'ja')), '受け付ける委任元: Planner · 委任できる相手: Travel'); assert.equal(lineOf(details(asked, 'ko')), '위임을 받는 Bot: Planner · 위임할 수 있는 Bot: Travel');
  const none = botCard();
  assert.equal(lineOf(details(none, 'zh-Hans')), '接受谁的委托: 不接受任何 Bot'); assert.equal(lineOf(details(none, 'zh-Hant')), '接受誰的委派: 不接受任何 Bot');
  assert.equal(lineOf(details(none, 'ja')), '受け付ける委任元: なし'); assert.equal(lineOf(details(none, 'ko')), '위임을 받는 Bot: 없음');
  assert.equal(lineOf(details(botCard({ acceptDelegation: 'team_bots' }), 'zh-Hant')), '接受誰的委派: 團隊中的任何 Bot');
});

test('the delegation words are in the shared catalog in every language, and the Traditional Chinese is not the Simplified', () => {
  const keys = ['teams_task_waiting_bot_fmt', 'teams_task_delegation_chain_fmt', 'teams_triage_delegated_by_fmt', 'teams_bot_accepts_delegation', 'teams_bot_accepts_none', 'teams_bot_accepts_team', 'teams_bot_delegates_to'] as const;
  for (const key of keys) {
    const seen = new Map<string, string>();
    for (const locale of ['en', 'zh-Hans', 'zh-Hant', 'ja', 'ko']) {
      const text = teamsText(locale)(key, 'A');
      assert.ok(text && text !== key, `${locale}: ${key} has words`);
      if (locale !== 'en') assert.notEqual(text, teamsText('en')(key, 'A'), `${locale}: ${key} is translated`);
      seen.set(locale, text);
    }
    if (key !== 'teams_bot_accepts_none') assert.notEqual(seen.get('zh-Hant'), seen.get('zh-Hans'), `${key}: Taiwan says 委派, the mainland 委托`);
  }
  for (const text of ['en', 'zh-Hans', 'zh-Hant', 'ja', 'ko'].flatMap(locale => keys.map(key => teamsText(locale)(key, 'A')))) assert.doesNotMatch(text, /assistant|助手|助理|アシスタント|어시스턴트/i, 'a Bot is a Bot');
});

test('what only the service sends is refused from a member by every client: the two vectors Bot v3 B8a added', () => {
  const names = ['reject-delegateTask-member', 'reject-detachDelegation-member'];
  for (const name of names) {
    const vector = vectors.commands.find(entry => entry.name === name);
    assert.ok(vector, `${name} is in the contract`); assert.equal(vector!.valid, false, name); assert.equal(isMemberExecutionCommand(vector!.input), false, `${name}: a member cannot send it`);
  }
});
