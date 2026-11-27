import assert from 'node:assert/strict';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { TeamsWorkspace } from '../../../packages/teams/workspace';
import type { TeamsApi } from '../../../packages/teams';
import type { MessageCommand } from '../../../packages/teams/messages';
import { FakeTeams, botCard, conversation, memoryStorage, team, until } from './support/fakeTeams';
import { loadTeamsModule } from './support/loadTeamsModule';
import { mount, scenario, textOf, withDom, reactTestRenderer } from './support/workspacePage';
const { BotDirectory } = loadTeamsModule<typeof import('../../../packages/teams/BotDirectory')>('BotDirectory');
const { TeamConversationView } = loadTeamsModule<typeof import('../../../packages/teams/TeamConversationView')>('TeamConversationView');
const { BotMemberDetails, botAvailabilityText } = loadTeamsModule<typeof import('../../../packages/teams/BotMemberDetails')>('BotMemberDetails');
const { teamsText } = loadTeamsModule<typeof import('../../../packages/teams/i18n')>('i18n');

async function open(context: Parameters<Parameters<typeof scenario>[1]>[0], overrides: Partial<TeamsApi> = {}) {
  const cleanups: (() => void | Promise<void>)[] = [];
  context.after(async () => { for (const work of cleanups) await work(); });
  const cleanup = (work: () => void | Promise<void>) => cleanups.unshift(work);
  withDom(context); const fake = new FakeTeams('alice'); fake.teams = [team('team-test', { role: 'owner' }), team('team-other')]; fake.bots = [botCard({ maintainer: { uid: 'alice', generation: 1 }, examples: ['Plan groceries'], availability: 'ready', capabilities: { durableWait: true, writeOperations: false }, dailyRemaining: 4, quotaResetsAt: Date.UTC(2026, 10, 17) })]; Object.assign(fake.api, overrides);
  fake.conversations.set('team-test', [conversation('bot-chat', { kind: 'bot_dm', participants: ['user:alice', 'shared_bot:assistant-fixture'] }), conversation('other-chat')]);
  const workspace = new TeamsWorkspace(fake.api, 'alice', { storage: memoryStorage, environment: new EventTarget() }); cleanup(() => workspace.close()); await until(() => workspace.getSnapshot().botsLoaded && !!workspace.getSnapshot().detail && workspace.getSnapshot().conversations.length === 2); return { fake, workspace, cleanup };
}
const directory = (workspace: TeamsWorkspace) => (s: ReturnType<TeamsWorkspace['getSnapshot']>) => createElement(BotDirectory, { key: `${s.uid}:${s.team!.id}:${s.team!.generation}`, api: workspace.api, uid: s.uid, team: s.team!, members: s.directory, bots: s.bots, locale: 'en', busy: s.messageBusy, status: '', attempt: workspace.attempt, onOpen: bot => workspace.startBotDm(bot), onExample: (bot, example) => { void workspace.openBotExample(bot, example); }, onChanged: workspace.refreshBots });

scenario('Market defaults stay in the original wizard, limit to three, can be edited and are pinned in the confirmed publication', async context => {
  const published: unknown[] = [];
  const { workspace, cleanup } = await open(context, { localBots: async () => [{ instanceId: 'new-local', name: 'Market Bot', description: '', examples: ['First default', 'Second default', 'Third default', 'Fourth default'] }], selectSharedFiles: async () => [], publishLocalBot: async input => { published.push(input); return { sharedBotId: input.sharedBotId, revision: 1 }; } });
  const page = await mount(workspace, directory(workspace)); cleanup(() => page.unmount());
  await page.click(page.button('Load local Bots')); await page.type(page.find(node => node.type === 'select')[0], 'new-local');
  assert.deepEqual(page.find(node => node.props.name?.startsWith('example-')).map(node => node.props.value), ['First default', 'Second default', 'Third default']);
  await page.type(page.find(node => node.props.name === 'example-2')[0], 'Edited second'); await page.type(page.find(node => node.type === 'textarea')[0], 'Published role');
  await page.submit(); assert.match(page.text(), /Edited second/); assert.equal(published.length, 0);
  await page.check(page.find(node => node.type === 'input' && node.props.type === 'checkbox').at(-1)); await page.click(page.button('Publish Bot'));
  assert.deepEqual((published[0] as { examples: string[] }).examples, ['First default', 'Edited second', 'Third default']);
});

