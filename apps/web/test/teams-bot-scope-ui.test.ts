import assert from 'node:assert/strict';
import test from 'node:test';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { TeamsWorkspace } from '../../../packages/teams/workspace';
import type { TeamsApi } from '../../../packages/teams';
import type { BotScopeEntry, PublishedBotScope } from '../../../packages/teams/bot-scope';
import type { LocalPublishableBot, PublishLocalBotInput, SharedBotCard } from '../../../packages/teams/execution';
import type { MessageCommand, TeamConversation } from '../../../packages/teams/messages';
import { FakeTeams, botCard, conversation, memoryStorage, team, until } from './support/fakeTeams';
import { loadTeamsModule } from './support/loadTeamsModule';
import { mount, scenario, textOf, withDom } from './support/workspacePage';

const { BotDirectory } = loadTeamsModule<typeof import('../../../packages/teams/BotDirectory')>('BotDirectory');
const { BotMemberDetails } = loadTeamsModule<typeof import('../../../packages/teams/BotMemberDetails')>('BotMemberDetails');
const { TeamConversationView } = loadTeamsModule<typeof import('../../../packages/teams/TeamConversationView')>('TeamConversationView');
const { botScopeLimits, botScopeNarrowed, choiceOf, effectiveBotScope, narrowingOf } = loadTeamsModule<typeof import('../../../packages/teams/bot-scope')>('bot-scope');
const { messageErrorText } = loadTeamsModule<typeof import('../../../packages/teams/errors')>('errors');
const { teamsText } = loadTeamsModule<typeof import('../../../packages/teams/i18n')>('i18n');

const calendar = { id: 'calendar', name: 'Calendar' }, notes = { id: 'notes', name: 'Notes' };
const orders = { source: 'orders', dataset: 'orders-2026', write: true, reportReview: false }, leads = { source: 'orders', dataset: 'leads', write: false, reportReview: false };
const published: PublishedBotScope = { localApps: [calendar, notes], datasets: [orders, leads] };
/** A Bot of another member that shares Local Apps and datasets of their computer. */
const scopedBot = (more: Partial<SharedBotCard> = {}) => botCard({ scope: published, ...more });
const ASSISTANT = 'assistant-fixture';

async function open(context: Parameters<Parameters<typeof scenario>[1]>[0], overrides: Partial<TeamsApi> = {}) {
  withDom(context);
  const fake = new FakeTeams('alice'); fake.teams = [team('team-test', { role: 'owner' })]; fake.bots = [botCard({ maintainer: { uid: 'alice', generation: 1 } })];
  Object.assign(fake.api, overrides);
  const workspace = new TeamsWorkspace(fake.api, 'alice', { storage: memoryStorage, environment: new EventTarget() }); context.after(() => workspace.close());
  await until(() => workspace.getSnapshot().botsLoaded && !!workspace.getSnapshot().detail);
  return { fake, workspace };
}

interface ChatOptions { bots?: SharedBotCard[]; kind?: TeamConversation['kind']; participants?: string[]; botScopes?: Record<string, BotScopeEntry>; locale?: string }
/** A Chat of `alice` with Bots, open on screen. Commands are recorded; the errors in `failures` are thrown by the next scope commands. */
async function chat(context: Parameters<Parameters<typeof scenario>[1]>[0], options: ChatOptions = {}) {
  const { fake, workspace } = await open(context);
  fake.bots = options.bots ?? [scopedBot()];
  await workspace.refreshBots(); await until(() => workspace.getSnapshot().bots.length === fake.bots.length && fake.bots.length > 0, 'the Bots');
  const make = (botScopes?: Record<string, BotScopeEntry>) => conversation('test-chat', { kind: options.kind ?? 'group', participants: options.participants ?? ['user:alice', 'user:bob', `shared_bot:${ASSISTANT}`], ...(botScopes ? { botScopes } : {}) });
  fake.conversations.set('team-test', [make(options.botScopes)]);
  await workspace.refresh(); workspace.selectConversation('test-chat'); await until(() => workspace.getSnapshot().selectedConversationId === 'test-chat', 'selection');
  const calls: MessageCommand[] = [], failures: string[] = [];
  fake.messages = async <T>(command: MessageCommand) => {
    calls.push(command);
    if (command.action === 'listTasks') return { tasks: [] } as T;
    if (command.action === 'setConversationBotScope' && failures.length) throw new Error(failures.shift());
    return { bots: structuredClone(fake.bots) } as T;
  };
  const page = await mount(workspace, snapshot => createElement(TeamConversationView, { workspace, snapshot, locale: options.locale ?? 'en' })); context.after(() => page.unmount());
  await until(() => page.find(node => node.type === 'header').length > 0, 'the chat');
  /** What the Chat's document says now, as the live listener delivers it. */
  const hold = async (botScopes: Record<string, BotScopeEntry> | undefined) => {
    fake.conversations.set('team-test', [make(botScopes)]);
    for (const watch of fake.open('team-test')) fake.deliver(watch);
    await until(() => workspace.getSnapshot().conversations.find(item => item.id === 'test-chat')?.botScopes === undefined ? botScopes === undefined : JSON.stringify(workspace.getSnapshot().conversations.find(item => item.id === 'test-chat')?.botScopes) === JSON.stringify(botScopes), 'the Chat as it is now');
  };
  const sent = () => calls.filter((command): command is Extract<MessageCommand, { action: 'setConversationBotScope' }> => command.action === 'setConversationBotScope').map(command => command.payload);
  return { fake, workspace, page, calls, failures, hold, sent };
}
type Page = Awaited<ReturnType<typeof chat>>['page'];
const BUTTON = 'What Bots can use in this chat';
/** The checkbox of a labelled row: the label's text is what is read. */
const box = (page: Page, label: string) => { const found = page.find(node => node.type === 'input' && node.props.type === 'checkbox' && textOf(node.parent) === label); assert.equal(found.length, 1, `one checkbox "${label}", found ${found.length}`); return found[0]; };
const ticked = (page: Page, label: string) => box(page, label).props.checked === true;

