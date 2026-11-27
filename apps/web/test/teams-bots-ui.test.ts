import assert from 'node:assert/strict';
import vectors from '../../../packages/teams/contracts/execution-vectors.json';
import { createElement, Profiler } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { TeamsWorkspace } from '../../../packages/teams/workspace';
import type { TeamsApi } from '../../../packages/teams';
import type { MessageCommand, TeamMessage } from '../../../packages/teams/messages';
import { FakeTeams, botCard, conversation, memoryStorage, message, team, until } from './support/fakeTeams';
import { loadTeamsModule } from './support/loadTeamsModule';
import { classes, mount, scenario, textOf, withDom } from './support/workspacePage';
const { BotDirectory } = loadTeamsModule<typeof import('../../../packages/teams/BotDirectory')>('BotDirectory');
const { TeamConversationView } = loadTeamsModule<typeof import('../../../packages/teams/TeamConversationView')>('TeamConversationView');
const { BotRun, usageText, tokenSplit, runLook, RUN_STATES } = loadTeamsModule<typeof import('../../../packages/teams/BotRunCards')>('BotRunCards');
const { teamsText } = loadTeamsModule<typeof import('../../../packages/teams/i18n')>('i18n');
const { messageErrorText } = loadTeamsModule<typeof import('../../../packages/teams/errors')>('errors');
async function open(context: Parameters<Parameters<typeof scenario>[1]>[0], overrides: Partial<TeamsApi> = {}) {
  withDom(context); const fake = new FakeTeams('alice'); fake.teams = [team('team-test', { role: 'owner' })]; fake.bots = [botCard({ maintainer: { uid: 'alice', generation: 1 } })]; Object.assign(fake.api, overrides);
  const workspace = new TeamsWorkspace(fake.api, 'alice', { storage: memoryStorage, environment: new EventTarget() }); context.after(() => workspace.close()); await until(() => workspace.getSnapshot().botsLoaded && !!workspace.getSnapshot().detail); return { fake, workspace };
}
const directory = (workspace: TeamsWorkspace) => (s: ReturnType<TeamsWorkspace['getSnapshot']>) => createElement(BotDirectory, { api: workspace.api, uid: s.uid, team: s.team!, members: s.directory, bots: s.bots, locale: 'en', busy: s.messageBusy, status: '', attempt: workspace.attempt, onOpen: bot => workspace.startBotDm(bot), onChanged: workspace.refreshBots });
const friendDirectory = (workspace: TeamsWorkspace) => (s: ReturnType<TeamsWorkspace['getSnapshot']>) => createElement(BotDirectory, { api: workspace.api, uid: s.uid, team: s.team!, members: s.directory, bots: s.bots, locale: 'en', busy: s.messageBusy, status: '', attempt: workspace.attempt, onOpen: bot => workspace.startBotDm(bot), onChanged: workspace.refreshBots, friend: 'Bob' });
scenario('Member home requires no model setup and opens the document; a finished task without usage says so', async context => {
  const { fake, workspace } = await open(context); const calls: MessageCommand[] = []; fake.conversations.set('team-test', [conversation('test-chat')]);
  fake.messages = async <T>(command: MessageCommand) => { calls.push(command); if (command.action === 'listTasks') return { tasks: [{ taskId: 'task-test', conversationId: 'test-chat', sharedBotId: 'assistant-fixture', requester: { uid: 'alice', generation: 1 }, state: 'completed', createdAt: 1, deadline: 2, artifactId: 'artifact-test' }] } as T; if (command.action === 'readArtifact') return { artifactId: 'artifact-test', revision: 1, digest: 'a'.repeat(64), document: { title: 'Trip preparation', body: 'Pack passports' }, currentRevision: 1 } as T; return { bots: fake.bots } as T; };
  const page = await mount(workspace, snapshot => createElement(TeamConversationView, { workspace, snapshot, locale: 'en' })); context.after(() => page.unmount()); assert.match(page.text(), /No model or API key setup/); assert.match(page.text(), /Usage unavailable/); assert.doesNotMatch(page.text(), /Missing usage|Reopen document/);
  await page.click(page.button('Open document')); assert.match(page.text(), /Trip preparation/); assert.match(page.text(), /Pack passports/); await page.click(page.button('Close')); await page.click(page.button('Open document')); assert.equal(calls.filter(c => c.action === 'readArtifact').length, 2);
});
scenario('The Bot document dialog renders its body through the host Markdown renderer, else as plain text', async context => {
  const { fake, workspace } = await open(context); fake.conversations.set('team-test', [conversation('test-chat')]);
  fake.messages = async <T>(command: MessageCommand) => { if (command.action === 'listTasks') return { tasks: [{ taskId: 'task-test', conversationId: 'test-chat', sharedBotId: 'assistant-fixture', requester: { uid: 'alice', generation: 1 }, state: 'completed', createdAt: 1, deadline: 2, artifactId: 'artifact-test' }] } as T; if (command.action === 'readArtifact') return { artifactId: 'artifact-test', revision: 1, digest: 'a'.repeat(64), document: { title: 'Trip preparation', body: '# Pack\n- passports' }, currentRevision: 1 } as T; return { bots: fake.bots } as T; };
  const rendered: string[] = [];
  const renderMarkdown = (text: string) => { rendered.push(text); return createElement('div', { className: 'host-md' }, 'rendered:' + text); };
  const page = await mount(workspace, snapshot => createElement(TeamConversationView, { workspace, snapshot, locale: 'en', renderMarkdown })); context.after(() => page.unmount());
  await until(() => /Open document/.test(page.text()), 'the task');
  await page.click(page.button('Open document'));
  assert.ok(rendered.length > 0 && rendered.every(text => text === '# Pack\n- passports')); assert.match(page.text(), /rendered:# Pack/);
});
scenario('Publication confirms explicit role, native files, membership audience, costs and quotas', async context => {
  const published: unknown[] = [];
  const { workspace } = await open(context, { localBots: async () => [{ instanceId: 'local-new', name: 'Travel', description: '', model: 'Configured model' }], selectSharedFiles: async () => [{ token: 'trusted-token', kind: 'file', name: 'Trip notes.txt', bytes: 18, sha256: 'a'.repeat(64), mediaType: 'text/plain' }], publishLocalBot: async input => { published.push(input); return { sharedBotId: input.sharedBotId, revision: 1 }; } });
  const page = await mount(workspace, directory(workspace)); context.after(() => page.unmount()); await page.click(page.button('Load local Bots')); await page.type(page.find(n => n.type === 'select')[0], 'local-new'); await page.type(page.find(n => n.type === 'textarea')[0], 'Help prepare a family trip.'); await page.click(page.button('Select read-only files / SKILL.md')); await page.check(page.find(n => n.type === 'input' && n.props.type === 'checkbox')[0]); await page.submit(); assert.match(page.text(), /Trip notes.txt/); assert.equal(page.button('Publish Bot').props.disabled, true); await page.check(page.find(n => n.type === 'input' && n.props.type === 'checkbox').at(-1)); await page.click(page.button('Publish Bot'));
  assert.equal(published.length, 1); const value = published[0] as Record<string, unknown>; assert.deepEqual(value.fileTokens, ['trusted-token']); assert.equal(value.role, 'Help prepare a family trip.'); assert.equal(value.instanceId, 'local-new'); assert.notEqual(value.sharedBotId, value.instanceId); assert.deepEqual(value.usePolicy, { mode: 'listed', members: [{ uid: 'bob', generation: 1 }] }); assert.deepEqual(value.quotas, { team: 100, member: 20, bot: 100 });
});
scenario('Publication refresh preserves role and native file drafts', async context => {
  const { fake, workspace } = await open(context, { localBots: async () => [{ instanceId: 'local-new', name: 'Travel', description: '' }], selectSharedFiles: async () => [{ token: 'native-token', kind: 'file', name: 'notes.txt', bytes: 4, sha256: 'a'.repeat(64), mediaType: 'text/plain' }], publishLocalBot: async () => ({ sharedBotId: 'shared-new', revision: 1 }) });
  const page = await mount(workspace, directory(workspace)); context.after(() => page.unmount()); await page.click(page.button('Load local Bots')); await page.type(page.find(n => n.type === 'select')[0], 'local-new'); await page.type(page.find(n => n.type === 'textarea')[0], 'Draft role'); await page.click(page.button('Select read-only files / SKILL.md')); await page.during(() => { fake.signal({ bots: true }); }); assert.equal(page.find(n => n.type === 'textarea')[0].props.value, 'Draft role'); assert.match(page.text(), /notes.txt/);
});
scenario('Publishing a Bot again shows what it is shared with now: each file is kept, replaced as its next revision or no longer shared, and the review says which (M3-1)', async context => {
  const published: Record<string, unknown>[] = [], sha = (c: string) => c.repeat(64), picks = [{ token: 'token-handbook-2', kind: 'file' as const, name: 'handbook-2026.md', sha256: sha('d'), bytes: 20, mediaType: 'text/markdown' }, { token: 'token-skill', kind: 'skill' as const, name: 'SKILL.md', sha256: sha('e'), bytes: 9, mediaType: 'text/markdown' }];
  const documents = [{ resourceId: 'res-handbook', revision: 2, kind: 'file' as const, name: 'handbook.md', sha256: sha('a'), bytes: 10 }, { resourceId: 'res-prices', revision: 1, kind: 'file' as const, name: 'prices.md', sha256: sha('b'), bytes: 11 }, { resourceId: 'res-notes', revision: 1, kind: 'file' as const, name: 'notes.md', sha256: sha('c'), bytes: 12 }];
  let revision = 2;
  const { fake, workspace } = await open(context, { localBots: async () => [{ instanceId: 'local-fixture', name: 'Travel', description: '', model: 'Configured model' }], selectSharedFiles: async () => picks.splice(0, 1),
    publishedKnowledge: async (_team, sharedBotId) => ({ sharedBotId, revision, role: 'Help with trips.', documents }), publishLocalBot: async input => { published.push(input as unknown as Record<string, unknown>); return { sharedBotId: input.sharedBotId, revision: input.expectedRevision + 1 }; } });
  fake.bots = [botCard({ maintainer: { uid: 'alice', generation: 1 }, revision: 2 })]; await workspace.refreshBots(); await until(() => workspace.getSnapshot().bots[0]?.revision === 2, 'the published Bot');
  const page = await mount(workspace, directory(workspace)); context.after(() => page.unmount());
  await page.click(page.button('Load local Bots')); await page.type(page.find(n => n.type === 'select')[0], 'local-fixture');
  await until(() => /Shared now/.test(page.text()), 'the files the Bot is shared with');
  assert.equal(page.find(n => n.type === 'textarea')[0].props.value, 'Help with trips.', 'the published role is carried into the form');
  assert.match(page.text(), /handbook\.md · file · revision 2/); assert.match(page.text(), /prices\.md · file · revision 1/); assert.match(page.text(), /requests that started with it are stopped/);
  assert.doesNotMatch(page.text(), new RegExp(sha('a')), 'no digest noise before the review');
  // Replace one, stop sharing another, leave the third alone.
  await page.click(page.oneByLabel('Replace handbook.md')); assert.match(page.text(), /will be replaced by handbook-2026\.md/);
  await page.click(page.oneByLabel('Stop sharing prices.md')); assert.match(page.text(), /prices\.md · file · revision 1 · will stop being shared/);
  assert.equal(page.byLabel('Replace prices.md').length, 0, 'a file that will stop being shared cannot also be replaced'); assert.equal(page.byLabel('Keep sharing prices.md').length, 1);
  // A replacement of another kind is refused and changes nothing.
  await page.click(page.oneByLabel('Replace notes.md')); assert.doesNotMatch(page.text(), /will be replaced by SKILL\.md/);
  await page.submit();
  assert.match(page.text(), /handbook\.md → handbook-2026\.md · file · revision 3 · d{64}/); assert.match(page.text(), /prices\.md · will stop being shared/); assert.match(page.text(), /notes\.md · file · unchanged/);
  await page.check(page.find(n => n.type === 'input' && n.props.type === 'checkbox').at(-1)); await page.click(page.button('Publish Bot'));
  assert.equal(published.length, 1); const first = published[0];
  assert.deepEqual([first.sharedBotId, first.expectedRevision, first.role, first.fileTokens], ['assistant-fixture', 2, 'Help with trips.', []]);
  assert.deepEqual(first.keptResources, ['res-notes']); assert.deepEqual(first.replacedResources, { 'res-handbook': 'token-handbook-2' });
  // Changing one's mind before publishing: both choices are undone and everything stays as it is.
  revision = 3; fake.bots = [botCard({ maintainer: { uid: 'alice', generation: 1 }, revision: 3 })]; await workspace.refreshBots(); await until(() => workspace.getSnapshot().bots[0]?.revision === 3, 'the next revision');
  await page.type(page.find(n => n.type === 'select')[0], 'local-fixture'); await until(() => /Shared now/.test(page.text()), 'the files again');
  await page.click(page.oneByLabel('Stop sharing notes.md')); await page.click(page.oneByLabel('Keep sharing notes.md')); assert.doesNotMatch(page.text(), /will stop being shared/);
  await page.submit(); await page.check(page.find(n => n.type === 'input' && n.props.type === 'checkbox').at(-1)); await page.click(page.button('Publish Bot'));
  assert.deepEqual(published[1].keptResources, ['res-handbook', 'res-prices', 'res-notes']); assert.deepEqual(published[1].replacedResources, {});
  // A publication this Desktop does not hold at the Bot's revision is entered again: nothing is carried.
  revision = 9; fake.bots = [botCard({ maintainer: { uid: 'alice', generation: 1 }, revision: 4 })]; await workspace.refreshBots(); await until(() => workspace.getSnapshot().bots[0]?.revision === 4, 'a revision this Desktop does not hold');
  await page.type(page.find(n => n.type === 'select')[0], 'local-fixture'); await page.during(async () => { await new Promise(resolve => setTimeout(resolve, 20)); });
  assert.doesNotMatch(page.text(), /Shared now/); assert.equal(page.find(n => n.type === 'textarea')[0].props.value, '');
});
scenario('On Desktop a maintainer sees how this Mac serves their Bots, what stops it, and turns on what keeps it serving; nobody else is shown it (M3-2)', async context => {
  const changes: unknown[] = []; let opened = 0, reads = 0;
  let host: import('../../../packages/teams/execution').SharedHostStatus = { bots: [{ teamId: 'team-test', sharedBotId: 'assistant-fixture', state: 'ready', running: 1 }], openAtLogin: { available: true, enabled: false, needsApproval: false }, keepAwake: false, onBattery: false, decisionConfigured: false, isolationAvailable: true };
  const { fake, workspace } = await open(context, { hostStatus: async () => { reads += 1; return host; }, openLoginItems: async () => { opened += 1; },
    // The system answers a request to open at login by asking the owner to allow it.
    setHostBackground: async change => { changes.push(change); host = { ...host, ...(change.openAtLogin ? { openAtLogin: { available: true, enabled: false, needsApproval: true } } : {}), ...(change.keepAwake === undefined ? {} : { keepAwake: change.keepAwake }) }; } });
  fake.bots = [botCard({ maintainer: { uid: 'alice', generation: 1 } }), botCard({ sharedBotId: 'assistant-elsewhere', name: 'Elsewhere', maintainer: { uid: 'alice', generation: 1 } }), botCard({ sharedBotId: 'assistant-theirs', name: 'Theirs', maintainer: { uid: 'bob', generation: 1 } })];
  await workspace.refreshBots(); await until(() => workspace.getSnapshot().bots.length === 3, 'three Bots');
  const page = await mount(workspace, directory(workspace)); context.after(() => page.unmount());
  await until(() => page.byLabel('This Mac').length === 1, 'the host section'); const panel = () => textOf(page.oneByLabel('This Mac'));
  assert.match(panel(), /Fixture Bot is answering from this Mac · 1 in progress/); assert.match(panel(), /Elsewhere is published from another computer\. This Mac does not answer for it\./); assert.doesNotMatch(panel(), /Theirs/, 'only the Bots the person maintains');
  assert.match(panel(), /keeps answering after you close the window\. It stops when you quit LingXi or sign out, and while the Mac is asleep or offline\./);
  assert.match(panel(), /After this Mac restarts, the Bot stays unavailable until you open LingXi\./); assert.match(panel(), /may go to sleep when it is idle/);
  const box = (label: RegExp) => page.find(n => n.type === 'input' && n.props.type === 'checkbox' && label.test(textOf(n.parent)))[0];
  assert.deepEqual([box(/Open LingXi in the background/).props.checked, box(/Keep this Mac awake/).props.checked], [false, false]);
  await page.check(box(/Open LingXi in the background/)); assert.deepEqual(changes, [{ openAtLogin: true }]);
  await until(() => /Allow LingXi in System Settings → General → Login Items\./.test(panel()), 'the system asks for approval'); await page.click(page.button('Open Login Items')); assert.equal(opened, 1);
  await page.check(box(/Keep this Mac awake/)); assert.deepEqual(changes.at(-1), { keepAwake: true });
  await until(() => /While plugged in, this Mac stays awake for as long as a Bot you share is online\./.test(panel()), 'kept awake on mains'); assert.equal(box(/Keep this Mac awake/).props.checked, true);
  // What stands in the way is named; the state is read again when the cards change.
  const before = reads; host = { ...host, onBattery: true, bots: [{ teamId: 'team-test', sharedBotId: 'assistant-fixture', state: 'not_ready', reason: 'model', running: 0 }] };
  fake.bots = fake.bots.map(bot => bot.sharedBotId === 'assistant-fixture' ? { ...bot, ready: false } : bot); await workspace.refreshBots();
  await until(() => /cannot answer: its model account is not available on this Mac/.test(panel()), 'the reason'); assert.ok(reads > before); assert.match(panel(), /On battery, this Mac stays awake only while a request is being answered\./);
  // A Bot that only another computer serves offers no switches: they would change nothing for it.
  host = { ...host, bots: [] }; fake.bots = fake.bots.map(bot => ({ ...bot, revision: bot.revision + 1 })); await workspace.refreshBots();
  await until(() => /Fixture Bot is published from another computer/.test(panel()), 'served elsewhere'); assert.doesNotMatch(panel(), /Keep this Mac awake|Open LingXi in the background/);
  // Someone who maintains no Bot here is shown nothing.
  fake.bots = [botCard({ sharedBotId: 'assistant-theirs', name: 'Theirs', maintainer: { uid: 'bob', generation: 1 } })]; await workspace.refreshBots();
  await until(() => page.byLabel('This Mac').length === 0, 'no host section');
});
scenario('A friendship\'s Bots are shared with the friend alone: no audience choice, the review names the friend, and a stopped Bot is published again as its next revision (D23)', async context => {
  const published: unknown[] = [];
  const { fake, workspace } = await open(context, { localBots: async () => [{ instanceId: 'local-fixture', name: 'Travel', description: '', model: 'Configured model' }], selectSharedFiles: async () => [], publishLocalBot: async input => { published.push(input); return { sharedBotId: input.sharedBotId, revision: input.expectedRevision + 1 }; } });
  fake.bots = [botCard({ maintainer: { uid: 'alice', generation: 1 }, state: 'paused', revision: 2 })]; await workspace.refreshBots(); await until(() => workspace.getSnapshot().bots[0]?.state === 'paused', 'the stopped Bot');
  const page = await mount(workspace, friendDirectory(workspace)); context.after(() => page.unmount());
  assert.match(page.text(), /Share a Bot with Bob/); assert.match(page.text(), /A Bot runs on its maintainer’s Desktop\. The maintainer can read every conversation with it\./); assert.match(page.text(), /Sharing stopped/); assert.doesNotMatch(page.text(), /Paused/);
  assert.doesNotMatch(page.text(), /Daily requests|All current and future members|Find Bots|Team Bots|Pause\b/);
  await page.click(page.button('Load local Bots'));
  assert.equal(page.find(n => n.type === 'select').length, 1, 'only the choice of the local Bot: the audience is the friend');
  await page.type(page.find(n => n.type === 'select')[0], 'local-fixture'); await page.type(page.find(n => n.type === 'textarea')[0], 'Help with trips.');
  assert.deepEqual(page.find(n => n.type === 'input' && n.props.type === 'checkbox').map(n => textOf(n.parent)), ['Give this Bot a shared list'], 'no member checkboxes');
  await page.submit();
  assert.match(page.text(), /Audience: Bob\./); assert.match(page.text(), /The maintainer can read every conversation with it\./); assert.doesNotMatch(page.text(), /every current and future team member/);
  await page.check(page.find(n => n.type === 'input' && n.props.type === 'checkbox').at(-1)); await page.click(page.button('Publish Bot'));
  assert.equal(published.length, 1); const value = published[0] as Record<string, unknown>;
  assert.deepEqual(value.usePolicy, { mode: 'all' }); assert.deepEqual(value.quotas, { team: 100, member: 20, bot: 100 });
  assert.equal(value.sharedBotId, 'assistant-fixture', 'the same shared Bot'); assert.equal(value.expectedRevision, 2, 'as its next revision');
  assert.deepEqual([value.teamId, value.generation], ['team-test', 1]);
});
scenario('Stop sharing asks first, names the Bot and can be cancelled; confirming uses shared identity and revision with no legacy recovery controls', async context => {
  const { fake, workspace } = await open(context); const page = await mount(workspace, directory(workspace)); context.after(() => page.unmount());
  const dialogs = () => page.find(n => n.type === 'dialog'), pauses = () => fake.commands.filter(c => c.action === 'pauseSharedBot');
  const inDialog = (name: string) => dialogs()[0].findAll((n: any) => n.type === 'button' && textOf(n) === name)[0];
  assert.equal(page.find(n => n.type === 'button' && textOf(n) === 'Pause').length, 0, 'the word is Stop sharing everywhere');
  await page.click(page.button('Stop sharing'));
  assert.equal(dialogs().length, 1); assert.equal(dialogs()[0].props.role, 'alertdialog');
  assert.match(textOf(dialogs()[0]), /Stop sharing “Fixture Bot”\?/); assert.match(textOf(dialogs()[0]), /New requests to this Bot stop\. Existing conversations stay\. You can share it again from LingXi Desktop\./);
  assert.ok(dialogs()[0].props['aria-describedby'], 'the consequence is the dialog\'s description'); assert.equal(inDialog('Cancel').props['data-autofocus'], true, 'the safe choice has the focus');
  assert.equal(pauses().length, 0, 'asking is not doing');
  await page.click(inDialog('Cancel')); assert.equal(dialogs().length, 0); assert.equal(pauses().length, 0);
  await page.click(page.button('Stop sharing')); await page.click(inDialog('Stop sharing'));
  const command = pauses()[0]; assert.equal(command?.action, 'pauseSharedBot'); if (command?.action === 'pauseSharedBot') assert.deepEqual(command.payload, { sharedBotId: 'assistant-fixture', expectedRevision: 1 }); assert.doesNotMatch(page.text(), /Settle request|Send answer/);
});
scenario('Usage reads like the phones say it: the total of the tokens in the compact number of the language, then the cost, and nothing is invented', async () => {
  const done = (usage: Record<string, unknown>) => ({ status: 'complete', inputTokens: 1, outputTokens: 1, ...usage }) as never;
  const english = (usage: unknown) => usageText(usage as never, 'en'), chinese = (usage: unknown) => usageText(usage as never, 'zh-Hans');
  // The vectors of TeamTaskUsageTests on iOS.
  assert.equal(english(undefined), 'Usage unavailable'); assert.equal(english({ status: 'missing' }), 'Usage unavailable'); assert.equal(english({ status: 'missing', inputTokens: 5, outputTokens: 6, costMicros: 7, currency: 'USD' }), 'Usage unavailable');
  assert.equal(chinese(undefined), '用量不可用');
  assert.equal(english({ status: 'partial', inputTokens: 3, outputTokens: 4 }), 'Partial usage · 7 tokens · cost unknown'); assert.equal(chinese({ status: 'partial', inputTokens: 3, outputTokens: 4 }), '用量不完整 · 7 tokens · 费用未知');
  assert.equal(english(done({ inputTokens: 123, outputTokens: 456, costMicros: 1322, currency: 'USD' })), '579 tokens · $0.0013');
  assert.equal(english(done({ inputTokens: 3, outputTokens: 4, costMicros: 0, currency: 'USD' })), '7 tokens · $0.00');
  assert.equal(english({ status: 'complete', outputTokens: 4, costMicros: 2000000, currency: 'USD' }), '4 tokens · $2.00', 'a missing count is zero');
  assert.equal(english(done({ inputTokens: 1000, outputTokens: 234, costMicros: 1200, currency: 'USD' })), '1.2K tokens · $0.0012');
  assert.equal(english(done({ inputTokens: 2000000, outputTokens: 0, costMicros: 0, currency: 'USD' })), '2M tokens · $0.00');
  assert.equal(chinese(done({ inputTokens: 12345, outputTokens: 0, costMicros: 0, currency: 'CNY' })), '1.2万 tokens · ¥0.00');
  assert.equal(english(done({ costMicros: 120000, currency: 'USD' })), '2 tokens · $0.12'); assert.equal(english(done({ costMicros: 123456, currency: 'usd' })), '2 tokens · $0.1235', 'the code is not case sensitive');
  assert.ok(english(done({ costMicros: 2500000, currency: 'EUR' })).endsWith('€2.50'));
  assert.equal(english(done({ costMicros: 50, currency: 'USD' })), '2 tokens · <$0.0001'); assert.equal(english(done({ costMicros: 100, currency: 'USD' })), '2 tokens · $0.0001');
  assert.equal(english({ status: 'complete', inputTokens: 1, outputTokens: 2, costMicros: 2500000 }), '3 tokens · 2.50 currency unknown'); assert.equal(english({ status: 'complete', inputTokens: 1, outputTokens: 2, costMicros: 2500000, currency: '' }), '3 tokens · 2.50 currency unknown');
  assert.equal(english({ status: 'complete', inputTokens: 1, outputTokens: 2 }), '3 tokens · cost unknown');
  // The split stays available, as the tooltip of the line.
  assert.equal(tokenSplit({ status: 'complete', inputTokens: 123, outputTokens: 456 }, 'en'), '123 / 456 tokens'); assert.equal(tokenSplit({ status: 'partial', outputTokens: 4 } as never, 'en'), '0 / 4 tokens');
  assert.equal(tokenSplit({ status: 'missing' }, 'en'), ''); assert.equal(tokenSplit(undefined, 'en'), '');
  assert.equal(tokenSplit({ status: 'complete', inputTokens: 123, outputTokens: 456 }, 'ja'), '123 / 456 トークン'); assert.equal(tokenSplit({ status: 'complete', inputTokens: 123, outputTokens: 456 }, 'ko'), '123 / 456 토큰');
});

scenario('A Bot task\'s card: the glyph beside the state in words, one line under it and at most one action; only Completed is green and only Failed red; a state it does not know is Unknown', async () => {
  const scope = { teamId: 'team-test', generation: 1 };
  // Bob neither asked for the task nor maintains the Bot, so he has nothing to cancel.
  let viewer = { uid: 'bob', bots: [], directory: [], pending: [] } as never;
  const draw = (task: Record<string, unknown>, locale = 'en') => renderToStaticMarkup(createElement(BotRun, { task: { taskId: 't', conversationId: 'c', sharedBotId: 's', requester: { uid: 'alice', generation: 1 }, createdAt: 1, deadline: 2, state: 'queued', ...task } as never, api: new FakeTeams().api, scope, locale, snapshot: viewer }));
  const states: [string, string, string, string][] = [
    ['queued', 'Queued', 'muted', 'Waiting for the Bot’s Desktop to pick this up.'], ['running', 'Processing', 'muted', 'The Bot is working on it.'],
    ['failed', 'Failed', 'danger', 'The Bot couldn’t finish this request. Send it again.'], ['expired', 'Expired', 'muted', 'The Bot’s Desktop didn’t pick this up in time. Send it again.'],
    ['interrupted', 'Interrupted', 'muted', 'The Bot’s Desktop stopped before finishing. Send a new request.'],
    ['authorization_required', 'Authorization required', 'muted', 'This result can’t be shown yet. Ask the Bot’s maintainer to authorize it.'], ['revoked', 'Access revoked', 'muted', 'You no longer have access to this Bot.'],
  ];
  for (const [state, label, tone, detail] of states) {
    const markup = draw({ state });
    assert.match(markup, new RegExp(`<section class="team-bot-run" data-state="${state}" data-tone="${tone}" aria-label="Task status">`), state);
    assert.ok(markup.includes(`<span>${label}</span>`), `${state}: the state is words`); assert.ok(markup.includes(`<p class="tw-run-detail">${detail}</p>`), `${state}: one line of what is going on or what to do`);
    assert.match(markup, /<span class="tw-run-glyph"><svg[^>]*aria-hidden="true"/, `${state}: the glyph is decoration, the state is never colour alone`); assert.match(markup, /<p class="tw-run-state" role="status">/);
    assert.doesNotMatch(markup, /<button/, `${state}: nothing to press without a document`);
  }
  // Cancelled says it in its label alone: the person did it, so there is no sentence under it, and no usage line unless the run used something.
  const cancelled = draw({ state: 'cancelled' }); assert.match(cancelled, /data-state="cancelled" data-tone="muted"/); assert.ok(cancelled.includes('<span>Cancelled</span>')); assert.doesNotMatch(cancelled, /tw-run-detail/);
  assert.match(draw({ state: 'cancelled', usage: { status: 'complete', inputTokens: 1, outputTokens: 2, costMicros: 1200, currency: 'USD' } }), /<p class="tw-run-detail" title="1 \/ 2 tokens">3 tokens · \$0\.0012<\/p>/);
  // Completed: green, no sentence, the usage as its line, and the split of the tokens for whoever hovers it.
  const done = draw({ state: 'completed', usage: { status: 'complete', inputTokens: 1000, outputTokens: 234, costMicros: 1200, currency: 'USD' } });
  assert.match(done, /data-state="completed" data-tone="ok"/); assert.ok(done.includes('<span>Completed</span>')); assert.ok(done.includes('<p class="tw-run-detail" title="1000 / 234 tokens">1.2K tokens · $0.0012</p>'));
  assert.ok(draw({ state: 'completed' }).includes('<p class="tw-run-detail">Usage unavailable</p>'), 'a usage that is not there is not made up');
  assert.ok(draw({ state: 'completed' }, 'zh-Hans').includes('<span>已完成</span>') && draw({ state: 'completed' }, 'zh-Hans').includes('用量不可用') && draw({ state: 'queued' }, 'zh-Hans').includes('等待 Bot 所在的 Desktop 接收此请求。'));
  // A running task turns, and has the still glyph for reduced motion beside it.
  const running = draw({ state: 'running' }); assert.match(running, /class="tw-icon tw-run-spin"/); assert.match(running, /class="tw-icon tw-run-still"/); assert.doesNotMatch(draw({ state: 'queued' }), /tw-run-spin|tw-run-still/);
  // The one action is the document; a failed task does not offer a document it has no reason to have.
  const withDocument = draw({ state: 'completed', artifactId: 'artifact-1' }); assert.equal((withDocument.match(/<button/g) ?? []).length, 1); assert.match(withDocument, /<button type="button">Open document<\/button>/);
  assert.match(draw({ state: 'completed', artifactId: 'artifact-1' }, 'zh-Hans'), />打开文档</);
  // Only Completed is ok and only Failed is danger; a state the service adds later is one more row of the table, and until then it reads Unknown.
  assert.deepEqual(Object.entries(RUN_STATES).filter(([, look]) => look.tone === 'ok').map(([state]) => state), ['completed']); assert.deepEqual(Object.entries(RUN_STATES).filter(([, look]) => look.tone === 'danger').map(([state]) => state), ['failed']);
  assert.deepEqual(Object.keys(RUN_STATES).sort(), ['authorization_required', 'awaiting_approval', 'cancelled', 'completed', 'declined', 'expired', 'failed', 'interrupted', 'needs_reconciliation', 'queued', 'revoked', 'running', 'waiting']);
  assert.deepEqual(Object.keys(RUN_STATES).sort(), [...vectors.taskStates].sort(), 'the table has a row for every state the contract lists, iOS and Android read the same list');
  // A reply held for its Bot's maintainer: the words of an approval, a sentence that says who reads it first, and nothing to approve here.
  const held = draw({ state: 'awaiting_approval' });
  assert.match(held, /data-state="awaiting_approval" data-tone="muted"/); assert.ok(held.includes('<span>Waiting for approval</span>')); assert.ok(held.includes('<p class="tw-run-detail">The Bot’s maintainer reads the reply before it is posted.</p>'));
  assert.doesNotMatch(held, /<button/, 'a member who is not the one asked to approve has nothing to press');
  assert.ok(draw({ state: 'awaiting_approval' }, 'zh-Hans').includes('<span>等待批准</span>'));
  // Main's states keep their own line: who the Bot waits for, and what needs checking.
  const waiting = draw({ state: 'waiting', wait: { waitId: 'w', kind: 'human_input', recipient: { uid: 'alice', generation: 1 }, messageId: 'm', expiresAt: Date.now() + 120_000 } });
  assert.match(waiting, /data-state="waiting" data-tone="muted"/); assert.ok(waiting.includes('<span>Waiting for a reply</span>')); assert.match(waiting, /<p class="tw-run-detail">Waiting for Member · expires in 2 min<\/p>/);
  assert.match(draw({ state: 'needs_reconciliation' }), /<span>Needs checking<\/span><\/p><p class="tw-run-detail">An action outside the chat may or may not have gone through\. The Bot’s maintainer needs to confirm it\.<\/p>/);
  // Sent while the Bot's Desktop was off (rule 6): the queued line names the Bot and says it answers when it is back, also for a member who has no card of it.
  // A task without the flag keeps the old line, and one that expired is the expired state: the service drops the flag at the deadline, and only a queued task speaks of waiting.
  assert.ok(draw({ state: 'queued', hostWait: true }).includes('<p class="tw-run-detail">Bot is offline and will reply when it’s back online</p>'));
  viewer = { uid: 'bob', bots: [botCard({ sharedBotId: 's', name: 'Helper', availability: 'offline', ready: false })], directory: [], pending: [] } as never;
  assert.ok(draw({ state: 'queued', hostWait: true }).includes('<p class="tw-run-detail">Helper is offline and will reply when it’s back online</p>'));
  assert.ok(draw({ state: 'queued', hostWait: true }, 'zh-Hans').includes('<p class="tw-run-detail">Helper 已离线，上线后会回复</p>'));
  assert.ok(draw({ state: 'queued', hostWait: true }, 'zh-Hant').includes('<p class="tw-run-detail">Helper 已離線，上線後會回覆</p>'));
  assert.ok(draw({ state: 'queued' }).includes('<p class="tw-run-detail">Waiting for the Bot’s Desktop to pick this up.</p>'));
  assert.ok(draw({ state: 'expired', hostWait: true }).includes('<p class="tw-run-detail">The Bot’s Desktop didn’t pick this up in time. Send it again.</p>'));
  assert.ok(draw({ state: 'running', hostWait: true }).includes('The Bot is working on it.'));
  // The one who asked can cancel a task that is still going: the second action, beside the document.
  viewer = { uid: 'alice', bots: [], directory: [], pending: [] } as never;
  assert.match(draw({ state: 'queued' }), /<button type="button" aria-label="Cancel task">Cancel<\/button>/); assert.doesNotMatch(draw({ state: 'completed' }), /<button/);
  assert.match(draw({ state: 'awaiting_approval' }), /<button type="button" aria-label="Cancel task">Cancel<\/button>/, 'the one who asked may take back a request whose reply waits for approval');
  const everyLanguage = ['en', 'zh-Hans', 'zh-Hant', 'ja', 'ko'].map(locale => teamsText(locale));
  assert.ok(Object.values(RUN_STATES).every(look => look.icon && everyLanguage.every(t => t(look.label) !== look.label && (!look.detail || t(look.detail) !== look.detail))), 'every row has its words in all five languages and a glyph');
  assert.equal(RUN_STATES.cancelled!.detail, undefined, 'the cancelled row has no detail cell');
  const unknown = draw({ state: 'archived' }); assert.match(unknown, /data-state="archived" data-tone="muted"/); assert.ok(unknown.includes('<span>Unknown</span>')); assert.doesNotMatch(unknown, /tw-run-detail/);
  assert.equal(runLook('constructor').label, 'chat_status_unknown', 'a state is looked up by what the table holds, not by what an object inherits'); assert.equal(runLook('__proto__').label, 'chat_status_unknown'); assert.equal(runLook('toString').tone, 'muted');
});

scenario('Changed member generation invalidates a confirmed publication review', async context => {
  let sends = 0;
  const { fake, workspace } = await open(context, { localBots: async () => [{ instanceId: 'local-new', name: 'Travel', description: '' }], selectSharedFiles: async () => [], publishLocalBot: async () => { sends++; return { sharedBotId: 'shared-new', revision: 1 }; } });
  const page = await mount(workspace, directory(workspace)); context.after(() => page.unmount());
  await page.click(page.button('Load local Bots')); await page.type(page.find(n => n.type === 'select')[0], 'local-new'); await page.type(page.find(n => n.type === 'textarea')[0], 'Trip role'); await page.check(page.find(n => n.type === 'input' && n.props.type === 'checkbox')[0]); await page.submit(); await page.check(page.find(n => n.type === 'input' && n.props.type === 'checkbox').at(-1)); assert.equal(page.button('Publish Bot').props.disabled, false);
  const detail = await fake.detail('team-test'); detail.members = detail.members.map(member => member.uid === 'bob' ? { ...member, generation: 2 } : member); fake.details.set('team-test', detail);
  await page.during(() => workspace.refresh()); assert.equal(page.button('Publish Bot').props.disabled, true); assert.match(page.text(), /memberships changed/); assert.equal(sends, 0);
});
scenario('Foreground task polling updates queued status within its one-second observation interval', async context => {
  const { fake, workspace } = await open(context);
  let state = 'queued', polls = 0, commits = 0;
  const deadline = Date.now() + 300_000;
  fake.messages = async <T>(command: MessageCommand) => {
    if (command.action !== 'listTasks') return { bots: fake.bots } as T;
    polls++;
    return { tasks: [{ taskId: 'task-test', conversationId: 'test-chat', sharedBotId: 'assistant-fixture', requester: { uid: 'alice', generation: 1 }, state, createdAt: 1, deadline }] } as T;
  };
  context.mock.timers.enable({ apis: ['setInterval'] });
  const page = await mount(workspace, snapshot => createElement(Profiler, { id: 'team-messages', onRender: () => { commits++; } },
    createElement(TeamConversationView, { workspace, snapshot, locale: 'en' })));
  context.after(() => page.unmount());
  assert.match(page.text(), /Queued/);
  const previousPolls = polls, previousCommits = commits;
  await page.during(() => { context.mock.timers.tick(1000); });
  assert.equal(polls, previousPolls + 1);
  assert.equal(commits, previousCommits, 'an identical response does not commit a render');
  state = 'running';
  await page.during(() => { context.mock.timers.tick(1000); }); assert.match(page.text(), /Processing/);
});
scenario('A task without a reply message is a Bot row inside the message log, with the same wrapper as a delivered one', async context => {
  const { fake, workspace } = await open(context); let state = 'queued';
  fake.messages = async <T>(command: MessageCommand) => command.action === 'listTasks' ? { tasks: [{ taskId: 'task-test', conversationId: 'test-chat', sharedBotId: 'assistant-fixture', requester: { uid: 'alice', generation: 1 }, state, createdAt: 1, deadline: Date.now() + 300000 }] } as T : { bots: fake.bots } as T;
  context.mock.timers.enable({ apis: ['setInterval'] });
  fake.conversations.set('team-test', [conversation('test-chat')]); await workspace.refresh();
  workspace.selectConversation('test-chat'); await until(() => workspace.getSnapshot().selectedConversationId === 'test-chat', 'selection');
  const page = await mount(workspace, snapshot => createElement(TeamConversationView, { workspace, snapshot, locale: 'en' })); context.after(() => page.unmount());
  for (const [next, label] of [['queued', /Queued/], ['interrupted', /Interrupted/]] as const) {
    state = next; await page.during(() => { context.mock.timers.tick(1000); });
    const row = page.find(n => n.props['data-task-id'] === 'task-test')[0]; assert.ok(row, `the ${next} task has a row`);
    assert.match(textOf(row), label); assert.equal(row.type, 'li');
    assert.ok(page.find(n => n.props.role === 'log')[0].findAll((n: any) => n === row).length > 0, 'inside the message log');
    assert.ok(row.findAll((n: any) => n.props?.className === 'tw-embed').length > 0, 'status sits in a .tw-embed wrapper like a delivered task');
    assert.equal(page.find(n => n.props.role === 'status' && label.test(textOf(n))).length, 1, 'and nowhere else');
  }
});
scenario('A task without a reply sits right after the message that requested it, not below newer messages', async context => {
  const { fake, workspace } = await open(context);
  const at = (day: number, hour: number, minute: number) => Date.UTC(2026, 10, day, hour, minute, 0);
  const iso = (ms: number) => new Date(ms).toISOString();
  const task = (taskId: string, state: string, createdAt: number) => ({ taskId, conversationId: 'test-chat', sharedBotId: 'assistant-fixture', requester: { uid: 'alice', generation: 1 }, state, createdAt, deadline: createdAt + 300000 });
  fake.messages = async <T>(command: MessageCommand) => command.action === 'listTasks' ? { tasks: [
    task('task-live', 'running', at(6, 9, 1)), task('task-failed', 'failed', at(6, 9, 0)), task('task-old', 'expired', at(2, 10, 1)),
  ] } as T : { bots: fake.bots } as T;
  fake.conversations.set('team-test', [conversation('test-chat', { lastSeq: 4, lastReadSeq: 4 })]);
  fake.threads.set('test-chat', [
    message(1, { text: 'Monday hello', createdAt: iso(at(2, 10, 0)) }),
    message(2, { text: 'Monday ask the Bot', author: { kind: 'user', uid: 'alice' }, createdAt: iso(at(2, 10, 1)) }),
    message(3, { text: 'Friday ask', createdAt: iso(at(6, 9, 0)) }),
    message(4, { text: 'Friday follow-up', createdAt: iso(at(6, 9, 1)) }),
  ]);
  context.mock.timers.enable({ apis: ['setInterval'] });
  await workspace.refresh(); workspace.selectConversation('test-chat'); await until(() => workspace.getSnapshot().selectedConversationId === 'test-chat' && workspace.getSnapshot().messages.length === 4, 'messages');
  const page = await mount(workspace, snapshot => createElement(TeamConversationView, { workspace, snapshot, locale: 'en' })); context.after(() => page.unmount());
  await page.during(() => { context.mock.timers.tick(1000); });
  const log = page.find(n => n.props.role === 'log')[0];
  const items = log.findAll((n: any) => n.type === 'li' && (n.props['data-message-id'] || n.props['data-task-id']));
  assert.deepEqual(items.map((n: any) => n.props['data-message-id'] ?? n.props['data-task-id']), ['message-1', 'message-2', 'task-old', 'message-3', 'task-failed', 'message-4', 'task-live']);
  assert.equal(items[5].props['data-grouped'], false, 'a message after a Bot row does not continue the run before it');
  assert.equal(items.filter((n: any) => n.props['data-task-id']).length, 3, 'one row per task');
});
scenario('Imported Bot publication binds required resources to the installer’s own selected files', async context => {
  const sent: unknown[] = [];
  const { workspace } = await open(context, { localBots: async () => [{ instanceId: 'imported-new', name: 'Imported travel', description: '', requiredResources: ['family-notes'] }], selectSharedFiles: async () => [{ token: 'installer-file-token', kind: 'file', name: 'My family notes.txt', bytes: 12, sha256: 'a'.repeat(64), mediaType: 'text/plain' }], publishLocalBot: async input => { sent.push(input); return { sharedBotId: input.sharedBotId, revision: 1 }; } });
  const page = await mount(workspace, directory(workspace)); context.after(() => page.unmount()); await page.click(page.button('Load local Bots')); await page.type(page.find(n => n.type === 'select')[0], 'imported-new'); await page.type(page.find(n => n.type === 'textarea')[0], 'My family role'); assert.equal(page.button('Review publication').props.disabled, true);
  await page.click(page.button('Select read-only files / SKILL.md')); assert.equal(page.button('Review publication').props.disabled, true, 'picking files alone never inherits author bindings');
  await page.type(page.find(n => n.type === 'select' && n.props.required && n.props.value === '')[0], 'installer-file-token'); assert.equal(page.button('Review publication').props.disabled, false); await page.submit(); await page.check(page.find(n => n.type === 'input' && n.props.type === 'checkbox').at(-1)); await page.click(page.button('Publish Bot')); assert.deepEqual((sent[0] as Record<string, unknown>).resourceBindings, { 'family-notes': 'installer-file-token' });
});
scenario('Task observer drops previously visible tasks after membership denial', async context => {
  const { fake, workspace } = await open(context); let denied = false;
  fake.messages = async <T>(command: MessageCommand) => { if (command.action === 'listTasks') { if (denied) throw new Error('MEMBERSHIP_REQUIRED'); return { tasks: [{ taskId: 'task-test', conversationId: 'test-chat', sharedBotId: 'assistant-fixture', requester: { uid: 'alice', generation: 1 }, state: 'queued', createdAt: 1, deadline: Date.now() + 300000 }] } as T; } return { bots: fake.bots } as T; };
  const page = await mount(workspace, snapshot => createElement(TeamConversationView, { workspace, snapshot, locale: 'en' })); context.after(() => page.unmount()); assert.match(page.text(), /Queued/); denied = true; await page.during(() => { fake.signal({ conversationId: 'test-chat' }); }); assert.doesNotMatch(page.text(), /Queued/); assert.match(page.text(), /Access changed/);
});

scenario('Publication keeps successive native selections and cancellation, including distinct same-name Skills', async context => {
  const file = (token: string, name: string, hash: string, kind: 'file' | 'skill') => ({ token, name, kind, bytes: 12, sha256: hash.repeat(64), mediaType: 'text/plain' });
  const notes = file('notes-token', 'notes.txt', 'a', 'file');
  const firstSkill = file('skill-one', 'SKILL.md', 'b', 'skill');
  const secondSkill = file('skill-two', 'SKILL.md', 'c', 'skill');
  const selections = [[notes, firstSkill], [firstSkill, secondSkill], []];
  const published: unknown[] = [];
  const { workspace } = await open(context, {
    localBots: async () => [{ instanceId: 'local-new', name: 'Travel', description: '' }],
    selectSharedFiles: async () => selections.shift()!,
    publishLocalBot: async input => { published.push(input); return { sharedBotId: input.sharedBotId, revision: 1 }; },
  });
  const page = await mount(workspace, directory(workspace)); context.after(() => page.unmount());
  await page.click(page.button('Load local Bots'));
  await page.type(page.find(n => n.type === 'select')[0], 'local-new');
  await page.type(page.find(n => n.type === 'textarea')[0], 'Read the notes and both Skills.');
  for (let pick = 0; pick < 3; pick++) await page.click(page.button('Select read-only files / SKILL.md'));
  await page.submit();
  await page.check(page.find(n => n.type === 'input' && n.props.type === 'checkbox').at(-1));
  await page.click(page.button('Publish Bot'));
  assert.deepEqual((published[0] as Record<string, unknown>).fileTokens, ['notes-token', 'skill-one', 'skill-two']);
});

// ── Waiting tasks, answers and cancellation ──
const botAuthor = { kind: 'shared_bot' as const, sharedBotId: 'assistant-fixture', displayName: 'Fixture Bot' };
const waitTask = (more: Record<string, unknown> = {}, recipient = { uid: 'alice', generation: 1 }) => ({
  taskId: 'task-test', conversationId: 'test-chat', sharedBotId: 'assistant-fixture', requester: recipient, state: 'waiting', createdAt: 1, deadline: Date.now() + 240000, epoch: 1, activeSpentMs: 1000,
  wait: { waitId: 'wait-test', kind: 'human_input', recipient, expiresAt: Date.now() + 240000, messageId: 'message-2' }, ...more,
});
/** A Bot DM with a Bot question in it and `tasks()` as the task list; `handle` answers every other command. The caller has set up the DOM once (`withDom`). */
async function openChat(context: Parameters<Parameters<typeof scenario>[1]>[0], tasks: () => unknown[], handle: (command: MessageCommand) => unknown = () => ({}), maintainer = 'alice', locale = 'en',
  thread: { second?: Partial<TeamMessage>; then?: TeamMessage[] } = {}) {
  const fake = new FakeTeams('alice'); fake.teams = [team('team-test', { role: 'owner' })]; const sent: MessageCommand[] = [];
  const workspace = new TeamsWorkspace(fake.api, 'alice', { storage: memoryStorage, environment: new EventTarget() }); context.after(() => workspace.close());
  fake.bots = [botCard({ maintainer: { uid: maintainer, generation: 1 } })];
  fake.messages = async <T>(command: MessageCommand) => {
    sent.push(structuredClone(command));
    if (command.action === 'listTasks') return { tasks: tasks() } as T;
    if (command.action === 'listSharedBots') return { bots: fake.bots } as T;
    // The maintainer's view of the Chat also reads the replies of their Bot that wait for them; none do here.
    if (command.action === 'listHeldReplies') return { replies: [] } as T;
    return handle(command) as T;
  };
  const messages = [message(1, { text: 'Plan my trip', author: { kind: 'user', uid: 'alice' } }), message(2, { text: 'Which city?', author: botAuthor, waitId: 'wait-test', taskId: 'task-test', ...thread.second }), ...(thread.then ?? [])];
  fake.conversations.set('team-test', [conversation('test-chat', { kind: 'bot_dm', participants: ['user:alice', 'shared_bot:assistant-fixture'], lastSeq: messages.length })]);
  fake.threads.set('test-chat', messages);
  await workspace.refresh(); await workspace.refreshBots(); workspace.selectConversation('test-chat');
  await until(() => workspace.getSnapshot().selectedConversationId === 'test-chat' && workspace.getSnapshot().messages.length === messages.length && workspace.getSnapshot().botsLoaded, 'the chat');
  const page = await mount(workspace, snapshot => createElement(TeamConversationView, { workspace, snapshot, locale })); context.after(() => page.unmount());
  const buttons = (text: string) => page.find(n => n.type === 'button' && textOf(n) === text);
  const box = page.messageEditor;
  return { fake, workspace, page, sent, buttons, box };
}
scenario('Bot replies hide automatic copies and keep full or revised documents accessible', async context => {
  withDom(context);
  const reply = '{"probeId":"fixture"}';
  const documents = [
    { artifactId: 'artifact-copy', revision: 1, text: reply, body: reply },
    { artifactId: 'artifact-full', revision: 1, text: 'Result saved as a shared document.', body: reply },
    { artifactId: 'artifact-revised', revision: 2, text: reply, body: reply },
  ];
  const chat = await openChat(context, () => [waitTask({ state: 'completed', wait: undefined, artifactId: 'artifact-copy', replyId: 'message-2' })], command => {
    if (command.action !== 'readArtifact') return {};
    const item = documents.find(document => document.artifactId === command.payload.artifactId)!;
    return { artifactId: item.artifactId, revision: item.revision, currentRevision: item.revision, document: { title: 'Shared Bot result', body: item.body } };
  }, 'alice', 'en', {
    second: { waitId: undefined, text: reply, artifactId: 'artifact-copy' },
    then: documents.slice(1).map((item, index) => message(index + 3, { author: botAuthor, text: item.text, artifactId: item.artifactId })),
  });
  await until(() => chat.sent.filter(command => command.action === 'readArtifact').length >= 3 && chat.buttons('Open document').length === 2);
  assert.match(chat.page.text(), /Completed/);
  assert.match(chat.page.text(), /"probeId":"fixture"/);
  const copy = chat.page.find(node => node.type === 'li' && node.props['data-message-id'] === 'message-2')[0];
  assert.doesNotMatch(textOf(copy), /Open document/);
  await chat.page.click(chat.buttons('Open document')[0]);
  assert.match(chat.page.text(), /Shared Bot result/);
  assert.match(chat.page.text(), /Revision 1/);
});
scenario('A waiting task names who it waits for and when the wait ends; only that person is offered Reply', async context => {
  withDom(context);
  const mine = await openChat(context, () => [waitTask()]);
  assert.match(mine.page.text(), /Waiting for a reply/); assert.match(mine.page.text(), /Waiting for you · expires in 4 min/);
  assert.equal(mine.buttons('Reply').length, 2, 'on the card and on the question');
  assert.ok(mine.page.find(n => n.type === 'span' && classes(n).includes('tw-tag') && textOf(n) === 'Question').length === 1, 'the question is marked');
  const other = await openChat(context, () => [waitTask({}, { uid: 'bob', generation: 1 })]);
  assert.match(other.page.text(), /Waiting for Bob · expires in 4 min/); assert.equal(other.buttons('Reply').length, 0);
  const stale = await openChat(context, () => [waitTask({}, { uid: 'alice', generation: 9 })]);
  assert.match(stale.page.text(), /Waiting for Alice/, 'the same account at another membership generation is not the recipient'); assert.equal(stale.buttons('Reply').length, 0);
  const chinese = await openChat(context, () => [waitTask({}, { uid: 'bob', generation: 1 })], () => ({}), 'alice', 'zh-Hans');
  assert.match(chinese.page.text(), /等待 Bob 回复 · 4 分钟后过期/); assert.doesNotMatch(chinese.page.text(), /Waiting for/);
  const mineZh = await openChat(context, () => [waitTask()], () => ({}), 'alice', 'zh-Hans');
  assert.match(mineZh.page.text(), /等待你回复/); assert.match(mineZh.page.text(), /回复/); assert.ok(mineZh.buttons('回复').length === 2 && mineZh.buttons('取消').length === 1);
});
scenario('Replying to a question answers its wait with fresh ids and never submits a Bot request', async context => {
  withDom(context);
  const chat = await openChat(context, () => [waitTask()]);
  await chat.page.click(chat.buttons('Reply')[0]);
  assert.match(chat.page.text(), /Replying to Fixture Bot’s question: Which city\?/);
  await chat.page.type(chat.box(), '@Fixture Bot Lisbon');
  await chat.page.submit();
  const answers = chat.sent.filter(c => c.action === 'answerWait');
  assert.equal(answers.length, 1); assert.equal(chat.sent.filter(c => c.action === 'submit').length, 0, 'the text names a Bot, and still asks nothing');
  const command = answers[0]!; if (command.action !== 'answerWait') return;
  assert.deepEqual([command.payload.conversationId, command.payload.waitId, command.payload.text], ['test-chat', 'wait-test', '@Fixture Bot Lisbon']);
  assert.notEqual(command.payload.messageId, command.payload.changeId);
  assert.doesNotMatch(chat.page.text(), /Replying to/, 'answer mode ends once the answer is accepted'); assert.equal(chat.page.editorText(), '');
});
scenario('The question message offers the same Reply, and Escape or the dismiss control leaves answer mode', async context => {
  withDom(context);
  const chat = await openChat(context, () => [waitTask()]);
  await chat.page.click(chat.buttons('Reply')[1]); assert.match(chat.page.text(), /Replying to Fixture Bot’s question/);
  await chat.page.click(chat.page.oneByLabel('Cancel reply')); assert.doesNotMatch(chat.page.text(), /Replying to/);
  await chat.page.click(chat.buttons('Reply')[0]); assert.match(chat.page.text(), /Replying to/);
  assert.equal(await chat.page.key(chat.box(), 'Escape'), true); assert.doesNotMatch(chat.page.text(), /Replying to/);
});
scenario('A closed wait ends answer mode, keeps the typed text and says why', async context => {
  withDom(context);
  const chat = await openChat(context, () => [waitTask()], command => { if (command.action === 'answerWait') throw new Error('WAIT_CLOSED'); return {}; });
  await chat.page.click(chat.buttons('Reply')[0]); await chat.page.type(chat.box(), 'Lisbon'); await chat.page.submit();
  assert.doesNotMatch(chat.page.text(), /Replying to/); assert.equal(chat.page.editorText(), 'Lisbon');
  assert.match(chat.page.text(), /This question is no longer waiting for a reply/);
  assert.equal(chat.page.find(n => n.props.className?.includes?.('tw-pending')).length, 0, 'a refused answer is not left in the queue');
});
scenario('Answer mode ends by itself when the task stops waiting', async context => {
  withDom(context);
  let state = 'waiting';
  const chat = await openChat(context, () => [state === 'waiting' ? waitTask() : waitTask({ state: 'queued', wait: undefined })]);
  await chat.page.click(chat.buttons('Reply')[0]); await chat.page.type(chat.box(), 'Lisbon'); assert.match(chat.page.text(), /Replying to/);
  state = 'queued'; chat.fake.signal({ conversationId: 'test-chat' }); await chat.page.during(() => undefined);
  assert.doesNotMatch(chat.page.text(), /Replying to/); assert.equal(chat.buttons('Reply').length, 0); assert.equal(chat.page.editorText(), 'Lisbon');
});
// ── Approving a list change ──
const proposal = { second: { text: 'Asks to change the list “Groceries” (3 changes).', approval: { title: 'Groceries', changes: [{ op: 'add' as const, text: 'Bread' },
  { op: 'update' as const, itemId: 'item_0001', done: true, item: 'Milk' }, { op: 'remove' as const, itemId: 'item_0002', item: null }] } } };
const approvalTask = (recipient = { uid: 'alice', generation: 1 }) => waitTask({ wait: { waitId: 'wait-test', kind: 'tool_approval', recipient, expiresAt: Date.now() + 240000, messageId: 'message-2' } }, recipient);
scenario('A proposed list change shows exactly what would change, and only the member the Bot works for can approve or decline it', async context => {
  withDom(context);
  const mine = await openChat(context, () => [approvalTask()], () => ({}), 'alice', 'en', proposal);
  for (const line of [/Change the list “Groceries”/, /Add “Bread”/, /Mark “Milk” as done/, /Remove an item that is no longer on the list/, /Waiting for approval/, /Nothing is changed unless you approve/]) assert.match(mine.page.text(), line);
  assert.deepEqual([mine.buttons('Approve').length, mine.buttons('Decline').length, mine.buttons('Reply').length], [1, 1, 0], 'an approval is never answered in words');
  const tags = mine.page.find(n => n.type === 'span' && classes(n).includes('tw-tag')).map(textOf); assert.ok(tags.includes('Approval') && !tags.includes('Question'));
  const other = await openChat(context, () => [approvalTask({ uid: 'bob', generation: 1 })], () => ({}), 'alice', 'en', proposal);
  assert.match(other.page.text(), /Waiting for Bob to approve or decline/); assert.deepEqual([other.buttons('Approve').length, other.buttons('Decline').length], [0, 0]);
  const stale = await openChat(context, () => [approvalTask({ uid: 'alice', generation: 9 })], () => ({}), 'alice', 'en', proposal);
  assert.equal(stale.buttons('Approve').length, 0, 'the same account at another membership generation cannot approve');
  const chinese = await openChat(context, () => [approvalTask()], () => ({}), 'alice', 'zh-Hans', proposal);
  assert.match(chinese.page.text(), /请求修改清单“Groceries”。/); assert.match(chinese.page.text(), /添加“Bread”/); assert.match(chinese.page.text(), /等待批准/);
  assert.deepEqual([chinese.buttons('批准').length, chinese.buttons('拒绝').length], [1, 1]); assert.doesNotMatch(chinese.page.text(), /Asks to change/);
});
scenario('A decision is one command however often it is sent, and a request that stopped waiting says so', async context => {
  withDom(context);
  let failure = 'SERVICE_UNAVAILABLE';
  const chat = await openChat(context, () => [approvalTask()], command => { if (command.action === 'decideWait' && failure) throw new Error(failure); return {}; }, 'alice', 'en', proposal);
  await chat.page.click(chat.buttons('Approve')[0]); assert.equal(chat.page.find(n => n.props.role === 'alert').length, 1, 'a failed decision is reported');
  failure = ''; await chat.page.click(chat.buttons('Approve')[0]); assert.equal(chat.page.find(n => n.props.role === 'alert').length, 0);
  const decisions = chat.sent.filter(c => c.action === 'decideWait'); assert.equal(decisions.length, 2);
  assert.deepEqual(decisions[0], decisions[1], 'the retry is the same command, so the reply cannot be posted twice');
  const command = decisions[0]!; if (command.action !== 'decideWait') return;
  assert.deepEqual([command.payload.conversationId, command.payload.waitId, command.payload.decision], ['test-chat', 'wait-test', 'approve']); assert.notEqual(command.payload.messageId, command.payload.changeId);
  assert.equal(chat.sent.filter(c => c.action === 'answerWait' || c.action === 'submit').length, 0);
  failure = 'WAIT_CLOSED'; await chat.page.click(chat.buttons('Decline')[0]);
  assert.match(chat.page.text(), /This request is no longer waiting for a decision/);
  const declined = chat.sent.filter(c => c.action === 'decideWait').at(-1)!; if (declined.action !== 'decideWait') return;
  assert.equal(declined.payload.decision, 'deny'); assert.notEqual(declined.commandId, command.commandId);
});
scenario('After the decision the request says how it ended, and the reply can be neither edited nor recalled', async context => {
  withDom(context);
  const reply = (decision: 'approve' | 'deny') => [message(3, { text: decision === 'approve' ? 'Approved' : 'Declined', author: { kind: 'user', uid: 'alice' }, waitId: 'wait-test', replyTo: 'message-2', decision })];
  const done = { ...waitTask({ state: 'queued', wait: undefined }) };
  const approved = await openChat(context, () => [done], () => ({}), 'alice', 'en', { ...proposal, then: reply('approve') });
  assert.match(approved.page.text(), /Approved by /); assert.deepEqual([approved.buttons('Approve').length, approved.buttons('Decline').length], [0, 0]);
  assert.deepEqual([approved.page.byLabel('Edit').length, approved.page.byLabel('Recall').length], [1, 1], 'only the request Alice typed herself can be edited or recalled, not her decision');
  const declined = await openChat(context, () => [done], () => ({}), 'alice', 'en', { ...proposal, then: reply('deny') });
  assert.match(declined.page.text(), /Declined by .*Nothing was changed/);
  const closed = await openChat(context, () => [waitTask({ state: 'expired', wait: undefined })], () => ({}), 'alice', 'en', proposal);
  assert.match(closed.page.text(), /This request ended without a decision\. Nothing was changed/); assert.equal(closed.buttons('Approve').length, 0);
  const chinese = await openChat(context, () => [done], () => ({}), 'alice', 'zh-Hans', { ...proposal, then: reply('approve') });
  assert.match(chinese.page.text(), /已批准/); assert.doesNotMatch(chinese.page.text(), /Approved/);
});
scenario('Cancel is for the requester and the Bot maintainer on a queued, running or waiting task', async context => {
  withDom(context);
  const task = (state: string, requester = 'alice', more: Record<string, unknown> = {}) => ({ ...waitTask({ state, ...more }, { uid: requester, generation: 1 }), ...(state === 'waiting' ? {} : { wait: undefined }) });
  for (const state of ['queued', 'running', 'waiting']) {
    const chat = await openChat(context, () => [task(state)], () => ({}), 'owner');
    assert.equal(chat.buttons('Cancel').length, 1, `the requester can cancel a ${state} task`);
  }
  const maintained = await openChat(context, () => [task('running', 'bob')], () => ({}), 'alice');
  assert.equal(maintained.buttons('Cancel').length, 1, 'the maintainer can cancel another member’s task');
  const stranger = await openChat(context, () => [task('running', 'bob')], () => ({}), 'owner');
  assert.equal(stranger.buttons('Cancel').length, 0);
  for (const state of ['completed', 'failed', 'expired', 'cancelled', 'interrupted']) assert.equal((await openChat(context, () => [task(state)])).buttons('Cancel').length, 0, `no Cancel on a ${state} task`);
});
scenario('Cancel sends cancelTask; a task being cancelled says so without a button; a finished task is refreshed quietly', async context => {
  withDom(context);
  let result: unknown = { taskId: 'task-test', state: 'running' }, failure = '';
  const chat = await openChat(context, () => [waitTask({ state: 'running', wait: undefined })], command => { if (command.action === 'cancelTask') { if (failure) throw new Error(failure); return result; } return {}; });
  await chat.page.click(chat.buttons('Cancel')[0]);
  assert.deepEqual(chat.sent.filter(c => c.action === 'cancelTask').map(c => c.action === 'cancelTask' ? c.payload : null), [{ taskId: 'task-test' }]);
  const before = chat.sent.filter(c => c.action === 'listTasks').length; failure = 'TASK_FINISHED';
  await chat.page.click(chat.buttons('Cancel')[0]); assert.equal(chat.page.find(n => n.props.role === 'alert').length, 0);
  assert.ok(chat.sent.filter(c => c.action === 'listTasks').length > before, 'the tasks were fetched again');
  failure = 'TASK_CANCEL_FORBIDDEN'; await chat.page.click(chat.buttons('Cancel')[0]); assert.match(chat.page.text(), /Only the person who asked, or the Bot’s maintainer, can cancel this task/);
  const stopping = await openChat(context, () => [waitTask({ state: 'running', wait: undefined, cancelRequestedAt: Date.now() })]);
  assert.match(stopping.page.text(), /Cancelling…/); assert.doesNotMatch(stopping.page.text(), /Processing/); assert.equal(stopping.buttons('Cancel').length, 0);
});
scenario('A cancelled task reads Cancelled and shows its usage only when it has one', async context => {
  withDom(context);
  const plain = await openChat(context, () => [waitTask({ state: 'cancelled', wait: undefined })]);
  assert.match(plain.page.text(), /Cancelled/); assert.doesNotMatch(plain.page.text(), /usage/);
  const used = await openChat(context, () => [waitTask({ state: 'cancelled', wait: undefined, usage: { status: 'partial', inputTokens: 3, outputTokens: 4 } })]);
  assert.match(used.page.text(), /Cancelled/); assert.match(used.page.text(), /Partial usage · 7 tokens · cost unknown/);
});
/** A finished task with a shared document whose revisions live in `store`; every command goes through `handle` first. */
async function openDocument(context: Parameters<Parameters<typeof scenario>[1]>[0], handle: (command: MessageCommand) => unknown = () => undefined) {
  const store = { revisions: [{ title: 'Trip preparation', body: 'Pack passports' }] }, saved = new Map<string, number>();
  const view = (revision = store.revisions.length) => ({ artifactId: 'artifact-test', revision, digest: 'a'.repeat(64), document: store.revisions[revision - 1], currentRevision: store.revisions.length });
  const chat = await openChat(context, () => [waitTask({ state: 'completed', wait: undefined, artifactId: 'artifact-test' })], command => {
    const handled = handle(command); if (handled !== undefined) return handled;
    if (command.action === 'readArtifact') return view(command.payload.revision);
    if (command.action === 'reviseArtifact') {
      const again = saved.get(command.payload.operationId); if (again) return { artifactId: 'artifact-test', revision: again, digest: 'b'.repeat(64) };
      if (command.payload.expectedRevision !== store.revisions.length) throw new Error('ARTIFACT_REVISION_CONFLICT');
      store.revisions.push(command.payload.document); saved.set(command.payload.operationId, store.revisions.length); return { artifactId: 'artifact-test', revision: store.revisions.length, digest: 'b'.repeat(64) };
    }
    if (command.action === 'listArtifactRevisions') return { artifactId: 'artifact-test', currentRevision: store.revisions.length, revisions: store.revisions.map((document, i) => ({ revision: i + 1, parentRevision: i || null, digest: 'c'.repeat(64), title: document.title, author: i ? { kind: 'user', uid: 'alice', generation: 1 } : { kind: 'shared_bot', sharedBotId: 'assistant-fixture' }, createdAt: 1700000000000 + i })).reverse() };
    return {};
  });
  await chat.page.click(chat.page.button('Open document'));
  const editor = () => ({ title: chat.page.find(n => n.type === 'input' && n.props.maxLength === 120)[0], body: chat.page.find(n => n.type === 'textarea' && n.props.rows === 14)[0], form: chat.page.find(n => n.type === 'form' && classes(n).includes('tw-doc-edit'))[0] });
  const revised = () => chat.sent.flatMap(c => c.action === 'reviseArtifact' ? [c.payload] : []);
  return { ...chat, store, editor, revised };
}
scenario('A shared document is edited on top of the revision that was read and shows the saved revision', async context => {
  withDom(context);
  const doc = await openDocument(context);
  assert.match(doc.page.text(), /Revision 1/); await doc.page.click(doc.page.button('Edit'));
  assert.equal(doc.page.button('Save').props.disabled, true, 'nothing changed yet');
  await doc.page.type(doc.editor().body, 'Pack passports and chargers'); await doc.page.submit(doc.editor().form);
  assert.deepEqual(doc.revised().map(({ operationId, ...rest }) => rest), [{ artifactId: 'artifact-test', expectedRevision: 1, document: { title: 'Trip preparation', body: 'Pack passports and chargers' } }]);
  assert.match(doc.page.text(), /Revision 2/); assert.match(doc.page.text(), /Pack passports and chargers/); assert.equal(doc.editor().form, undefined, 'back to reading');
});
scenario('A save whose answer was lost is repeated with the same operation id; a different text gets a new one', async context => {
  withDom(context);
  let lose = false;
  const doc = await openDocument(context, command => { if (command.action === 'readArtifact' && lose) { lose = false; throw new Error('SERVICE_UNAVAILABLE'); } return undefined; });
  await doc.page.click(doc.page.button('Edit')); await doc.page.type(doc.editor().body, 'Second'); lose = true; await doc.page.submit(doc.editor().form);
  assert.match(doc.page.text(), /Could not sync messages/); assert.equal(doc.editor().body.props.value, 'Second', 'the text is kept');
  await doc.page.submit(doc.editor().form);
  assert.equal(doc.revised().length, 2); assert.equal(doc.revised()[0].operationId, doc.revised()[1].operationId);
  assert.equal(doc.store.revisions.length, 2, 'one revision was created'); assert.match(doc.page.text(), /Revision 2/);
  await doc.page.click(doc.page.button('Edit')); await doc.page.type(doc.editor().body, 'Third'); await doc.page.submit(doc.editor().form);
  assert.notEqual(doc.revised()[2].operationId, doc.revised()[0].operationId); assert.equal(doc.revised()[2].expectedRevision, 2);
});
scenario('An edit made on an outdated revision is not saved: the text stays beside the newer version until the person decides', async context => {
  withDom(context);
  const doc = await openDocument(context);
  await doc.page.click(doc.page.button('Edit')); await doc.page.type(doc.editor().body, 'My packing list');
  doc.store.revisions.push({ title: 'Trip preparation', body: 'Bob changed this' });
  await doc.page.submit(doc.editor().form);
  assert.equal(doc.store.revisions.length, 2, 'nothing was overwritten');
  assert.match(doc.page.text(), /was changed while you were editing \(now revision 2\)/); assert.match(doc.page.text(), /Bob changed this/);
  assert.equal(doc.editor().body.props.value, 'My packing list');
  assert.equal(doc.page.button('Save over revision 2').props.disabled, false); await doc.page.submit(doc.editor().form);
  assert.equal(doc.revised().at(-1)!.expectedRevision, 2); assert.notEqual(doc.revised().at(-1)!.operationId, doc.revised()[0].operationId);
  assert.match(doc.page.text(), /Revision 3/); assert.match(doc.page.text(), /My packing list/);
  await doc.page.click(doc.page.button('Edit')); await doc.page.type(doc.editor().body, 'Lost on purpose');
  doc.store.revisions.push({ title: 'Trip preparation', body: 'Carol changed this' }); await doc.page.submit(doc.editor().form);
  await doc.page.click(doc.page.button('Discard my changes'));
  assert.match(doc.page.text(), /Revision 4/); assert.match(doc.page.text(), /Carol changed this/); assert.equal(doc.store.revisions.length, 4);
});
scenario('Closing a document with unsaved changes asks first; an invalid edit cannot be saved', async context => {
  withDom(context);
  const doc = await openDocument(context);
  await doc.page.click(doc.page.button('Edit')); await doc.page.type(doc.editor().title, '   ');
  assert.equal(doc.page.button('Save').props.disabled, true, 'a blank title');
  await doc.page.type(doc.editor().title, 'Trip'); await doc.page.type(doc.editor().body, 'x'.repeat(256 * 1024 + 1));
  assert.equal(doc.page.button('Save').props.disabled, true); assert.match(doc.page.text(), /too long to save/);
  await doc.page.type(doc.editor().body, 'Shorter');
  const dialog = doc.page.find(n => n.type === 'dialog')[0];
  await doc.page.during(() => dialog.props.onCancel({ preventDefault() {} }));
  assert.match(doc.page.text(), /You have unsaved changes/); await doc.page.click(doc.page.button('Keep editing')); assert.equal(doc.editor().body.props.value, 'Shorter');
  await doc.page.during(() => doc.page.find(n => n.type === 'dialog')[0].props.onCancel({ preventDefault() {} })); await doc.page.click(doc.page.button('Discard changes'));
  assert.equal(doc.page.find(n => n.type === 'dialog').length, 0); assert.equal(doc.revised().length, 0);
});
scenario('Document history names who wrote each revision; an earlier revision is read-only', async context => {
  withDom(context);
  const doc = await openDocument(context);
  doc.store.revisions.push({ title: 'Trip preparation', body: 'Second version' });
  await doc.page.click(doc.page.button('History'));
  assert.match(doc.page.text(), /Document history/); assert.match(doc.page.text(), /Alice · .* · current/); assert.match(doc.page.text(), /Fixture Bot · /);
  await doc.page.click(doc.page.oneByLabel('Open revision 1'));
  assert.match(doc.page.text(), /Revision 1 of 2 · an earlier version/); assert.match(doc.page.text(), /Pack passports/); assert.equal(doc.page.has('Edit'), false);
  await doc.page.click(doc.page.button('Show latest')); assert.match(doc.page.text(), /Second version/); assert.equal(doc.page.has('Edit'), true);
  assert.deepEqual(doc.sent.flatMap(c => c.action === 'readArtifact' ? [c.payload] : []), [{ artifactId: 'artifact-test' }, { artifactId: 'artifact-test', revision: 1 }, { artifactId: 'artifact-test' }]);
});
scenario('A task whose outside action is unconfirmed says so; only the maintainer confirms what happened', async context => {
  withDom(context);
  const unchecked = (requester = 'alice') => [waitTask({ state: 'needs_reconciliation', wait: undefined, unverifiedOperations: 1 }, { uid: requester, generation: 1 })];
  const operations = [
    { operationId: 'a'.repeat(64), logicalStepId: 'task-test/t1/c0', category: 'store_idempotent', tool: { name: 'shared_list.add', version: '1' }, state: 'succeeded', epoch: 1, dispatchedAt: 1700000000000, settledAt: 1700000000500 },
    { operationId: 'b'.repeat(64), logicalStepId: 'task-test/t2/c0', category: 'unqueryable', tool: { name: 'email.send', version: '1' }, state: 'unknown', epoch: 1, dispatchedAt: 1700000001000 },
  ];
  let failure = '';
  const answer = (command: MessageCommand) => {
    if (command.action === 'listOperations') return { taskId: 'task-test', operations };
    if (command.action === 'resolveOperation') { if (failure) throw new Error(failure); operations[1] = { ...operations[1]!, state: command.payload.resolution, resolvedBy: { uid: 'alice', generation: 1 }, resolvedAt: 1 } as never; return { taskId: 'task-test', state: 'interrupted', operationId: command.payload.operationId, operationState: command.payload.resolution }; }
    return {};
  };
  const maintainer = await openChat(context, () => unchecked('bob'), answer, 'alice');
  assert.match(maintainer.page.text(), /Needs checking/); assert.match(maintainer.page.text(), /Check it and confirm the result/); assert.equal(maintainer.buttons('Cancel').length, 0);
  await maintainer.page.click(maintainer.page.button('Review actions'));
  assert.match(maintainer.page.text(), /email\.send · Result unknown/); assert.match(maintainer.page.text(), /shared_list\.add · Happened/);
  assert.equal(maintainer.buttons('It happened').length, 1, 'only the unconfirmed action is decided');
  failure = 'TASK_RUNNING'; await maintainer.page.click(maintainer.page.button('It did not happen')); assert.match(maintainer.page.text(), /still running and may report the result itself/);
  failure = ''; const before = maintainer.sent.filter(c => c.action === 'listTasks').length;
  await maintainer.page.click(maintainer.page.button('It did not happen'));
  assert.deepEqual(maintainer.sent.flatMap(c => c.action === 'resolveOperation' ? [c.payload] : []).at(-1), { taskId: 'task-test', operationId: 'b'.repeat(64), resolution: 'failed' });
  assert.match(maintainer.page.text(), /email\.send · Did not happen/); assert.match(maintainer.page.text(), /confirmed by Alice/); assert.equal(maintainer.buttons('It happened').length, 0);
  assert.ok(maintainer.sent.filter(c => c.action === 'listTasks').length > before, 'the task is fetched again');
  operations[1] = { ...operations[1]!, state: 'unknown' }; delete (operations[1] as Record<string, unknown>).resolvedBy;
  const requester = await openChat(context, () => unchecked('alice'), answer, 'owner');
  assert.match(requester.page.text(), /The Bot’s maintainer needs to confirm it/); await requester.page.click(requester.page.button('View actions'));
  assert.match(requester.page.text(), /email\.send · Result unknown/); assert.equal(requester.buttons('It happened').length, 0);
  const stranger = await openChat(context, () => unchecked('bob'), answer, 'owner');
  assert.match(stranger.page.text(), /Needs checking/); assert.equal(stranger.page.has('View actions'), false); assert.equal(stranger.page.has('Review actions'), false);
});
/** The Bot directory with a Bot that keeps a shared list. The items live in `store`; `store.lose` drops the answer of the next write after saving it. */
async function openList(context: Parameters<Parameters<typeof scenario>[1]>[0]) {
  const store = { dataRevision: 1, next: 2, lose: false, items: [{ itemId: 'item_0001', text: 'Milk', done: false }], history: [] as any[] }, saved = new Map<string, number>(), sent: any[] = [];
  const { fake, workspace } = await open(context), inner = fake.messages;
  fake.messages = async <T>(command: MessageCommand) => {
    if (!('schemaVersion' in command) || command.action !== 'readList' && command.action !== 'writeList' && command.action !== 'listHistory') return inner<T>(command);
    sent.push(structuredClone(command)); const sharedBotId = command.payload.sharedBotId;
    if (command.action === 'listHistory') {
      const before = command.payload.beforeRevision ?? Infinity, older = store.history.filter(r => r.dataRevision < before);
      return { sharedBotId, dataRevision: store.dataRevision, hasMore: older.length > 2, revisions: older.slice(0, 2) } as T;
    }
    if (command.action === 'readList') return { sharedBotId, title: 'Groceries', description: 'What the household needs.', dataRevision: store.dataRevision, items: structuredClone(store.items), updatedAt: 1 } as T;
    const p = command.payload, again = saved.get(p.operationId); if (again !== undefined) return { sharedBotId, dataRevision: again } as T;
    if (p.expectedDataRevision !== store.dataRevision) throw new Error('LIST_REVISION_CONFLICT');
    for (const change of p.changes) {
      if (change.op === 'add') { store.items.push({ itemId: `item_${String(store.next++).padStart(4, '0')}`, text: change.text.trim(), done: false }); continue; }
      const index = store.items.findIndex(item => item.itemId === change.itemId); if (index < 0) throw new Error('LIST_ITEM_REQUIRED');
      if (change.op === 'remove') store.items.splice(index, 1);
      else Object.assign(store.items[index]!, change.text === undefined ? {} : { text: change.text.trim() }, change.done === undefined ? {} : { done: change.done });
    }
    store.dataRevision += 1; saved.set(p.operationId, store.dataRevision);
    if (store.lose) { store.lose = false; throw new Error('SERVICE_UNAVAILABLE'); }
    return { sharedBotId, dataRevision: store.dataRevision } as T;
  };
  fake.bots = [botCard({ maintainer: { uid: 'alice', generation: 1 }, sharedList: { title: 'Groceries' } })]; await workspace.refreshBots(); await until(() => !!workspace.getSnapshot().bots[0]?.sharedList, 'the Bot with a list');
  const page = await mount(workspace, directory(workspace)); context.after(() => page.unmount());
  const writes = () => sent.flatMap(c => c.action === 'writeList' ? [c.payload] : []);
  const add = { box: () => page.oneByLabel('New item'), form: () => page.oneByClass('tw-list-add') };
  return { page, store, writes, add, fake, workspace, sent };
}
scenario('A shared list opens from its Bot; adding, ticking, renaming and removing each save one change on top of the list that was read', async context => {
  const list = await openList(context), { page } = list;
  assert.equal(page.byLabel('New item').length, 0); await page.click(page.button('List: Groceries'));
  assert.match(page.text(), /Milk/); assert.match(page.text(), /can read and change it too/); assert.equal(page.button('Add').props.disabled, true);
  await page.type(list.add.box(), ' Bread '); await page.submit(list.add.form());
  assert.equal(list.add.box().props.value, '', 'the box is ready for the next item'); assert.match(page.text(), /Bread/);
  await page.check(page.find(n => n.type === 'input' && n.props.type === 'checkbox' && n.props.checked === false)[0]);
  await page.click(page.oneByLabel('Edit “Bread”')); assert.equal(page.button('Save').props.disabled, true, 'nothing changed yet');
  await page.type(page.oneByLabel('Item text'), 'Rye bread'); await page.submit(page.oneByLabel('Item text').parent);
  await page.click(page.oneByLabel('Remove “Milk”'));
  assert.deepEqual(list.writes().map(({ operationId, sharedBotId, ...rest }) => rest), [
    { expectedDataRevision: 1, changes: [{ op: 'add', text: ' Bread ' }] }, { expectedDataRevision: 2, changes: [{ op: 'update', itemId: 'item_0001', done: true }] },
    { expectedDataRevision: 3, changes: [{ op: 'update', itemId: 'item_0002', text: 'Rye bread' }] }, { expectedDataRevision: 4, changes: [{ op: 'remove', itemId: 'item_0001' }] }]);
  assert.equal(new Set(list.writes().map(write => write.operationId)).size, 4);
  assert.deepEqual(list.store.items, [{ itemId: 'item_0002', text: 'Rye bread', done: false }]); assert.doesNotMatch(page.text(), /Milk/);
  await page.type(list.add.box(), 'x'.repeat(501)); assert.match(page.text(), /too long to save/); assert.equal(page.button('Add').props.disabled, true);
});
scenario('A change made on an outdated list is not saved: the newer list is shown and what was typed stays for the person to decide', async context => {
  const list = await openList(context), { page } = list; await page.click(page.button('List: Groceries'));
  await page.type(list.add.box(), 'Eggs');
  list.store.items.push({ itemId: 'item_0002', text: 'Bob added oat milk', done: false }); list.store.next = 3; list.store.dataRevision = 2;
  await page.submit(list.add.form());
  assert.equal(list.store.items.length, 2, 'nothing was saved'); assert.match(page.text(), /was changed in the meantime, so your change was not saved/); assert.match(page.text(), /Bob added oat milk/);
  assert.equal(list.add.box().props.value, 'Eggs');
  await page.submit(list.add.form());
  assert.deepEqual(list.writes().map(write => write.expectedDataRevision), [1, 2]); assert.notEqual(list.writes()[0].operationId, list.writes()[1].operationId);
  assert.deepEqual(list.store.items.map(item => item.text), ['Milk', 'Bob added oat milk', 'Eggs']); assert.doesNotMatch(page.text(), /was changed in the meantime/);
  // An item someone removed cannot be changed: the list is shown again without it.
  await page.click(page.oneByLabel('Edit “Milk”')); await page.type(page.oneByLabel('Item text'), 'Whole milk');
  list.store.items.shift(); list.store.dataRevision = 4; await page.submit(page.oneByLabel('Item text').parent);
  assert.match(page.text(), /was changed in the meantime/); assert.equal(page.byLabel('Item text').length, 0); assert.doesNotMatch(page.text(), /Milk\b(?! )/);
});
scenario('A list change whose answer was lost is repeated with the same operation id and saves nothing twice', async context => {
  const list = await openList(context), { page } = list; await page.click(page.button('List: Groceries'));
  await page.type(list.add.box(), 'Eggs'); list.store.lose = true; await page.submit(list.add.form());
  assert.match(page.text(), /Could not sync messages/); assert.equal(list.add.box().props.value, 'Eggs', 'the text is kept');
  await page.submit(list.add.form());
  assert.equal(list.writes().length, 2); assert.equal(list.writes()[0].operationId, list.writes()[1].operationId);
  assert.deepEqual(list.store.items.map(item => item.text), ['Milk', 'Eggs']); assert.equal(list.add.box().props.value, '');
  assert.doesNotMatch(page.text(), /Could not sync messages/);
});
scenario('An open list shows what others save without Refresh, and its history says who changed what, a page at a time', async context => {
  const list = await openList(context), { page } = list; await page.click(page.button('List: Groceries'));
  await page.type(list.add.box(), 'Eggs');
  // Bob saves while the list is open: the Bot's change signal brings it in, and what Alice was typing stays.
  list.store.items.push({ itemId: 'item_0002', text: 'Oat milk', done: false }); list.store.dataRevision = 2;
  await page.during(() => list.fake.signal({ bots: true }));
  assert.match(page.text(), /Oat milk/); assert.equal(list.add.box().props.value, 'Eggs');
  // A signal about a conversation is not about the list.
  const reads = () => list.sent.filter(c => c.action === 'readList').length, before = reads();
  await page.during(() => list.fake.signal({ conversationId: 'test-chat' }));
  assert.equal(reads(), before);
  assert.equal(list.sent.some(c => c.action === 'listHistory'), false, 'history is read only when it is opened');
  list.store.history = [
    { dataRevision: 4, author: { kind: 'shared_bot', sharedBotId: 'assistant-fixture', taskId: 'task_0001', requester: { uid: 'bob', generation: 1 } }, createdAt: 3,
      changes: [{ op: 'update', itemId: 'item_0001', text: 'Whole milk', done: true, before: { text: 'Milk', done: false } }] },
    { dataRevision: 3, author: { kind: 'user', uid: 'alice', generation: 1 }, createdAt: 2, changes: [{ op: 'remove', itemId: 'item_0003', before: { text: 'Wine', done: false } }] },
    { dataRevision: 2, author: { kind: 'user', uid: 'bob', generation: 1 }, createdAt: 1, changes: [{ op: 'add', itemId: 'item_0002', text: 'Oat milk' }] },
    { dataRevision: 1, author: { kind: 'user', uid: 'carol', generation: 1 }, createdAt: 0, changes: [{ op: 'update', itemId: 'item_0001', done: false, before: { text: 'Milk', done: true } }] }];
  await page.during(() => page.oneByClass('tw-list-history').props.onToggle({ currentTarget: { open: true } }));
  const text = page.text();
  assert.match(text, /Fixture Bot, for Bob/); assert.match(text, /Renamed “Milk” to “Whole milk”; ticked “Whole milk”/);
  assert.match(text, /You/); assert.match(text, /Removed “Wine”/); assert.doesNotMatch(text, /Oat milk”/);
  await page.click(page.button('Show older changes'));
  assert.match(page.text(), /Added “Oat milk”/); assert.match(page.text(), /a former member/); assert.match(page.text(), /Unticked “Milk”/);
  assert.equal(page.has('Show older changes'), false);
  assert.deepEqual(list.sent.flatMap(c => c.action === 'listHistory' ? [c.payload.beforeRevision ?? null] : []), [null, 3]);
});
scenario('Publication offers a shared list, names it in the review, and carries an existing list into the next publication', async context => {
  const published: any[] = [];
  const { fake, workspace } = await open(context, { localBots: async () => [{ instanceId: 'local-new', name: 'Travel', description: '', capabilities: { durableWait: true, writeOperations: true } }, { instanceId: 'local-fixture', name: 'Fixture', description: '', capabilities: { durableWait: true, writeOperations: true } }], selectSharedFiles: async () => [],
    publishLocalBot: async input => { published.push(input); return { sharedBotId: input.sharedBotId, revision: input.expectedRevision + 1 }; } });
  const inner = fake.messages;
  fake.messages = async <T>(command: MessageCommand) => 'schemaVersion' in command && command.action === 'readList'
    ? { sharedBotId: command.payload.sharedBotId, title: 'Groceries', description: 'What the household needs.', dataRevision: 0, items: [], updatedAt: 1 } as T : inner<T>(command);
  const page = await mount(workspace, directory(workspace)); context.after(() => page.unmount());
  const listBox = () => page.find(n => n.type === 'input' && n.props.type === 'checkbox' && textOf(n.parent) === 'Give this Bot a shared list')[0];
  const name = () => page.find(n => n.type === 'input' && n.props.maxLength === 120)[0], purpose = () => page.find(n => n.type === 'textarea' && n.props.maxLength === 2000)[0];
  const confirm = async () => { await page.submit(); await page.check(page.find(n => n.type === 'input' && n.props.type === 'checkbox').at(-1)); await page.click(page.button('Publish Bot')); };
  await page.click(page.button('Load local Bots')); await page.type(page.find(n => n.type === 'select')[0], 'local-new'); await page.type(page.find(n => n.type === 'textarea')[0], 'Keep the shopping list.');
  assert.equal(name(), undefined); await page.check(listBox());
  assert.equal(page.button('Review publication').props.disabled, true, 'a list needs a name and a purpose');
  await page.type(name(), ' Groceries '); await page.type(purpose(), ' What the household needs. '); assert.equal(page.button('Review publication').props.disabled, false);
  await page.submit(); assert.match(page.text(), /Shared list: Groceries/);
  await page.check(page.find(n => n.type === 'input' && n.props.type === 'checkbox').at(-1)); await page.click(page.button('Publish Bot'));
  assert.deepEqual(published[0].sharedList, { title: 'Groceries', description: 'What the household needs.' });
  // The Bot that already has a list is published again with it, unless its maintainer switches the list off.
  fake.bots = [botCard({ maintainer: { uid: 'alice', generation: 1 }, instanceId: 'local-fixture', sharedList: { title: 'Groceries' } })]; await workspace.refreshBots(); await until(() => !!workspace.getSnapshot().bots[0]?.sharedList, 'the Bot with a list');
  await page.click(page.button('Load local Bots')); await page.type(page.find(n => n.type === 'select')[0], 'local-fixture'); await until(() => purpose()?.props.value === 'What the household needs.', 'the list of the published Bot');
  assert.equal(listBox().props.checked, true); assert.equal(name().props.value, 'Groceries');
  await page.type(page.find(n => n.type === 'textarea')[0], 'Keep the shopping list.'); await confirm();
  assert.deepEqual(published[1].sharedList, { title: 'Groceries', description: 'What the household needs.' });
  await page.click(page.button('Load local Bots')); await page.type(page.find(n => n.type === 'select')[0], 'local-fixture'); await until(() => purpose()?.props.value === 'What the household needs.', 'the list of the published Bot');
  await page.check(listBox(), false); assert.match(page.text(), /Publishing without the list hides it from everyone/);
  await page.type(page.find(n => n.type === 'textarea')[0], 'Keep the shopping list.'); await confirm();
  assert.equal(published.length, 3); assert.equal('sharedList' in published[2], false);
  // The maintainer can have the assistant ask before each change; a Bot published that way keeps asking in its next publication.
  const askBox = () => page.find(n => n.type === 'input' && n.props.type === 'checkbox' && textOf(n.parent) === 'Ask before the Bot changes the list')[0];
  await page.click(page.button('Load local Bots')); await page.type(page.find(n => n.type === 'select')[0], 'local-new'); await page.type(page.find(n => n.type === 'textarea')[0], 'Keep the shopping list.');
  assert.equal(askBox(), undefined, 'offered only with a list'); await page.check(listBox()); await page.type(name(), 'Groceries'); await page.type(purpose(), 'What the household needs.');
  assert.equal(askBox().props.checked, false); await page.check(askBox()); assert.match(page.text(), /waits up to a day for them to approve or decline/);
  await page.submit(); assert.match(page.text(), /Shared list: Groceries · the Bot asks before each change/);
  await page.check(page.find(n => n.type === 'input' && n.props.type === 'checkbox').at(-1)); await page.click(page.button('Publish Bot'));
  assert.deepEqual(published[3].sharedList, { title: 'Groceries', description: 'What the household needs.', approval: 'requester' });
  fake.bots = [botCard({ maintainer: { uid: 'alice', generation: 1 }, instanceId: 'local-fixture', sharedList: { title: 'Groceries', approval: 'requester' } })]; await workspace.refreshBots();
  await until(() => workspace.getSnapshot().bots[0]?.sharedList?.approval === 'requester', 'the Bot that asks');
  await page.click(page.button('Load local Bots')); await page.type(page.find(n => n.type === 'select')[0], 'local-fixture'); await until(() => purpose()?.props.value === 'What the household needs.', 'the list of the published Bot');
  assert.equal(askBox().props.checked, true); await page.type(page.find(n => n.type === 'textarea')[0], 'Keep the shopping list.'); await confirm();
  assert.deepEqual(published[4].sharedList, { title: 'Groceries', description: 'What the household needs.', approval: 'requester' });
});
// ── Handing an assistant to another maintainer (M3-3) ──
const offer = (more: Record<string, unknown> = {}) => ({ transferId: 'transfer-test', from: { uid: 'bob', generation: 1 }, to: { uid: 'alice', generation: 1 }, state: 'offered' as const, expiresAt: Date.UTC(2026, 10, 20), ...more });
scenario('A maintainer offers their assistant to a member who can use it, is told what goes and what stays, and can take the offer back (M3-3)', async context => {
  const offers: import('../../../packages/teams/execution').OfferSharedBotInput[] = []; let fail = true;
  const { fake, workspace } = await open(context, { offerSharedBot: async input => {
    offers.push(structuredClone(input)); if (fail) { fail = false; throw new Error('SERVICE_UNAVAILABLE'); }
    fake.bots = [botCard({ maintainer: { uid: 'alice', generation: 1 }, transfer: offer({ transferId: input.transferId, from: { uid: 'alice', generation: 1 }, to: input.to }) })];
  } });
  const page = await mount(workspace, directory(workspace)); context.after(() => page.unmount());
  const buttons = (text: string) => page.find(n => n.type === 'button' && textOf(n) === text), options = () => page.find(n => n.type === 'option').map(textOf);
  const box = (label: RegExp) => page.find(n => n.type === 'input' && n.props.type === 'checkbox' && label.test(textOf(n.parent)))[0];
  assert.equal(page.byLabel('Handover').length, 0); await page.click(page.button('Hand over…'));
  assert.match(page.text(), /They receive the published role and the files it is shared with\./); assert.match(page.text(), /Your local Bot, your model account and everything else on this Mac stay with you\./);
  assert.match(page.text(), /Conversations people already had with it stay with them and with you\. The new maintainer cannot read them\./);
  assert.match(page.text(), /Once they take it over, their Mac answers and they pay for the model\. You can no longer stop sharing or publish it, and people start a new chat with it\./);
  assert.deepEqual(options(), ['Choose a member', 'Bob', 'Owner'], 'not oneself'); assert.equal(page.button('Offer').props.disabled, true);
  await page.type(page.find(n => n.type === 'select')[0], 'bob'); assert.equal(page.button('Offer').props.disabled, true, 'the right to pass it on is confirmed first');
  await page.check(box(/I may pass this role and these files on/)); await page.click(page.button('Offer')); await until(() => offers.length === 1, 'the first try');
  // Its answer was lost: the same offer is made again, not a second one.
  await until(() => buttons('Offer').length === 1 && !page.button('Offer').props.disabled, 'the dialog stays'); await page.click(page.button('Offer'));
  await until(() => offers.length === 2 && page.byLabel('Handover').length === 1, 'the offer');
  assert.deepEqual(offers[0], offers[1]); assert.match(offers[0]!.transferId, /^[A-Za-z0-9_-]{8,64}$/u);
  assert.deepEqual({ ...offers[0], transferId: '' }, { teamId: 'team-test', generation: 1, transferId: '', sharedBotId: 'assistant-fixture', expectedRevision: 1, to: { uid: 'bob', generation: 1 } });
  assert.match(textOf(page.oneByLabel('Handover')), /Offered to Bob\. You maintain it until they take it over\. The offer ends on /); assert.equal(buttons('Hand over…').length, 0); assert.equal(buttons('Stop sharing').length, 1, 'still the maintainer');
  await page.click(page.button('Take back')); const end = fake.commands.find(c => c.action === 'endTransfer');
  assert.deepEqual(end && 'payload' in end ? end.payload : undefined, { transferId: offers[0]!.transferId });
  // While their Mac takes it over the card says so.
  fake.bots = [botCard({ maintainer: { uid: 'alice', generation: 1 }, transfer: offer({ from: { uid: 'alice', generation: 1 }, to: { uid: 'bob', generation: 1 }, state: 'accepted' }) })]; await workspace.refreshBots();
  await until(() => /Bob is taking this Bot over\. You maintain it until their Mac answers for it\./.test(page.text()), 'accepted');
  // A handover never widens who sees the files: only members who may use the assistant can be chosen.
  fake.bots = [botCard({ maintainer: { uid: 'alice', generation: 1 }, usePolicy: { mode: 'listed', members: [{ uid: 'bob', generation: 1 }] } })]; await workspace.refreshBots();
  await until(() => buttons('Hand over…').length === 1, 'no offer'); await page.click(page.button('Hand over…')); assert.deepEqual(options(), ['Choose a member', 'Bob']);
});
scenario('The member an assistant is offered to takes it over on their Desktop with one of their Bots and is told what that means (M3-3)', async context => {
  const accepted: unknown[] = [];
  const { fake, workspace } = await open(context, { localBots: async () => [{ instanceId: 'local-mine', name: 'Mine', description: '', model: 'my-model' }],
    acceptSharedBot: async input => { accepted.push(structuredClone(input)); fake.bots = [botCard({ maintainer: { uid: 'alice', generation: 1 }, revision: 2 })]; return { sharedBotId: 'assistant-fixture', revision: 2 }; } });
  fake.bots = [botCard({ maintainer: { uid: 'bob', generation: 1 }, transfer: offer() }), botCard({ sharedBotId: 'assistant-other', name: 'Other', maintainer: { uid: 'bob', generation: 1 } })];
  await workspace.refreshBots(); await until(() => workspace.getSnapshot().bots.length === 2, 'two Bots');
  const page = await mount(workspace, directory(workspace)); context.after(() => page.unmount());
  const box = (label: RegExp) => page.find(n => n.type === 'input' && n.props.type === 'checkbox' && label.test(textOf(n.parent)))[0];
  assert.equal(page.byLabel('Handover').length, 1, 'only on the offered assistant');
  assert.match(textOf(page.oneByLabel('Handover')), /Bob wants to hand this Bot over to you\. It stays with Bob until you take it over\. The offer ends on /); assert.doesNotMatch(page.text(), /open LingXi Desktop on the Mac/);
  await page.click(page.button('Take over…')); await until(() => /You receive the role and the files Fixture Bot is published with\. Nothing else of Bob’s comes with it\./.test(page.text()), 'the dialog');
  assert.match(page.text(), /This Mac answers for the Bot from now on, with your model account and at your cost\./);
  assert.match(page.text(), /You can read every new conversation with it\. Earlier conversations stay with the people who had them, and you cannot read them\./);
  assert.match(page.text(), /Who may use it, its daily limits and its shared list stay as they are\./); assert.match(page.text(), /Mine · my-model/);
  assert.equal(page.button('Take over').props.disabled, true); await page.type(page.find(n => n.type === 'select')[0], 'local-mine');
  assert.equal(page.button('Take over').props.disabled, true); await page.check(box(/I take over maintaining this Bot and its model costs/));
  await page.click(page.button('Take over')); await until(() => accepted.length === 1 && page.byLabel('Handover').length === 0, 'taken over');
  assert.deepEqual(accepted, [{ teamId: 'team-test', generation: 1, transferId: 'transfer-test', instanceId: 'local-mine' }]); assert.match(page.text(), /Maintainer: Alice/);
});
scenario('Off Desktop an offered assistant can be declined but not taken over or offered, and nobody but the two is shown the offer (M3-3)', async context => {
  const { fake, workspace } = await open(context);
  fake.bots = [botCard({ maintainer: { uid: 'bob', generation: 1 }, transfer: offer() }), botCard({ sharedBotId: 'assistant-mine', name: 'Mine', maintainer: { uid: 'alice', generation: 1 } }), botCard({ sharedBotId: 'assistant-other', name: 'Other', maintainer: { uid: 'bob', generation: 1 } })];
  await workspace.refreshBots(); await until(() => workspace.getSnapshot().bots.length === 3, 'three Bots');
  const page = await mount(workspace, directory(workspace)); context.after(() => page.unmount());
  const buttons = (text: string) => page.find(n => n.type === 'button' && textOf(n) === text);
  assert.equal(page.byLabel('Handover').length, 1); assert.match(textOf(page.oneByLabel('Handover')), /To take it over, open LingXi Desktop on the Mac that will answer for it\./);
  assert.deepEqual([buttons('Take over…').length, buttons('Hand over…').length, buttons('Decline').length], [0, 0, 1], 'the role and files are only on the Desktop that published them');
  await page.click(page.button('Decline')); const end = fake.commands.find(c => c.action === 'endTransfer'); assert.deepEqual(end && 'payload' in end ? end.payload : undefined, { transferId: 'transfer-test' });
});
scenario('A chat with an assistant from before it changed maintainer stays readable, takes no new request and leads to a new chat (M3-3)', async context => {
  withDom(context); const { findBotDm } = await import('../../../packages/teams/workspace');
  const dm = (id: string, readers?: string[]) => conversation(id, { kind: 'bot_dm', participants: ['user:alice', 'shared_bot:assistant-fixture'], lastSeq: 2, ...(readers ? { readers } : {}) });
  const cards = [botCard({ maintainer: { uid: 'owner', generation: 1 } })];
  assert.equal(findBotDm([dm('before', ['alice', 'bob']), dm('after', ['alice', 'owner'])], 'alice', 'assistant-fixture', cards)?.id, 'after');
  assert.equal(findBotDm([dm('before', ['alice', 'bob'])], 'alice', 'assistant-fixture', cards), undefined);
  assert.equal(findBotDm([dm('unknown')], 'alice', 'assistant-fixture', cards)?.id, 'unknown', 'a source that does not name the readers says nothing');
  assert.equal(findBotDm([dm('own', ['owner'])], 'alice', 'assistant-fixture', [botCard({ maintainer: { uid: 'alice', generation: 1 } })]), undefined, 'the earlier maintainer’s own chat is not the new maintainer’s');
  // The member's chat was made while Bob maintained the assistant. The owner maintains it now.
  const { fake, page, sent, buttons, box } = await openChat(context, () => [], () => ({}), 'owner');
  assert.ok(box(), 'readers unknown: the chat is used as before');
  fake.conversations.set('team-test', [dm('test-chat', ['alice', 'bob'])]); for (const watch of fake.open('team-test')) fake.deliver(watch);
  await until(() => /This Bot has a new maintainer and no longer answers in this chat\. What was said here stays as it is\./.test(page.text()), 'the notice');
  assert.equal(box(), undefined); assert.match(page.text(), /Plan my trip/, 'what was said stays');
  // A chat whose readers include the current maintainer is the current one.
  fake.conversations.set('team-test', [dm('test-chat', ['alice', 'owner'])]); for (const watch of fake.open('team-test')) fake.deliver(watch);
  await until(() => !!box(), 'the composer');
  fake.conversations.set('team-test', [dm('test-chat', ['alice', 'bob'])]); for (const watch of fake.open('team-test')) fake.deliver(watch);
  await until(() => buttons('Start a new chat').length === 1, 'the way to a new chat');
  await page.click(buttons('Start a new chat')[0]); await until(() => sent.some(c => c.action === 'createConversation'), 'a new chat');
  const created = sent.find(c => c.action === 'createConversation');
  assert.ok(created && 'payload' in created && created.action === 'createConversation' && created.payload.conversationId !== 'test-chat' && created.payload.kind === 'bot_dm');
});
scenario('Auto-reply: the wizard offers it only with Decision set up on this Desktop, and the review and publication carry the daily limit', async context => {
  const published: Record<string, unknown>[] = []; let decisionConfigured = false;
  const status = (): import('../../../packages/teams/execution').SharedHostStatus => ({ bots: [], openAtLogin: { available: false, enabled: false, needsApproval: false }, keepAwake: false, onBattery: false, decisionConfigured, isolationAvailable: true });
  const { workspace } = await open(context, { localBots: async () => [{ instanceId: 'local-new', name: 'Travel', description: '', model: 'Configured model' }], selectSharedFiles: async () => [], hostStatus: async () => status(),
    publishLocalBot: async input => { published.push(input as unknown as Record<string, unknown>); return { sharedBotId: input.sharedBotId, revision: 1 }; } });
  const page = await mount(workspace, directory(workspace)); context.after(() => page.unmount());
  const autoBox = () => page.find(n => n.type === 'input' && n.props.type === 'checkbox' && textOf(n.parent) === 'Let the Bot decide when to answer')[0];
  await page.click(page.button('Load local Bots'));
  assert.equal(autoBox().props.disabled, true); assert.match(page.text(), /set up Decision on this Desktop first: Settings → Models & Services → Decision/);
  decisionConfigured = true; await page.click(page.button('Load local Bots'));
  assert.equal(autoBox().props.disabled, false);
  await page.type(page.find(n => n.type === 'select')[0], 'local-new'); await page.type(page.find(n => n.type === 'textarea')[0], 'Help with trips.');
  await page.check(autoBox()); assert.match(page.text(), /This Desktop’s Decision service and the Bot’s model see those messages, and you pay for both/);
  await page.type(page.find(n => n.type === 'input' && n.props.type === 'number').at(-1), '5');
  await page.submit(); assert.match(page.text(), /Answers on its own in chats that turn it on, at most 5 a day/);
  await page.check(page.find(n => n.type === 'input' && n.props.type === 'checkbox').at(-1)); await page.click(page.button('Publish Bot'));
  assert.equal(published.length, 1); assert.deepEqual(published[0]!.autoReply, { daily: 5 });
});
scenario('Where this computer cannot isolate Bots the publish button is greyed out and says why; where it can, or the host has not said, it is left to try (B2)', async context => {
  const status = (isolationAvailable: boolean): import('../../../packages/teams/execution').SharedHostStatus => ({ bots: [], openAtLogin: { available: false, enabled: false, needsApproval: false }, keepAwake: false, onBattery: false, decisionConfigured: false, isolationAvailable });
  const native = { localBots: async () => [], selectSharedFiles: async () => [], publishLocalBot: async (input: { sharedBotId: string }) => ({ sharedBotId: input.sharedBotId, revision: 1 }) };
  const explanation = /Sharing a Team Bot needs macOS, or Linux with bubblewrap \(bwrap\) installed\./;
  for (const [name, overrides, greyed] of [
    ['cannot', { hostStatus: async () => status(false) }, true],
    ['can', { hostStatus: async () => status(true) }, false],
    ['has not said', { hostStatus: () => new Promise<never>(() => undefined) }, false],
    ['cannot be asked', { hostStatus: async () => { throw new Error('TEAM_HOST_UNAVAILABLE'); } }, false],
    ['is a browser', {}, false],
  ] as const) {
    const { workspace } = await open(context, name === 'is a browser' ? {} : { ...native, ...overrides });
    const page = await mount(workspace, directory(workspace)); context.after(() => page.unmount());
    if (greyed) await until(() => explanation.test(page.text()), 'the explanation'); else await page.during(async () => { await new Promise(resolve => setTimeout(resolve, 20)); });
    assert.equal(explanation.test(page.text()), greyed, name);
    if (name === 'is a browser') { assert.equal(page.find(n => n.type === 'button' && textOf(n) === 'Load local Bots').length, 0, 'a browser has no local Bots to publish'); continue; }
    const load = page.button('Load local Bots');
    assert.equal(load.props.disabled === true, greyed, name);
    const note = page.find(n => n.type === 'p' && explanation.test(textOf(n)))[0];
    assert.equal(load.props['aria-describedby'], greyed ? note?.props.id : undefined, `${name}: the button is described by the explanation`);
  }
  // The same explanation is what an attempt that reached the host is answered with, in every language.
  const refused = (locale: string) => messageErrorText('TEAM_HOST_ISOLATION_UNAVAILABLE', locale);
  assert.equal(refused('en'), 'Sharing a Team Bot needs macOS, or Linux with bubblewrap (bwrap) installed.');
  for (const locale of ['zh-Hans', 'zh-Hant', 'ja', 'ko']) { assert.ok(refused(locale).includes('bwrap'), locale); assert.notEqual(refused(locale), refused('en'), locale); }
});
scenario('Auto-reply: a person in a Chat turns a Bot on after being told where the messages go; a declined run is one quiet line', async context => {
  const { fake, workspace } = await open(context); const calls: MessageCommand[] = [];
  fake.bots = [botCard({ maintainer: { uid: 'alice', generation: 1 }, autoReply: { daily: 7 } })]; await workspace.refreshBots(); await until(() => !!workspace.getSnapshot().bots[0]?.autoReply, 'the Bot');
  // `hidden-bot` is on in this Chat, and published only to others: this person cannot see it, and their change leaves it on.
  fake.conversations.set('team-test', [conversation('test-chat', { kind: 'group', participants: ['user:alice', 'user:bob', 'shared_bot:assistant-fixture', 'shared_bot:hidden-bot'],
    autoReply: { revision: 2, targets: ['hidden-bot'] } })]);
  await workspace.refresh(); workspace.selectConversation('test-chat'); await until(() => workspace.getSnapshot().selectedConversationId === 'test-chat', 'selection');
  fake.messages = async <T>(command: MessageCommand) => { calls.push(command);
    if (command.action === 'listTasks') return { tasks: [{ taskId: 'task-test', conversationId: 'test-chat', sharedBotId: 'assistant-fixture', requester: { uid: 'bob', generation: 1 }, state: 'declined', optional: true, createdAt: 1, deadline: 2,
      usage: { status: 'complete', inputTokens: 1200, outputTokens: 0 } }] } as T;
    return { bots: fake.bots } as T; };
  const page = await mount(workspace, snapshot => createElement(TeamConversationView, { workspace, snapshot, locale: 'en' })); context.after(() => page.unmount());
  await until(() => /Did not reply/.test(page.text()), 'the declined run'); assert.doesNotMatch(page.text(), /Cancel task|Failed/);
  assert.match(page.text(), /Did not reply · 1\.2K tokens/, 'a run that chose silence still shows what it used');
  const button = page.oneByLabel('Bots that answer on their own'); assert.equal(button.props['aria-pressed'], false);
  await page.click(button);
  assert.match(page.text(), /New messages in this chat are sent to the Decision service and model that you set up for Fixture Bot\. It answers on its own at most 7 times a day\./);
  assert.equal(page.button('Save').props.disabled, true, 'nothing changed yet');
  await page.check(page.find(n => n.type === 'input' && n.props.type === 'checkbox' && textOf(n.parent) === 'Fixture Bot')[0]); await page.click(page.button('Save'));
  const sent = calls.find(command => command.action === 'setConversationAutoReply') as Extract<MessageCommand, { action: 'setConversationAutoReply' }> | undefined;
  assert.deepEqual(sent?.payload, { conversationId: 'test-chat', expectedRevision: 2, targets: ['assistant-fixture', 'hidden-bot'] });
});
scenario('Auto-reply: a group also sets how many Bots answer one message and whether a message that @mentions only people is judged; only what changed is sent; a Bot DM has neither', async context => {
  const { fake, workspace } = await open(context); const calls: MessageCommand[] = [];
  fake.bots = [botCard({ maintainer: { uid: 'alice', generation: 1 }, autoReply: { daily: 7 } })]; await workspace.refreshBots(); await until(() => !!workspace.getSnapshot().bots[0]?.autoReply, 'the Bot');
  fake.conversations.set('team-test', [
    conversation('test-chat', { kind: 'group', participants: ['user:alice', 'user:bob', 'shared_bot:assistant-fixture'], autoReply: { revision: 3, targets: ['assistant-fixture'], maxReplies: 2 } }),
    conversation('bot-chat', { kind: 'bot_dm', participants: ['user:alice', 'shared_bot:assistant-fixture'], autoReply: { revision: 1, targets: ['assistant-fixture'] } }),
  ]);
  await workspace.refresh(); workspace.selectConversation('test-chat'); await until(() => workspace.getSnapshot().selectedConversationId === 'test-chat', 'selection');
  fake.messages = async <T>(command: MessageCommand) => { calls.push(command); return (command.action === 'listTasks' ? { tasks: [] } : { bots: fake.bots }) as T; };
  const page = await mount(workspace, snapshot => createElement(TeamConversationView, { workspace, snapshot, locale: 'en' })); context.after(() => page.unmount());
  const sent = () => (calls.filter(command => command.action === 'setConversationAutoReply') as Extract<MessageCommand, { action: 'setConversationAutoReply' }>[]).map(command => command.payload);
  const most = () => page.find(node => node.type === 'select' && textOf(node.parent).startsWith('Most Bots that answer one message on their own'))[0];
  const judge = () => page.find(node => node.type === 'input' && node.props.type === 'checkbox' && textOf(node.parent) === 'Let Bots also judge messages that @mention only people')[0];
  await until(() => /test-chat/.test(page.text()), 'the chat'); await page.click(page.oneByLabel('Bots that answer on their own'));
  // It opens on what the Chat holds: at most two Bots, and a message to people alone is left to them. One to the limit of five can be chosen.
  assert.equal(most().props.value, 2); assert.equal(most().props.children.length, 5); assert.equal(judge().props.checked, false);
  assert.match(page.text(), /Bots on different Desktops are ranked by how sure each is first; Decision probabilities from different maintainers are not compared directly and only break ties\./);
  assert.match(page.text(), /Off: a message that @mentions only people is meant for them, and Bots stay quiet\./);
  assert.equal(page.button('Save').props.disabled, true, 'nothing changed yet');
  await page.type(most(), '3'); await page.click(page.button('Save'));
  assert.deepEqual(sent(), [{ conversationId: 'test-chat', expectedRevision: 3, targets: ['assistant-fixture'], maxReplies: 3 }], 'only the setting that changed is sent');
  await page.click(page.oneByLabel('Bots that answer on their own')); await page.check(judge()); await page.click(page.button('Save'));
  assert.deepEqual(sent()[1], { conversationId: 'test-chat', expectedRevision: 3, targets: ['assistant-fixture'], humanMentions: 'judge' });
  // A Bot DM has the one Bot, which is always the one asked: neither setting means anything there.
  workspace.selectConversation('bot-chat'); await until(() => workspace.getSnapshot().selectedConversationId === 'bot-chat', 'selection');
  await page.click(page.oneByLabel('Bots that answer on their own'));
  assert.equal(most(), undefined); assert.doesNotMatch(page.text(), /Most Bots that answer one message|only people/);
});
scenario('Auto-reply: a Chat whose Bots cannot answer on their own shows no switch', async context => {
  const { fake, workspace } = await open(context);
  fake.conversations.set('team-test', [conversation('test-chat', { kind: 'group', participants: ['user:alice', 'shared_bot:assistant-fixture'] })]);
  await workspace.refresh(); workspace.selectConversation('test-chat'); await until(() => workspace.getSnapshot().selectedConversationId === 'test-chat', 'selection');
  fake.messages = async <T>(command: MessageCommand) => (command.action === 'listTasks' ? { tasks: [] } : { bots: fake.bots }) as T;
  const page = await mount(workspace, snapshot => createElement(TeamConversationView, { workspace, snapshot, locale: 'en' })); context.after(() => page.unmount());
  await until(() => /test-chat/.test(page.text()), 'the chat'); assert.equal(page.byLabel('Bots that answer on their own').length, 0);
});