scenario('A member example opens the correct existing DM and appends to its draft without sending or changing another draft', async context => {
  const { fake, workspace, cleanup } = await open(context); workspace.setDraft('bot-chat', 'Existing draft'); workspace.setDraft('other-chat', 'Unrelated draft');
  const page = await mount(workspace, snapshot => createElement(TeamConversationView, { workspace, snapshot, locale: 'en' })); cleanup(() => page.unmount());
  await page.click(page.button('Plan groceries'));
  assert.equal(workspace.getSnapshot().selectedConversationId, 'bot-chat');
  assert.equal(page.find(node => node.type === 'textarea')[0].props.value, 'Existing draft\n\nPlan groceries');
  assert.equal(workspace.draft('other-chat'), 'Unrelated draft');
  assert.equal(fake.commands.some(command => command.action === 'submit' || command.action === 'mutate'), false);
  await page.click(page.button('Plan groceries')); assert.equal(page.find(node => node.type === 'textarea')[0].props.value, 'Existing draft\n\nPlan groceries\n\nPlan groceries');
  assert.match(page.text(), /Wait and resume: Supported · Write operations: Not supported/); assert.match(page.text(), /remaining requests today: 4/); assert.match(page.text(), /2026-11-17 00:00:00 UTC/);
  workspace.selectTeam('team-other'); assert.equal(workspace.draft('bot-chat'), ''); workspace.selectTeam('team-test'); assert.equal(workspace.draft('bot-chat'), 'Existing draft\n\nPlan groceries\n\nPlan groceries');
});

scenario('Repeated example intent updates the composer after local edits even when the last published draft has the same text', async context => {
  const { fake, workspace, cleanup } = await open(context); workspace.selectConversation('bot-chat');
  workspace.setDraft('other-chat', 'Unrelated draft');
  const page = await mount(workspace, snapshot => createElement(TeamConversationView, { workspace, snapshot, locale: 'en' })); cleanup(() => page.unmount());
  const box = () => page.find(node => node.type === 'textarea')[0];
  for (const original of ['', 'Keep my local draft']) {
    await page.type(box(), original);
    await page.click(page.button('Plan groceries'));
    const expected = original ? `${original}\n\nPlan groceries` : 'Plan groceries';
    assert.equal(box().props.value, expected);
    // Return to the local text without publishing a workspace snapshot, then repeat the same intent.
    await page.type(box(), original);
    assert.equal(box().props.value, original);
    await page.click(page.button('Plan groceries'));
    assert.equal(box().props.value, expected, 'the fill is applied once even if its result equals the previous published value');
    assert.equal(workspace.draft('bot-chat'), expected);
  }
  assert.equal(workspace.draft('other-chat'), 'Unrelated draft');
  assert.equal(fake.commands.some(command => command.action === 'submit' || command.action === 'mutate'), false);
});

scenario('Explicit unavailable health blocks a new request while preserving history, examples and the draft; missing projections stay Unknown', async context => {
  const { fake, workspace, cleanup } = await open(context); fake.bots = [botCard({ examples: ['Try a question'], availability: 'quota_exhausted', ready: true, dailyRemaining: 0 })]; await workspace.refreshBots(); workspace.selectConversation('bot-chat');
  const page = await mount(workspace, snapshot => createElement(TeamConversationView, { workspace, snapshot, locale: 'en' })); cleanup(() => page.unmount());
  await page.click(page.button('Try a question')); assert.equal(page.oneByLabel('Send').props.disabled, true); assert.match(page.text(), /Daily quota exhausted/); assert.match(page.text(), /remaining requests today: 0/); assert.match(page.text(), /Wait and resume: Unknown · Write operations: Unknown/); assert.match(page.text(), /Resets: Unknown/);
  await page.key(page.find(node => node.type === 'textarea')[0], 'Enter'); assert.equal(fake.commands.some(command => command.action === 'submit'), false); assert.equal(workspace.draft('bot-chat'), 'Try a question');
  fake.bots = [botCard()]; await page.during(() => workspace.refreshBots()); assert.match(page.text(), /Status unverified/); assert.match(page.text(), /remaining requests today: Unknown/);
});