// ───────────────────────────── what a Chat may use, as a function of what is published and what is ticked ─────────────────────────────

test('what is ticked becomes the narrowing: a kind with nothing left out is not narrowed, everything allowed is no narrowing, and only what is published is named', () => {
  const all = choiceOf(published);
  assert.deepEqual(all, { localApps: ['calendar', 'notes'], datasets: ['orders-2026', 'leads'] });
  assert.equal(narrowingOf(published, all), null, 'everything allowed lifts the limit');
  assert.deepEqual(narrowingOf(published, { localApps: ['calendar'], datasets: all.datasets }), { localApps: ['calendar'] }, 'datasets are not narrowed, so what is published of them later is allowed too');
  assert.deepEqual(narrowingOf(published, { localApps: all.localApps, datasets: ['leads'] }), { datasets: [{ source: 'orders', dataset: 'leads' }] });
  assert.deepEqual(narrowingOf(published, { localApps: [], datasets: [] }), { localApps: [], datasets: [] }, 'nothing allowed is said as empty lists, not as no narrowing');
  assert.deepEqual(narrowingOf(published, { localApps: ['notes', 'mail'], datasets: ['orders-2026', 'removed', 'leads'] }), { localApps: ['notes'] }, 'what is not published is never named');
  assert.deepEqual(narrowingOf({ localApps: [calendar], datasets: [] }, { localApps: [], datasets: [] }), { localApps: [] }, 'a kind with nothing published is left out');
  assert.equal(narrowingOf({ localApps: [], datasets: [] }, { localApps: [], datasets: [] }), null);
  // Whatever is ticked is what the service then lets the Bot use here.
  for (const apps of [[], ['calendar'], ['notes'], ['calendar', 'notes']]) for (const data of [[], ['orders-2026'], ['leads'], ['orders-2026', 'leads']]) {
    const effective = effectiveBotScope(published, narrowingOf(published, { localApps: apps, datasets: data }) ?? undefined);
    assert.deepEqual(choiceOf(effective), { localApps: published.localApps.map(app => app.id).filter(id => apps.includes(id)), datasets: published.datasets.map(row => row.dataset).filter(id => data.includes(id)) });
  }
  assert.equal(botScopeNarrowed(published, effectiveBotScope(published, undefined)), false);
  assert.equal(botScopeNarrowed(published, effectiveBotScope(published, { localApps: ['notes'] })), true);
  assert.equal(botScopeNarrowed(undefined, effectiveBotScope(undefined, undefined)), false);
  assert.deepEqual([undefined, { revision: 2 }, { revision: 2, localApps: [] }, { revision: 1, datasets: [] }].map(entry => botScopeLimits(entry)), [false, false, true, true], 'a limit that was lifted keeps only its revision');
});

// ───────────────────────────── what a Bot may use is shown with the Bot ─────────────────────────────