scenario('Delayed local publication metadata never overwrites a newly selected Bot or its examples', async context => {
  let release!: (value: import('../../../packages/teams/execution').PublishedKnowledge) => void;
  const pending = new Promise<import('../../../packages/teams/execution').PublishedKnowledge>(resolve => { release = resolve; });
  const { workspace, cleanup } = await open(context, { localBots: async () => [{ instanceId: 'local-fixture', name: 'Old', description: '', examples: ['Old example'] }, { instanceId: 'new-local', name: 'New', description: '', examples: ['New example'] }], selectSharedFiles: async () => [], publishLocalBot: async input => ({ sharedBotId: input.sharedBotId, revision: 1 }), publishedKnowledge: () => pending });
  const page = await mount(workspace, directory(workspace)); cleanup(() => page.unmount());
  await page.click(page.button('Load local Bots')); await page.type(page.find(node => node.type === 'select')[0], 'local-fixture'); await page.type(page.find(node => node.type === 'select')[0], 'new-local'); await page.type(page.find(node => node.type === 'textarea')[0], 'New role');
  await page.during(() => release({ sharedBotId: 'assistant-fixture', revision: 1, role: 'Old delayed role', documents: [] }));
  assert.equal(page.find(node => node.type === 'textarea')[0].props.value, 'New role'); assert.equal(page.find(node => node.props.name === 'example-1')[0].props.value, 'New example');
});

scenario('Task export reuses authorized listTasks, exposes JSON/CSV downloads and removes links when membership authority changes', async context => {
  const { fake, workspace, cleanup } = await open(context); const calls: MessageCommand[] = [], original = fake.messages;
  fake.messages = async command => { calls.push(command); return command.action === 'listTasks' ? { tasks: [], next: null } as never : original(command); };
  const page = await mount(workspace, snapshot => createElement(TeamConversationView, { workspace, snapshot, locale: 'en' })); cleanup(() => page.unmount());
  await page.click(page.button('Export my task summaries'));
  const links = page.find(node => node.type === 'a' && !!node.props.download); assert.equal(links.length, 2);
  assert.match(page.text(), /0 task summaries ready/);
  const json = await (await fetch(links[0].props.href)).json(); assert.deepEqual(json.tasks, []); assert.equal(json.scope, 'own-current-membership');
  assert.ok(calls.filter(command => command.action === 'listTasks').every(command => command.action === 'listTasks' && command.payload.conversationId === undefined));
  await page.during(() => fake.invalidate()); assert.equal(page.find(node => node.type === 'a' && !!node.props.download).length, 0);
});