scenario('A Bot card shows the names of the Local Apps and datasets its maintainer shared, a dataset it may edit says so, and a Bot that shares none says that', async context => {
  const { fake, workspace } = await open(context);
  fake.bots = [scopedBot({ name: 'Sales Bot' }), botCard({ sharedBotId: 'plain-bot', name: 'Plain Bot' })];
  await workspace.refreshBots(); await until(() => workspace.getSnapshot().bots.length === 2, 'the Bots');
  const page = await mount(workspace, s => createElement(BotDirectory, { api: workspace.api, uid: s.uid, team: s.team!, members: s.directory, bots: s.bots, locale: 'en', busy: s.messageBusy, status: '', attempt: workspace.attempt, onOpen: bot => workspace.startBotDm(bot), onChanged: workspace.refreshBots }));
  context.after(() => page.unmount());
  const cards = page.find(node => node.type === 'li' && classesOf(node).includes('teams-card'));
  assert.equal(cards.length, 2);
  const sales = textOf(cards[0]), plain = textOf(cards[1]);
  assert.match(sales, /Can use on its maintainer’s computer/); assert.match(sales, /Local AppsCalendarNotes/); assert.match(sales, /Datasetsorders-2026Can editleads/);
  assert.doesNotMatch(sales, /Uses nothing/); assert.doesNotMatch(sales, /Limited in this chat/, 'a card is what the maintainer shared, not what a Chat leaves');
  assert.match(plain, /Uses nothing from its maintainer’s computer/); assert.doesNotMatch(plain, /Can use on its maintainer/);
  assert.equal(page.find(node => node.type === 'li' && textOf(node) === 'orders-2026Can edit').length, 1, 'the badge is on the dataset it belongs to');
  assert.equal(page.find(node => node.type === 'li' && textOf(node) === 'leads').length, 1, 'a dataset that is only read has no badge');
});
const classesOf = (node: { props: { className?: unknown } }): string[] => String(node.props.className ?? '').split(' ');

scenario('In a Chat the Bot is shown with what that Chat leaves of it: narrowed away is gone, and what a Chat cut is said; nothing left says so too', async context => {
  const { page, hold } = await chat(context, { kind: 'bot_dm', participants: ['user:alice', `shared_bot:${ASSISTANT}`] });
  const details = () => textOf(page.oneByClass('tw-bot-member-details'));
  assert.match(details(), /Local AppsCalendarNotes/); assert.match(details(), /Datasetsorders-2026Can editleads/); assert.doesNotMatch(details(), /Limited in this chat/);
  await hold({ [ASSISTANT]: { revision: 1, localApps: ['calendar'], datasets: [{ source: 'orders', dataset: 'leads' }] } });
  assert.match(details(), /Local AppsCalendar(?!Notes)/); assert.doesNotMatch(details(), /Notes|orders-2026|Can edit/); assert.match(details(), /Datasetsleads/); assert.match(details(), /Limited in this chat/);
  await hold({ [ASSISTANT]: { revision: 2, localApps: [], datasets: [] } });
  assert.match(details(), /Uses nothing from its maintainer’s computer/); assert.match(details(), /Limited in this chat/, 'the Chat is why it uses nothing');
  await hold({ [ASSISTANT]: { revision: 3 } });
  assert.match(details(), /Local AppsCalendarNotes/); assert.doesNotMatch(details(), /Limited in this chat/, 'a limit that was lifted is no limit');
});

for (const locale of ['zh-Hans', 'zh-Hant', 'ja', 'ko']) test(`${locale}: what a Bot may use is told in the language of the person`, () => {
  const t = teamsText(locale), en = teamsText('en');
  const draw = (bot: SharedBotCard, conversationScopes?: Record<string, BotScopeEntry>) => renderToStaticMarkup(createElement(BotMemberDetails, { bot, bots: [bot], locale, ...(conversationScopes ? { conversation: { botScopes: conversationScopes } } : {}) })).replace(/<!-- -->/g, '');
  const html = draw(scopedBot(), { [ASSISTANT]: { revision: 1, localApps: ['calendar'] } });
  for (const key of ['teams_bot_scope_uses', 'teams_bot_scope_apps', 'teams_bot_scope_datasets', 'teams_bot_scope_edit', 'teams_bot_scope_limited'] as const) {
    assert.notEqual(t(key), en(key), `${key} is translated`); assert.ok(html.includes(t(key)), `${key} is shown`);
  }
  assert.notEqual(t('teams_bot_scope_none'), en('teams_bot_scope_none')); assert.ok(draw(botCard()).includes(t('teams_bot_scope_none')));
  assert.doesNotMatch(html, /\bassistant\b/i);
});

// ───────────────────────────── narrowing a Chat ─────────────────────────────

scenario('The Chat offers to limit its Bots to a person who takes part in it, when a Bot of the Chat shares something', async context => {
  const cases: Array<[string, ChatOptions, boolean]> = [
    ['a Bot that shares something', {}, true],
    ['a Bot DM', { kind: 'bot_dm', participants: ['user:alice', `shared_bot:${ASSISTANT}`] }, true],
    ['a Bot that shares nothing', { bots: [botCard()] }, false],
    ['a Bot that shares an empty scope', { bots: [scopedBot({ scope: { localApps: [], datasets: [] } })] }, false],
    ['a person who only reads the Chat', { participants: ['user:bob', `shared_bot:${ASSISTANT}`] }, false],
    ['a Bot that is not in the Chat', { participants: ['user:alice', 'user:bob'] }, false],
    ['a Chat of people', { kind: 'dm', participants: ['user:alice', 'user:bob'] }, false],
  ];
  for (const [name, options, offered] of cases) {
    const { page } = await chat(context, options);
    assert.equal(page.byLabel(BUTTON).length, offered ? 1 : 0, name);
    await page.unmount();
  }
});