for (const switchTo of ['chat', 'team', 'generation', 'account'] as const) scenario(`A delayed example never fills another ${switchTo} after DM creation`, async context => {
  const { fake, workspace, cleanup } = await open(context);
  const bot = botCard({ sharedBotId: 'assistant-new', maintainer: { uid: 'alice', generation: 1 }, examples: ['Delayed example'] });
  fake.bots = [bot]; await workspace.refreshBots();
  let release!: () => void, created = '';
  const waiting = new Promise<void>(resolve => { release = resolve; }), original = fake.messages;
  fake.messages = async command => {
    if (command.action !== 'createConversation') return original(command);
    created = command.payload.conversationId; await waiting;
    return {} as never;
  };
  const opening = workspace.openBotExample(bot, 'Delayed example');
  await until(() => !!created, 'DM creation reaches the service');
  let account: TeamsWorkspace | undefined;
  if (switchTo === 'chat') {
    const alternative = conversation('alternate-chat', { kind: 'bot_dm', participants: ['user:alice', 'shared_bot:assistant-new'] });
    fake.conversations.set('team-test', [...fake.conversations.get('team-test')!, alternative]); for (const watch of fake.open('team-test')) fake.deliver(watch);
    workspace.selectConversation(alternative.id); workspace.setDraft(alternative.id, 'Keep this chat draft');
  } else if (switchTo === 'team') { workspace.selectTeam('team-other'); workspace.setDraft(created, 'Keep this team draft'); }
  else if (switchTo === 'generation') { fake.teams = fake.teams.map(value => value.id === 'team-test' ? { ...value, generation: 2 } : value); await workspace.refresh(); workspace.setDraft(created, 'Keep this generation draft'); }
  else {
    workspace.close();
    const other = new FakeTeams('bob'); other.teams = [team('team-test')]; account = new TeamsWorkspace(other.api, 'bob', { storage: memoryStorage, environment: new EventTarget() }); cleanup(() => account!.close()); await until(() => !!account!.getSnapshot().team); account.setDraft(created, 'Keep this account draft');
  }
  release(); await opening;
  if (switchTo === 'chat') assert.equal(workspace.draft('alternate-chat'), 'Keep this chat draft');
  else if (switchTo === 'account') { assert.equal(account!.draft(created), 'Keep this account draft'); assert.equal(workspace.draft(created), ''); }
  else assert.equal(workspace.draft(created), `Keep this ${switchTo} draft`);
  assert.equal(fake.commands.some(command => command.action === 'submit'), false);
});

scenario('A newly created DM retains its example while the directory listener catches up', async context => {
  const { fake, workspace } = await open(context);
  const bot = botCard({ sharedBotId: 'assistant-new', examples: ['New DM example'] }); fake.bots = [bot]; await workspace.refreshBots();
  assert.equal(await workspace.openBotExample(bot, 'New DM example'), true);
  const id = workspace.getSnapshot().selectedConversationId; assert.ok(id);
  assert.equal(workspace.draft(id), 'New DM example'); assert.equal(fake.commands.some(command => command.action === 'submit'), false);
});

scenario('The original wizard blocks shared lists without verified write support and clears the list when switching to a read-only Bot', async context => {
  const published: import('../../../packages/teams/execution').PublishLocalBotInput[] = [];
  const { workspace, cleanup } = await open(context, { localBots: async () => [{ instanceId: 'writer-local', name: 'Writer', description: '', capabilities: { durableWait: true, writeOperations: true } }, { instanceId: 'reader-local', name: 'Reader', description: '', capabilities: { durableWait: false, writeOperations: false } }, { instanceId: 'unknown-local', name: 'Unknown', description: '' }], selectSharedFiles: async () => [], publishLocalBot: async input => { published.push(input); return { sharedBotId: input.sharedBotId, revision: 1 }; } });
  const page = await mount(workspace, directory(workspace)); cleanup(() => page.unmount());
  await page.click(page.button('Load local Bots'));
  const select = () => page.find(node => node.type === 'select')[0];
  const list = () => page.find(node => node.type === 'input' && node.props.type === 'checkbox' && String(node.parent.children.map((value: any) => typeof value === 'string' ? value : '').join('')).includes('Give this Bot'))[0];
  await page.type(select(), 'unknown-local'); assert.equal(list().props.disabled, true); await page.check(list()); assert.equal(list().props.checked, false);
  await page.type(select(), 'writer-local'); assert.equal(list().props.disabled, false); await page.check(list());
  await page.type(page.find(node => node.type === 'input' && node.props.maxLength === 120)[0], 'Shared list'); await page.type(page.find(node => node.type === 'textarea' && node.props.maxLength === 2000)[0], 'Approved use');
  await page.type(select(), 'reader-local'); assert.equal(list().props.checked, false); assert.equal(list().props.disabled, true); assert.equal(page.find(node => node.type === 'input' && node.props.maxLength === 120).length, 0); assert.match(page.text(), /Shared lists need verified write operation support/);
  await page.type(page.find(node => node.type === 'textarea')[0], 'Read-only role'); await page.submit(); await page.check(page.find(node => node.type === 'input' && node.props.type === 'checkbox').at(-1)); await page.click(page.button('Publish Bot'));
  assert.equal(published.length, 1); assert.equal('sharedList' in published[0], false);
});

for (const availability of ['maintenance', 'quota_exhausted'] as const) scenario(`${availability} expiry and failed refresh never enable a new Bot submission before fresh Ready`, async context => {
  const { fake, workspace, cleanup } = await open(context); workspace.selectConversation('bot-chat');
  const page = await mount(workspace, snapshot => createElement(TeamConversationView, { workspace, snapshot, locale: 'en' })); cleanup(() => page.unmount());
  await page.type(page.find(node => node.type === 'textarea')[0], 'Preserve this request');
  const { act } = reactTestRenderer(), flush = async () => { for (let i = 0; i < 100; i++) await Promise.resolve(); };
  // Cancel the fixture's real presence timer before installing the fake clock.
  fake.bots = [botCard({ availability: 'unverified', ready: false, onlineUntil: 0 })];
  await page.during(() => workspace.refreshBots());
  const start = Date.now(); context.mock.timers.enable({ apis: ['setTimeout', 'Date'], now: start });
  const initial = botCard({ availability, ready: availability === 'quota_exhausted', statusExpiresAt: start + 90000, onlineUntil: start + 600000 });
  fake.bots = [initial]; await act(async () => { await workspace.refreshBots(); await flush(); });
  let queues = 0; const model = workspace.getSnapshot().messaging!, queue = model.queue.bind(model); model.queue = async (...args) => { queues++; return queue(...args); };
  const submit = () => act(async () => { page.find(node => node.type === 'form')[0].props.onSubmit({ preventDefault() {} }); await flush(); });
  assert.equal(page.oneByLabel('Send').props.disabled, true); await submit(); assert.equal(queues, 0);
  let reject!: (error: Error) => void; const pending = new Promise<never>((_, fail) => { reject = fail; }), original = fake.messages;
  fake.messages = async command => command.action === 'listSharedBots' ? pending : original(command);
  await act(async () => { context.mock.timers.tick(90000); await flush(); });
  assert.match(page.text(), /Status unverified/); assert.equal(page.oneByLabel('Send').props.disabled, true); await submit(); assert.equal(queues, 0);
  await act(async () => { reject(new Error('SERVICE_UNAVAILABLE')); await flush(); });
  assert.equal(workspace.getSnapshot().botsUnverified, true); assert.equal(page.oneByLabel('Send').props.disabled, true); await submit(); assert.equal(queues, 0); assert.equal(workspace.draft('bot-chat'), 'Preserve this request');
  fake.messages = original; fake.bots = [botCard({ availability: 'unverified', ready: false, statusExpiresAt: Date.now() + 90000 })]; await act(async () => { await workspace.refreshBots(); await flush(); });
  assert.equal(page.oneByLabel('Send').props.disabled, true, 'an explicit Unknown snapshot does not erase the previous refusal');
  fake.bots = [botCard({ availability: 'ready', ready: true, statusExpiresAt: Date.now() + 90000, onlineUntil: Date.now() + 90000 })]; await act(async () => { await workspace.refreshBots(); await flush(); });
  assert.equal(page.oneByLabel('Send').props.disabled, false); await submit(); assert.equal(queues, 1); assert.equal(fake.commands.filter(command => command.action === 'submit').length, 1);
});