scenario('Limiting a Bot in a Chat: each thing starts ticked where the Chat allows it, saving names what stays allowed with the revision it was read at, and the Chat then shows it', async context => {
  const { page, hold, sent } = await chat(context);
  await page.click(page.oneByLabel(BUTTON));
  assert.match(page.text(), /A Bot can only use what its maintainer shared with the team\. Uncheck what a Bot should not use in this chat\./);
  assert.match(page.text(), /Limit what Fixture Bot can use here/); assert.match(page.text(), /Local Apps/); assert.match(page.text(), /Datasets/);
  for (const name of ['Calendar', 'Notes', 'orders-2026Can edit', 'leads']) assert.equal(ticked(page, name), true, `${name} starts allowed`);
  assert.equal(page.button('Save').props.disabled, true, 'nothing changed yet'); assert.equal(page.button('Allow everything shared').props.disabled, true, 'nothing is limited yet');
  // Not allowing Notes names only Calendar: datasets are left as they are.
  await page.check(box(page, 'Notes'), false); assert.equal(ticked(page, 'Notes'), false);
  await page.click(page.button('Save'));
  assert.deepEqual(sent(), [{ conversationId: 'test-chat', sharedBotId: ASSISTANT, expectedRevision: 0, scope: { localApps: ['calendar'] } }]);
  assert.match(page.text(), /Saved/); assert.equal(page.find(node => node.props.role === 'alert').length, 0);
  // The Chat's document says so, and the dialog follows it.
  await hold({ [ASSISTANT]: { revision: 1, localApps: ['calendar'] } });
  assert.equal(ticked(page, 'Calendar'), true); assert.equal(ticked(page, 'Notes'), false); assert.equal(ticked(page, 'leads'), true);
  assert.equal(page.button('Save').props.disabled, true); assert.equal(page.button('Allow everything shared').props.disabled, false, 'there is a limit to lift now');
  // The next change is written over revision 1: a dataset not allowed adds its entry, and the apps keep what they were.
  await page.check(box(page, 'leads'), false); await page.click(page.button('Save'));
  assert.deepEqual(sent().at(-1), { conversationId: 'test-chat', sharedBotId: ASSISTANT, expectedRevision: 1, scope: { localApps: ['calendar'], datasets: [{ source: 'orders', dataset: 'orders-2026' }] } });
  await hold({ [ASSISTANT]: { revision: 2, localApps: ['calendar'], datasets: [{ source: 'orders', dataset: 'orders-2026' }] } });
  // Allowing all of it again is no limit at all.
  await page.check(box(page, 'Notes')); await page.check(box(page, 'leads')); assert.equal(page.button('Save').props.disabled, false);
  await page.click(page.button('Save'));
  assert.deepEqual(sent().at(-1), { conversationId: 'test-chat', sharedBotId: ASSISTANT, expectedRevision: 2, scope: null });
  // Nothing allowed is two empty lists.
  await hold({ [ASSISTANT]: { revision: 3 } });
  for (const name of ['Calendar', 'Notes', 'orders-2026Can edit', 'leads']) await page.check(box(page, name), false);
  await page.click(page.button('Save'));
  assert.deepEqual(sent().at(-1), { conversationId: 'test-chat', sharedBotId: ASSISTANT, expectedRevision: 3, scope: { localApps: [], datasets: [] } });
});

scenario('Allowing everything shared again writes no limit, whatever the Chat held before', async context => {
  const { page, sent } = await chat(context, { botScopes: { [ASSISTANT]: { revision: 4, localApps: ['notes'], datasets: [{ source: 'orders', dataset: 'leads' }] } } });
  await page.click(page.oneByLabel(BUTTON));
  assert.deepEqual(['Calendar', 'Notes', 'orders-2026Can edit', 'leads'].map(name => ticked(page, name)), [false, true, false, true]);
  await page.click(page.button('Allow everything shared'));
  assert.deepEqual(sent(), [{ conversationId: 'test-chat', sharedBotId: ASSISTANT, expectedRevision: 4, scope: null }]);
  assert.match(page.text(), /Saved/);
});

scenario('A change made meanwhile is not written over: the Chat is shown as it is now, and the person is told', async context => {
  const { page, hold, failures, calls } = await chat(context);
  await page.click(page.oneByLabel(BUTTON));
  const reads = () => calls.filter(command => command.action === 'listSharedBots').length, before = reads();
  failures.push('BOT_SCOPE_REVISION_CONFLICT');
  await page.check(box(page, 'Notes'), false); await page.click(page.button('Save'));
  assert.match(textOf(page.find(node => node.props.role === 'alert')[0]), /Someone changed this a moment ago\. The latest is shown\. Check it and try again\./);
  assert.equal(reads(), before + 1, 'what the Bot shares is read again');
  assert.equal(page.find(node => /Saved/.test(textOf(node)) && node.props.role === 'status').length, 0);
  // Someone else limited it to Notes: that is what is shown now, and the unsaved change is gone.
  await hold({ [ASSISTANT]: { revision: 5, localApps: ['notes'] } });
  assert.deepEqual(['Calendar', 'Notes'].map(name => ticked(page, name)), [false, true]);
  assert.equal(page.button('Save').props.disabled, true, 'what is shown is what the Chat holds');
  // A second try is a fresh one, from revision 5.
  await page.check(box(page, 'Calendar')); await page.check(box(page, 'Notes'), false); await page.click(page.button('Save'));
  assert.equal(page.find(node => node.props.role === 'alert').length, 0, 'a new attempt clears the old message');
});

scenario('A Bot that no longer shares what was ticked is read again, shown as it shares now, and the person is told', async context => {
  const { fake, workspace, page, calls, failures, sent } = await chat(context);
  await page.click(page.oneByLabel(BUTTON));
  failures.push('SCOPE_NOT_PUBLISHED');
  await page.check(box(page, 'leads'), false); await page.click(page.button('Save'));
  assert.match(textOf(page.find(node => node.props.role === 'alert')[0]), /The maintainer changed what this Bot shares\. The latest is shown\. Check it and try again\./);
  assert.ok(calls.some(command => command.action === 'listSharedBots'), 'the Bots are read again');
  // The maintainer published again without Notes and leads.
  fake.bots = [scopedBot({ revision: 2, scope: { localApps: [calendar], datasets: [orders] } })]; await workspace.refreshBots(); await until(() => workspace.getSnapshot().bots[0]?.revision === 2, 'the new publication');
  assert.deepEqual(['Calendar', 'orders-2026Can edit'].map(name => ticked(page, name)), [true, true]);
  assert.equal(page.find(node => node.type === 'input' && node.props.type === 'checkbox').length, 2, 'what is no longer shared is no longer offered');
  assert.equal(page.button('Save').props.disabled, true);
  assert.equal(sent().length, 1);
});

scenario('A refused save is told in the words of the language, and a Bot that no longer takes part cannot be limited', async context => {
  const { page, failures } = await chat(context, { locale: 'ja' });
  const t = teamsText('ja');
  await page.click(page.oneByLabel(t('teams_bot_scope_title')));
  failures.push('BOT_PARTICIPANT_REQUIRED');
  await page.check(box(page, 'Notes'), false); await page.click(page.button(t('common_save')));
  assert.equal(textOf(page.find(node => node.props.role === 'alert')[0]), messageErrorText('BOT_PARTICIPANT_REQUIRED', 'ja'));
  assert.notEqual(messageErrorText('BOT_PARTICIPANT_REQUIRED', 'ja'), messageErrorText('SOMETHING_ELSE', 'ja'));
});

scenario('Each Bot of a Chat is limited on its own, at its own revision, and only a Bot that shares something is offered', async context => {
  const second = scopedBot({ sharedBotId: 'second-bot', name: 'Second Bot', scope: { localApps: [{ id: 'mail', name: 'Mail' }], datasets: [] } });
  const quiet = botCard({ sharedBotId: 'quiet-bot', name: 'Quiet Bot' });
  const { page, sent } = await chat(context, { bots: [scopedBot(), second, quiet], participants: ['user:alice', `shared_bot:${ASSISTANT}`, 'shared_bot:second-bot', 'shared_bot:quiet-bot'],
    botScopes: { [ASSISTANT]: { revision: 2, localApps: ['calendar'] }, 'second-bot': { revision: 7 } } });
  await page.click(page.oneByLabel(BUTTON));
  assert.match(page.text(), /Limit what Fixture Bot can use here/); assert.match(page.text(), /Limit what Second Bot can use here/); assert.doesNotMatch(page.text(), /Limit what Quiet Bot/);
  const saves = page.find(node => node.type === 'button' && textOf(node) === 'Save'), resets = page.find(node => node.type === 'button' && textOf(node) === 'Allow everything shared');
  assert.deepEqual(resets.map(node => node.props.disabled), [false, true], 'only the Bot that is limited has a limit to lift');
  await page.check(box(page, 'Mail'), false);
  assert.equal(saves[0]!.props.disabled, true, 'the other Bot has nothing to save');
  await page.click(page.find(node => node.type === 'button' && textOf(node) === 'Save')[1]);
  assert.deepEqual(sent(), [{ conversationId: 'test-chat', sharedBotId: 'second-bot', expectedRevision: 7, scope: { localApps: [] } }]);
});