// U13: the words of the member details, the availability and the export come from the catalog, in every language, and a Bot is never an assistant.
scenario('Availability and the member details read in each of the five languages; a state this build does not know reads as unverified', async () => {
  const bot = (more: Partial<Parameters<typeof botCard>[0]> = {}) => botCard({ examples: ['Plan groceries'], capabilities: { durableWait: true, writeOperations: false }, dailyRemaining: 4, quotaResetsAt: Date.UTC(2026, 10, 17), ...more });
  const states = ['ready', 'paused', 'offline', 'maintenance', 'quota_exhausted', 'unverified'] as const;
  const english = states.map(availability => botAvailabilityText(bot({ availability }), 'en'));
  assert.deepEqual(english, ['Ready', 'Sharing stopped', 'Host offline', 'Temporarily under maintenance', 'Daily quota exhausted', 'Status unverified']);
  for (const locale of ['zh-Hans', 'zh-Hant', 'ja', 'ko']) {
    const words = states.map(availability => botAvailabilityText(bot({ availability }), locale));
    assert.equal(new Set(words).size, states.length, `${locale}: each state has its own words`);
    assert.ok(words.every((word, index) => word !== english[index]), `${locale}: the words are translated, not left in English`);
  }
  assert.equal(botAvailabilityText(bot({ state: 'paused', availability: 'ready' }), 'en'), 'Sharing stopped', 'a Bot whose sharing stopped reads so whatever its host reports');
  assert.equal(botAvailabilityText(bot({ availability: 'from-a-newer-service' as never }), 'en'), 'Status unverified'); assert.equal(botAvailabilityText(bot({ availability: 'constructor' as never }), 'en'), 'Status unverified');
  const draw = (locale: string, more: Parameters<typeof bot>[0] = {}) => renderToStaticMarkup(createElement(BotMemberDetails, { bot: bot({ availability: 'ready', ...more }), bots: [], locale, onExample: () => {} })).replace(/<!-- -->/g, '');
  assert.match(draw('en'), /Wait and resume: Supported · Write operations: Not supported/); assert.match(draw('en'), /Your remaining requests today: 4 · Resets: 2026-11-17 00:00:00 UTC/);
  assert.match(draw('ja'), /待機と再開: 対応 · 書き込み操作: 非対応/); assert.match(draw('ja'), /自分の今日の残りリクエスト数: 4 · リセット時刻: 2026-11-17 00:00:00 UTC/);
  assert.match(draw('zh-Hant'), /等待與恢復: 支援 · 寫入操作: 不支援/); assert.match(draw('ko'), /대기 및 재개: 지원됨 · 쓰기 작업: 지원되지 않음/);
  assert.match(draw('zh-Hans', { dailyRemaining: undefined, capabilities: undefined, quotaResetsAt: undefined }), /等待与恢复: 未知 · 写入操作: 未知.*本人今日剩余请求: 未知 · 重置于: 未知/s, 'a projection that is missing reads Unknown in the language of the person');
  assert.ok(draw('ja').includes('Plan groceries') && draw('ja').includes('入力例は下書きに追加されます'), 'the examples are the Bot\'s own words; the note about them is translated');
});

for (const [locale, tasks, ready] of [['en', 1, '1 task summary ready.'], ['en', 0, '0 task summaries ready.'], ['zh-Hant', 2, '已產生 2 項任務摘要。'], ['ja', 2, 'タスク概要を 2 件生成しました。'], ['ko', 2, '작업 요약 2개를 생성했습니다.']] as const) scenario(`The task export speaks ${locale} and counts ${tasks}`, async context => {
  const { fake, workspace, cleanup } = await open(context); const t = teamsText(locale);
  fake.messages = async () => ({ tasks: Array.from({ length: tasks }, (_, index) => ({ taskId: `task-${index}`, conversationId: 'bot-chat', sharedBotId: 'assistant-fixture', requester: { uid: 'alice', generation: 1 }, state: 'completed', createdAt: index + 1, deadline: 100, epoch: 1, activeSpentMs: 3 })), next: null }) as never;
  const page = await mount(workspace, snapshot => createElement(TeamConversationView, { workspace, snapshot, locale })); cleanup(() => page.unmount());
  assert.ok(page.text().includes(t('tw_report_intro')), 'the explanation is in the language of the person');
  await page.click(page.button(t('tw_report_export')));
  assert.ok(page.text().includes(ready), `${locale}: ${page.text()}`);
  assert.deepEqual(page.find(node => node.type === 'a' && !!node.props.download).map(node => textOf(node)), [t('tw_report_download_json'), t('tw_report_download_csv')]);
  assert.ok(page.text().includes(t('tw_report_no_cost')) && page.text().includes(t('tw_report_missing_fmt', tasks)));
});