scenario('A limit set in the Chat names ids only: no path, token or secret goes with it', async context => {
  const { page, sent } = await chat(context);
  await page.click(page.oneByLabel(BUTTON));
  await page.check(box(page, 'Calendar'), false); await page.check(box(page, 'leads'), false); await page.click(page.button('Save'));
  const [payload] = sent(); assert.ok(payload);
  assert.deepEqual(Object.keys(payload!).sort(), ['conversationId', 'expectedRevision', 'scope', 'sharedBotId']);
  assert.deepEqual(payload!.scope, { localApps: ['notes'], datasets: [{ source: 'orders', dataset: 'orders-2026' }] });
});

// ───────────────────────────── publishing with what the Bot may use ─────────────────────────────

const own: PublishedBotScope = { localApps: [calendar, notes], datasets: [{ source: 'orders', dataset: 'orders-2026', write: true, reportReview: true }, leads] };
const plainLocal: LocalPublishableBot = { instanceId: 'plain', name: 'Plain', description: '', model: 'Configured model' };
const sharingLocal = (more: Partial<LocalPublishableBot> = {}): LocalPublishableBot => ({ instanceId: 'local-new', name: 'Travel', description: '', model: 'Configured model', scope: own, ...more });
async function wizard(context: Parameters<Parameters<typeof scenario>[1]>[0], locals: LocalPublishableBot[], cards: SharedBotCard[] = [], friend?: string) {
  const sentInputs: PublishLocalBotInput[] = [];
  const { fake, workspace } = await open(context, { localBots: async () => locals, selectSharedFiles: async () => [], publishLocalBot: async input => { sentInputs.push(input); return { sharedBotId: input.sharedBotId, revision: 1 }; } });
  fake.bots = cards; await workspace.refreshBots(); await until(() => workspace.getSnapshot().bots.length === cards.length, 'the Bots');
  const page = await mount(workspace, s => createElement(BotDirectory, { api: workspace.api, uid: s.uid, team: s.team!, members: s.directory, bots: s.bots, locale: 'en', busy: s.messageBusy, status: '', attempt: workspace.attempt, onOpen: bot => workspace.startBotDm(bot), onChanged: workspace.refreshBots, ...(friend ? { friend } : {}) }));
  context.after(() => page.unmount());
  await page.click(page.button('Load local Bots'));
  const choose = async (instanceId: string) => { await page.type(page.find(node => node.type === 'select')[0], instanceId); };
  const review = async () => { await page.type(page.find(node => node.type === 'textarea')[0], 'Help prepare a family trip.'); await page.submit(); };
  const consent = () => page.find(node => node.type === 'input' && node.props.type === 'checkbox').at(-1);
  return { page, sentInputs, choose, review, consent };
}
const SCOPE_LEGEND = 'What this Bot may use in Team team-test';

scenario('Publishing: a Bot with nothing of this Desktop to share is published as before, with no question about it and nothing sent', async context => {
  const { page, sentInputs, choose, review, consent } = await wizard(context, [plainLocal, sharingLocal()]);
  await choose('plain');
  assert.doesNotMatch(page.text(), /What this Bot may use/); assert.doesNotMatch(page.text(), /Team members can see the names/);
  assert.match(page.text(), /The Bot changes nothing outside the chat except its shared list, if it has one\./); assert.doesNotMatch(page.text(), /shared above/);
  await review();
  assert.doesNotMatch(page.text(), /Can use on its maintainer/); assert.match(page.text(), /I have permission to share the role, examples, selected resources and their derived results with this audience, and confirm model costs and quotas\./);
  await page.check(consent()); await page.click(page.button('Publish Bot'));
  assert.equal(sentInputs.length, 1); assert.equal('scope' in sentInputs[0]!, false, 'a Bot published without a scope shares none');
});

scenario('Publishing: only what the local Bot may use is offered, nothing is ticked, a dataset offers only what the Bot may do in it, and nothing is sent until something is ticked', async context => {
  const { page, sentInputs, choose, review, consent } = await wizard(context, [sharingLocal()]);
  await choose('local-new');
  assert.match(page.text(), new RegExp(SCOPE_LEGEND)); assert.match(page.text(), /Team members can see the names of what you share here\./);
  assert.deepEqual(['Calendar', 'Notes', 'orders-2026', 'leads'].map(name => ticked(page as Page, name)), [false, false, false, false], 'the maintainer chooses; nothing is shared by default');
  assert.equal(page.find(node => node.type === 'input' && node.props.type === 'checkbox' && /^Can /.test(textOf(node.parent))).length, 0, 'what the Bot may do in a dataset is not asked until the dataset is');
  assert.doesNotMatch(page.text(), /Mail/, 'what the Bot does not have is not offered');
  // Ticking a dataset offers what the Bot may do in it, nothing more.
  await page.check(box(page as Page, 'orders-2026'));
  assert.deepEqual(['Can edit', 'Can open review cases'].map(name => ticked(page as Page, name)), [false, false], 'each starts off');
  await page.check(box(page as Page, 'leads'));
  assert.equal(page.find(node => node.type === 'input' && node.props.type === 'checkbox' && /^Can /.test(textOf(node.parent))).length, 2, 'a dataset the Bot can only read has no switch');
  await page.check(box(page as Page, 'leads'), false);
  // Opening review cases needs editing, and no editing stops them.
  await page.check(box(page as Page, 'Can open review cases')); assert.equal(ticked(page as Page, 'Can edit'), true, 'a review case is opened by writing it');
  await page.check(box(page as Page, 'Can edit'), false); assert.equal(ticked(page as Page, 'Can open review cases'), false, 'no editing, no review cases');
  await page.check(box(page as Page, 'orders-2026'), false); assert.equal(page.find(node => node.type === 'input' && node.props.type === 'checkbox' && /^Can /.test(textOf(node.parent))).length, 0, 'not shared: nothing to decide');
  // Unticked everything: still nothing sent.
  await review(); await page.check(consent()); await page.click(page.button('Publish Bot'));
  assert.equal('scope' in sentInputs[0]!, false);
});

scenario('Publishing with a scope: the review lists it, the consent covers it, and the publication carries ids, names and flags of exactly what was ticked', async context => {
  const { page, sentInputs, choose, review, consent } = await wizard(context, [sharingLocal()]);
  await choose('local-new');
  await page.check(box(page as Page, 'Calendar')); await page.check(box(page as Page, 'orders-2026')); await page.check(box(page as Page, 'Can edit'));
  assert.match(page.text(), /and what it does with the Local Apps and datasets shared above\./, 'the Bot no longer changes nothing outside the chat');
  assert.doesNotMatch(page.text(), /The Bot changes nothing outside the chat except its shared list, if it has one\.(?! *,)/);
  await review();
  assert.match(page.text(), /Can use on its maintainer’s computer/);
  assert.equal(page.find(node => node.type === 'li' && textOf(node) === 'Calendar').length, 1); assert.equal(page.find(node => node.type === 'li' && textOf(node) === 'orders-2026Can edit').length, 1);
  assert.equal(page.find(node => node.type === 'li' && textOf(node) === 'Notes').length, 0, 'only what was ticked');
  assert.match(page.text(), /I have permission to share the role, examples, selected resources and their derived results with this audience, to let the Bot use what is shared from this computer, and to confirm model costs and quotas\./);
  await page.check(consent()); await page.click(page.button('Publish Bot'));
  assert.equal(sentInputs.length, 1);
  assert.deepEqual(sentInputs[0]!.scope, { localApps: [{ id: 'calendar', name: 'Calendar' }], datasets: [{ source: 'orders', dataset: 'orders-2026', write: true, reportReview: false }] });
  assert.deepEqual(Object.keys(sentInputs[0]!.scope!.datasets[0]!).sort(), ['dataset', 'reportReview', 'source', 'write']);
});

scenario('Publishing with a scope: a change after the review asks for the review again, and a review case needs the edit it comes with', async context => {
  const { page, sentInputs, choose, review, consent } = await wizard(context, [sharingLocal()]);
  await choose('local-new'); await page.check(box(page as Page, 'orders-2026')); await page.check(box(page as Page, 'Can open review cases'));
  await review(); await page.check(consent());
  assert.equal(page.button('Publish Bot').props.disabled, false);
  // The form behind the review changes: the review is no longer of what would be published.
  await page.check(box(page as Page, 'Notes'));
  assert.match(page.text(), /Publication inputs or memberships changed\. Review again before publishing\./); assert.equal(page.button('Publish Bot').props.disabled, true);
  await page.submit(); await page.check(consent()); await page.click(page.button('Publish Bot'));
  assert.equal(sentInputs.length, 1);
  assert.deepEqual(sentInputs[0]!.scope, { localApps: [{ id: 'notes', name: 'Notes' }], datasets: [{ source: 'orders', dataset: 'orders-2026', write: true, reportReview: true }] });
});

scenario('Publishing: what the Bot\'s own scope loses while the form is open is not sent, however it was ticked', async context => {
  const locals = [sharingLocal()];
  const { page, sentInputs, choose, review, consent } = await wizard(context, locals);
  await choose('local-new');
  await page.check(box(page as Page, 'Calendar')); await page.check(box(page as Page, 'orders-2026')); await page.check(box(page as Page, 'Can edit'));
  // Its own scope is changed on this computer, and the Bots are loaded again: no Calendar, and no editing of the dataset.
  locals[0] = sharingLocal({ scope: { localApps: [notes], datasets: [{ source: 'orders', dataset: 'orders-2026', write: false, reportReview: false }] } });
  await page.click(page.button('Load local Bots'));
  assert.deepEqual(['Notes', 'orders-2026'].map(name => ticked(page as Page, name)), [false, true]); assert.doesNotMatch(page.text(), /Calendar/);
  assert.equal(page.find(node => node.type === 'input' && node.props.type === 'checkbox' && /^Can /.test(textOf(node.parent))).length, 0, 'the Bot cannot edit it any more');
  await review(); await page.check(consent()); await page.click(page.button('Publish Bot'));
  assert.deepEqual(sentInputs[0]!.scope, { localApps: [], datasets: [{ source: 'orders', dataset: 'orders-2026', write: false, reportReview: false }] });
});

scenario('Publishing again ticks what is published now, as far as the Bot still has it, and says what stops being shared', async context => {
  const card = botCard({ maintainer: { uid: 'alice', generation: 1 }, instanceId: 'local-new', revision: 3, scope: {
    localApps: [calendar, { id: 'gone', name: 'Old app' }], datasets: [{ source: 'orders', dataset: 'orders-2026', write: true, reportReview: true }, { source: 'orders', dataset: 'removed', write: false, reportReview: false }] } });
  const lowered = sharingLocal({ instanceId: 'local-lowered', scope: { localApps: [calendar], datasets: [{ source: 'orders', dataset: 'orders-2026', write: true, reportReview: false }] } });
  const { page, sentInputs, choose, review, consent } = await wizard(context, [sharingLocal(), lowered], [card, { ...card, sharedBotId: 'lowered-bot', instanceId: 'local-lowered' }]);
  await choose('local-new');
  assert.deepEqual(['Calendar', 'Notes', 'orders-2026', 'leads'].map(name => ticked(page as Page, name)), [true, false, true, false]);
  assert.deepEqual(['Can edit', 'Can open review cases'].map(name => ticked(page as Page, name)), [true, true], 'what the Bot may do in it is as it was published');
  assert.match(page.text(), /No longer in this Bot’s own scope, so it stops being shared: Old app, removed/);
  await review(); await page.check(consent()); await page.click(page.button('Publish Bot'));
  assert.deepEqual(sentInputs[0]!.scope, { localApps: [{ id: 'calendar', name: 'Calendar' }], datasets: [{ source: 'orders', dataset: 'orders-2026', write: true, reportReview: true }] });
  // The Bot may no longer open review cases: publishing again does not ask for it.
  await choose('local-lowered');
  assert.deepEqual(['Can edit'].map(name => ticked(page as Page, name)), [true]); assert.equal(page.find(node => node.type === 'input' && node.props.type === 'checkbox' && /^Can open review/.test(textOf(node.parent))).length, 0);
  await review(); await page.check(consent()); await page.click(page.button('Publish Bot'));
  assert.deepEqual(sentInputs[1]!.scope!.datasets, [{ source: 'orders', dataset: 'orders-2026', write: true, reportReview: false }]);
});

scenario('Publishing again a Bot that has nothing left to share says what stops, shows that it uses nothing, and sends no scope', async context => {
  const card = botCard({ maintainer: { uid: 'alice', generation: 1 }, instanceId: 'plain', revision: 2, scope: { localApps: [calendar], datasets: [leads] } });
  const { page, sentInputs, choose, review, consent } = await wizard(context, [plainLocal], [card]);
  await choose('plain');
  assert.match(page.text(), new RegExp(SCOPE_LEGEND)); assert.match(page.text(), /No longer in this Bot’s own scope, so it stops being shared: Calendar, leads/);
  assert.equal(page.find(node => node.type === 'input' && node.props.type === 'checkbox' && ['Calendar', 'leads'].includes(textOf(node.parent))).length, 0, 'nothing to tick');
  await review(); assert.match(page.text(), /Uses nothing from its maintainer’s computer/);
  await page.check(consent()); await page.click(page.button('Publish Bot'));
  assert.equal('scope' in sentInputs[0]!, false);
});

scenario('Publishing in a friendship says whom it is for', async context => {
  const { page, choose } = await wizard(context, [sharingLocal()], [], 'Bob');
  await choose('local-new');
  assert.match(page.text(), /What this Bot may use for Bob/); assert.match(page.text(), /Bob can see the names of what you share here\./); assert.doesNotMatch(page.text(), /Team members can see/);
});

for (const [code, key] of [['SCOPE_OUTSIDE_BOT', 'tw_error_scope_outside_bot'], ['SCOPE_UNSUPPORTED', 'tw_error_scope_unsupported'], ['BOT_SCOPE_REVISION_CONFLICT', 'tw_error_scope_conflict'], ['SCOPE_NOT_PUBLISHED', 'tw_error_scope_not_published']] as const) {
  test(`${code} is told in every language, in words of its own`, () => {
    for (const locale of ['en', 'zh-Hans', 'zh-Hant', 'ja', 'ko']) {
      const text = messageErrorText(code, locale);
      assert.equal(text, teamsText(locale)(key)); assert.notEqual(text, messageErrorText('NOT_A_CODE', locale), `${locale}: not the generic failure`);
      if (locale !== 'en') assert.notEqual(text, messageErrorText(code, 'en'), `${locale}: translated`);
      assert.doesNotMatch(text, /\bassistant\b/i);
    }
  });
}
