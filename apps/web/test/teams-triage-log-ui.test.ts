import assert from 'node:assert/strict';
import test from 'node:test';
import vectors from '../../../packages/teams/contracts/execution-vectors.json';
import { createElement } from 'react';
import { TeamsWorkspace } from '../../../packages/teams/workspace';
import type { SharedBotCard, TaskState, TaskTriage, TriageLogItem, TriageLogPage } from '../../../packages/teams/execution';
import type { MessageCommand, TeamConversation } from '../../../packages/teams/messages';
import { FakeTeams, botCard, conversation, memoryStorage, team, until } from './support/fakeTeams';
import { loadTeamsModule } from './support/loadTeamsModule';
import { mount, scenario, textOf, withDom } from './support/workspacePage';

const log = loadTeamsModule<typeof import('../../../packages/teams/triage-log')>('triage-log');
const { BotDirectory } = loadTeamsModule<typeof import('../../../packages/teams/BotDirectory')>('BotDirectory');
const { TeamConversationView } = loadTeamsModule<typeof import('../../../packages/teams/TeamConversationView')>('TeamConversationView');
const { teamsText } = loadTeamsModule<typeof import('../../../packages/teams/i18n')>('i18n');
const { messageErrorText } = loadTeamsModule<typeof import('../../../packages/teams/errors')>('errors');
const { executionCommand } = loadTeamsModule<typeof import('../../../packages/teams/execution')>('execution');

const BOT = 'assistant-fixture', CHAT = 'test-chat';
const SCOPE = { teamId: 'team-test', generation: 1 };
type Listed = Extract<MessageCommand, { action: 'listTriage' }>;

/** One row of the log: the Bot declined, the Decision service gave 12%, newest verdicts have the larger number. */
const row = (n: number, more: Partial<TriageLogItem> = {}, triage: Partial<TaskTriage> = {}): TriageLogItem => ({
  taskId: `task-${String(n).padStart(4, '0')}`, conversationId: CHAT, messageId: `message-${n}`, createdAt: 1_700_000_000_000 + n, state: 'declined', conversationTitle: 'Planning',
  triage: { action: 'decline', source: 'decision', reason: 'below_threshold', probability: 0.12, at: 1_700_000_000_000 + n * 1000, ...triage }, ...more,
});
/** `count` rows, newest first. */
const rows = (count: number, more: (n: number) => Partial<TriageLogItem> = () => ({})) => Array.from({ length: count }, (_, index) => row(count - index, more(count - index)));
const deferred = <T>() => { let resolve!: (value: T) => void, reject!: (error: unknown) => void; const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
/** What the service answers to `listTriage`: the rows of this Bot after the cursor, at most `limit`, of one Chat when asked. */
function serve(all: TriageLogItem[]) {
  const calls: Listed[] = [];
  const answer = (command: Listed): TriageLogPage => {
    calls.push(structuredClone(command));
    const { conversationId, cursor, limit = 20 } = command.payload;
    const wanted = all.filter(item => !conversationId || item.conversationId === conversationId);
    const from = cursor === undefined ? 0 : wanted.findIndex(item => item.taskId === cursor) + 1;
    const page = wanted.slice(from, from + limit);
    return { items: structuredClone(page), ...(from + limit < wanted.length ? { nextCursor: page.at(-1)!.taskId } : {}) };
  };
  return { calls, answer };
}

// ───────────────────────────── the words, the decoding and the list, without a screen ─────────────────────────────

test('where a judged message ended up is told from the state of its task: every state is one of five outcomes', () => {
  const states: Record<TaskState, string> = {
    completed: 'replied', declined: 'silent', cancelled: 'silent', queued: 'waiting', running: 'waiting', waiting: 'waiting', awaiting_approval: 'approval',
    failed: 'failed', expired: 'failed', interrupted: 'failed', authorization_required: 'failed', revoked: 'failed', needs_reconciliation: 'failed',
  };
  for (const [state, outcome] of Object.entries(states)) assert.equal(log.triageOutcome(state as TaskState), outcome, state);
  assert.deepEqual(Object.keys(states).sort(), [...vectors.taskStates].sort(), 'the contract lists these states and no others: iOS and Android read the same list');
  assert.equal(log.triagePercent(0.124), 12); assert.equal(log.triagePercent(0.125), 13); assert.equal(log.triagePercent(1), 100); assert.equal(log.triagePercent(0), 0);
});

test('every verdict, reason and outcome has words in every language, and a reason this version does not know has none', () => {
  const locales = ['en', 'zh-Hans', 'zh-Hant', 'ja', 'ko'];
  const en = teamsText('en');
  assert.equal(log.triageActionText(en, 'decline'), 'No reply'); assert.equal(log.triageActionText(en, 'run'), 'Reply'); assert.equal(log.triageActionText(en, 'run_may_decline'), 'Left to the Bot');
  assert.equal(log.triageOutcomeText(en, 'silent'), 'Stayed silent'); assert.equal(log.triageProbabilityText(en, 0.124), '12% likely');
  assert.equal(log.triageOutcomeText(en, 'approval'), 'Waiting for approval');
  assert.deepEqual((['approved', 'rejected', 'timeout', 'superseded'] as const).map(approval => log.triageApprovalText(en, approval)), ['Approved', 'Declined', 'Not approved in time', 'Dropped for a newer message']);
  assert.equal(log.triageReasonText(en, undefined), ''); assert.equal(log.triageReasonText(en, 'a_new_reason' as never), '');
  for (const locale of locales) {
    const t = teamsText(locale), seen = new Set<string>();
    for (const action of ['decline', 'run', 'run_may_decline'] as const) seen.add(log.triageActionText(t, action));
    for (const reason of ['quiet_hours', 'cooldown', 'keyword', 'addressed', 'below_threshold', 'between_thresholds', 'above_threshold', 'decision_unavailable', 'message_unavailable'] as const) seen.add(log.triageReasonText(t, reason));
    for (const outcome of ['replied', 'silent', 'failed', 'waiting', 'approval'] as const) seen.add(log.triageOutcomeText(t, outcome));
    // How a reply that was held came out; one that still waits says so as its outcome, and a reply that never was held, or a code this version does not know, says nothing.
    for (const approval of ['approved', 'rejected', 'timeout', 'superseded'] as const) seen.add(log.triageApprovalText(t, approval));
    assert.equal(seen.size, 21, `${locale}: every one reads differently and none is empty`); assert.ok(!seen.has(''), locale);
    assert.equal(log.triageApprovalText(t, 'held'), ''); assert.equal(log.triageApprovalText(t, undefined), ''); assert.equal(log.triageApprovalText(t, 'a_new_outcome' as never), '');
    assert.match(log.triageProbabilityText(t, 0.5), /50/, locale);
    if (locale !== 'en') assert.notEqual(log.triageProbabilityText(t, 0.5), log.triageProbabilityText(en, 0.5), locale);
  }
});

test('a row of the log is read as the service sent it; a row this version cannot read is left out, and the page it came in is still shown', () => {
  const good = row(1, {}, { reason: 'quiet_hours', source: 'quiet' });
  assert.deepEqual(log.triageItem(structuredClone(good)), good);
  const bare = { ...row(2), triage: { action: 'run', source: 'rule', at: 5 } } as unknown;
  assert.deepEqual(log.triageItem(bare), { ...row(2), triage: { action: 'run', source: 'rule', at: 5 } });
  assert.equal(log.triageItem({ ...row(3), conversationTitle: '' })!.conversationTitle, undefined, 'an empty title is no title');
  // A reply that was held says how it came out; a code this version does not know is left out of the row, the row stays.
  for (const approval of ['held', 'approved', 'rejected', 'timeout', 'superseded'] as const) assert.equal(log.triageItem({ ...row(3), state: 'awaiting_approval', approval })!.approval, approval, approval);
  assert.equal(log.triageItem({ ...row(3), state: 'awaiting_approval' })!.state, 'awaiting_approval'); assert.equal('approval' in log.triageItem(row(3))!, false);
  for (const approval of ['a_new_outcome', 7, null, '']) assert.equal('approval' in log.triageItem({ ...row(3), approval })!, false, String(approval));
  // What it does not know is left out of the row; what it needs is the row.
  assert.equal(log.triageItem({ ...row(4), triage: { ...row(4).triage, reason: 'a_new_reason' } })!.triage.reason, undefined);
  for (const probability of [-0.1, 1.2, '0.5', Number.NaN]) assert.equal(log.triageItem({ ...row(5), triage: { ...row(5).triage, probability } })!.triage.probability, undefined, String(probability));
  const unreadable: unknown[] = [null, 7, 'x', [], {}, { ...row(6), triage: null }, { ...row(6), taskId: '' }, { ...row(6), taskId: 7 }, { ...row(6), conversationId: undefined }, { ...row(6), messageId: undefined },
    { ...row(6), createdAt: '1' }, { ...row(6), createdAt: 9e15 }, { ...row(6), state: 'a_new_state' }, { ...row(6), state: undefined },
    { ...row(6), triage: { ...row(6).triage, action: 'a_new_action' } }, { ...row(6), triage: { ...row(6).triage, source: 'elsewhere' } }, { ...row(6), triage: { ...row(6).triage, at: Number.POSITIVE_INFINITY } },
    { ...row(6), triage: { ...row(6).triage, at: 1e16 } }];
  for (const value of unreadable) assert.equal(log.triageItem(value), undefined, JSON.stringify(value));
  const page = log.triagePage({ items: [good, { nonsense: true }, row(7)], nextCursor: 'task-0007' });
  assert.deepEqual(page.items.map(item => item.taskId), ['task-0001', 'task-0007']); assert.equal(page.nextCursor, 'task-0007');
  assert.equal(log.triagePage({ items: [] }).nextCursor, undefined); assert.equal('nextCursor' in log.triagePage({ items: [], nextCursor: '' }), false);
  for (const nonsense of [undefined, null, 'page', [], {}, { items: 'x' }]) assert.throws(() => log.triagePage(nonsense), /INVALID_DATA/);
});

test('the log is asked for with the member command: the Bot, a Chat when given, the cursor of the page before and a page of 20', async () => {
  const sent: MessageCommand[] = [];
  const api = { messages: async <T>(command: MessageCommand) => { sent.push(command); return { items: [] } as T; } };
  await log.triageLogRead(api, SCOPE, { sharedBotId: BOT })(undefined, 20);
  await log.triageLogRead(api, SCOPE, { sharedBotId: BOT, conversationId: CHAT })('task-0004', 50);
  assert.deepEqual(sent.map(command => [command.action, (command as Listed).payload, (command as Listed).teamId, (command as Listed).generation]), [
    ['listTriage', { sharedBotId: BOT, limit: 20 }, 'team-test', 1], ['listTriage', { sharedBotId: BOT, conversationId: CHAT, cursor: 'task-0004', limit: 50 }, 'team-test', 1]]);
  await assert.rejects(log.triageLogRead(api, SCOPE, { sharedBotId: BOT })(undefined, 51), /INVALID_INPUT/, 'the service gives at most 50');
  await assert.rejects(log.triageLogRead(api, SCOPE, { sharedBotId: BOT })(undefined, 0), /INVALID_INPUT/);
  assert.equal(sent.length, 2);
  await assert.rejects(log.triageLogRead({ messages: async () => ({ nope: true }) as never }, SCOPE, { sharedBotId: BOT })(undefined, 20), /INVALID_DATA/, 'an answer that is no page is not an empty log');
  assert.throws(() => executionCommand(SCOPE, 'listTriage', { sharedBotId: BOT, conversationId: 'x' } as never), /INVALID_INPUT/);
});

test('the list reads the first page, then a page at a time after it: each task once, newest first, until the log ends', async () => {
  const service = serve(rows(45)), views: string[] = [];
  const reader = new log.TriageLogReader(async (cursor, limit) => service.answer({ payload: { sharedBotId: BOT, ...(cursor ? { cursor } : {}), limit } } as Listed), view => views.push(view.status), 20);
  assert.equal(reader.view.status, 'loading');
  await reader.load();
  assert.deepEqual(views, ['loading', 'ready']); assert.equal(reader.view.items.length, 20); assert.equal(reader.view.items[0]!.taskId, 'task-0045'); assert.equal(reader.view.nextCursor, 'task-0026');
  await reader.more();
  assert.equal(reader.view.items.length, 40); assert.equal(reader.view.items.at(-1)!.taskId, 'task-0006'); assert.equal(reader.view.nextCursor, 'task-0006');
  await reader.more();
  assert.equal(reader.view.items.length, 45); assert.equal(reader.view.nextCursor, undefined, 'the end of the log');
  await reader.more();
  assert.equal(service.calls.length, 3, 'nothing is asked after the end');
  assert.deepEqual(service.calls.map(call => [call.payload.cursor, call.payload.limit]), [[undefined, 20], ['task-0026', 20], ['task-0006', 20]]);
  assert.equal(new Set(reader.view.items.map(item => item.taskId)).size, 45);
  // Refreshing starts over from the newest.
  await reader.load();
  assert.equal(reader.view.items.length, 20); assert.equal(service.calls.at(-1)!.payload.cursor, undefined);
});

test('a page that repeats rows of the one before adds nothing twice', async () => {
  const pages = [{ items: [row(3), row(2)], nextCursor: 'task-0002' }, { items: [row(2), row(1)] }];
  const reader = new log.TriageLogReader(async () => pages.shift()!, () => {});
  await reader.load(); await reader.more();
  assert.deepEqual(reader.view.items.map(item => item.taskId), ['task-0003', 'task-0002', 'task-0001']);
});

test('a read that fails keeps what was read and where it stopped, and reading again goes on from there; only the service saying "not the maintainer" ends the log', async () => {
  const outcomes: Array<() => Promise<TriageLogPage>> = [
    async () => ({ items: [row(3), row(2)], nextCursor: 'task-0002' }), async () => { throw new Error('SERVICE_UNAVAILABLE'); }, async () => ({ items: [row(1)] }),
  ];
  const cursors: Array<string | undefined> = [];
  const reader = new log.TriageLogReader(async cursor => { cursors.push(cursor); return outcomes.shift()!(); }, () => {});
  await reader.load(); await reader.more();
  assert.equal(reader.view.status, 'failed'); assert.equal(reader.view.error, 'SERVICE_UNAVAILABLE'); assert.equal(reader.view.items.length, 2, 'the rows read before stay'); assert.equal(reader.view.nextCursor, 'task-0002');
  await reader.more();
  assert.equal(reader.view.status, 'ready'); assert.equal(reader.view.items.length, 3); assert.deepEqual(cursors, [undefined, 'task-0002', 'task-0002']);
  // A first page that fails has nothing and can start again.
  const first = new log.TriageLogReader(async () => { throw new Error('INVALID_DATA'); }, () => {});
  await first.load(); assert.equal(first.view.status, 'failed'); assert.deepEqual(first.view.items, []); assert.equal(first.view.error, 'INVALID_DATA');
  await first.more(); assert.equal(first.view.status, 'failed', 'there is no page to go on from');
  // Not the maintainer: no log, no rows, nothing more to read.
  let reads = 0;
  const denied = new log.TriageLogReader(async () => { reads++; throw new Error('MAINTAINER_REQUIRED'); }, () => {});
  await denied.load(); await denied.more();
  assert.equal(denied.view.status, 'denied'); assert.deepEqual(denied.view.items, []); assert.equal(reads, 1);
  // An error that is not an Error is the service being unavailable.
  const odd = new log.TriageLogReader(async () => { throw 'nope'; }, () => {}); await odd.load(); assert.equal(odd.view.error, 'SERVICE_UNAVAILABLE');
});

test('a read that was overtaken, or answers after the list was closed, changes nothing', async () => {
  const first = deferred<TriageLogPage>(), second = deferred<TriageLogPage>(), reads = [first, second];
  const reader = new log.TriageLogReader(() => reads.shift()!.promise, () => {});
  const stale = reader.load(), current = reader.load();
  second.resolve({ items: [row(9)] }); await current;
  first.resolve({ items: [row(1)] }); await stale;
  assert.deepEqual(reader.view.items.map(item => item.taskId), ['task-0009'], 'the older answer did not replace the newer one');
  const late = deferred<TriageLogPage>(), views: string[] = [];
  const closing = new log.TriageLogReader(() => late.promise, view => views.push(view.status));
  const pending = closing.load(); closing.close(); late.resolve({ items: [row(1)] }); await pending;
  assert.deepEqual(views, ['loading'], 'nothing was shown after it was closed');
  // A page being read is not read twice, and refreshing while it is read wins over it.
  const slow = deferred<TriageLogPage>(); let asked = 0;
  const busy = new log.TriageLogReader(() => { asked++; return asked === 1 ? Promise.resolve({ items: [row(2)], nextCursor: 'task-0002' }) : slow.promise; }, () => {});
  await busy.load(); const more = busy.more(); void busy.more(); void busy.load(); slow.resolve({ items: [row(1)] }); await more;
  assert.equal(asked, 3, 'the second "more" did nothing while the first was being read');
});

test('the reason a Bot stayed silent is looked for from the newest verdict down, only as far as it must be', async () => {
  const service = serve(rows(120)), read = async (cursor: string | undefined, limit: number) => service.answer({ payload: { sharedBotId: BOT, conversationId: CHAT, ...(cursor ? { cursor } : {}), limit } } as Listed);
  const near = await log.findTriage(read, ['task-0120', 'task-0101']);
  assert.deepEqual([...near.keys()].sort(), ['task-0101', 'task-0120']); assert.equal(service.calls.length, 1, 'both are on the first page: one read of 50');
  assert.equal(service.calls[0]!.payload.limit, 50);
  service.calls.length = 0;
  const far = await log.findTriage(read, ['task-0120', 'task-0045']);
  assert.deepEqual([...far.keys()].sort(), ['task-0045', 'task-0120']); assert.equal(service.calls.length, 2);
  service.calls.length = 0;
  service.calls.length = 0;
  const long = serve(rows(260)), readLong = async (cursor: string | undefined, limit: number) => long.answer({ payload: { sharedBotId: BOT, conversationId: CHAT, ...(cursor ? { cursor } : {}), limit } } as Listed);
  const lost = await log.findTriage(readLong, ['task-0150', 'task-9999']);
  assert.deepEqual([...lost.keys()], ['task-0150'], 'a task with no row is not in the answer'); assert.equal(long.calls.length, log.TRIAGE_FIND_PAGES, 'and the search is not endless');
  assert.equal(log.TRIAGE_FIND_PAGES, 4);
  long.calls.length = 0;
  assert.equal((await log.findTriage(readLong, ['task-0001'], 2)).size, 0); assert.equal(long.calls.length, 2, 'the cap of pages is a parameter');
  assert.equal((await log.findTriage(read, ['task-9999'])).size, 0, 'a short log is read to its end and no further');
  service.calls.length = 0;
  assert.equal((await log.findTriage(read, [])).size, 0); assert.equal(service.calls.length, 0, 'nothing wanted, nothing read');
  // A cursor that does not move ends the search; a read that fails ends it with what it has.
  let repeats = 0;
  assert.equal((await log.findTriage(async () => { repeats++; return { items: [], nextCursor: 'task-0001' }; }, ['x'])).size, 0); assert.equal(repeats, 2);
  const partial = await log.findTriage(async cursor => { if (cursor) throw new Error('SERVICE_UNAVAILABLE'); return { items: [row(2)], nextCursor: 'task-0002' }; }, ['task-0002', 'task-0001']);
  assert.deepEqual([...partial.keys()], ['task-0002']);
});

test('the Bots of a Chat whose log a person reads there are the ones they maintain that answer on their own in it', () => {
  const chat = (more: Partial<TeamConversation> = {}) => conversation(CHAT, { kind: 'group', participants: ['user:bob', 'shared_bot:mine-on-0001', 'shared_bot:mine-off-0001', 'shared_bot:mine-set-0001', 'shared_bot:theirs-0001', 'shared_bot:older-0001'], ...more });
  const bots: SharedBotCard[] = [
    botCard({ sharedBotId: 'mine-on-0001', maintainer: { uid: 'alice', generation: 1 }, autoReply: { daily: 5 } }),
    botCard({ sharedBotId: 'mine-off-0001', maintainer: { uid: 'alice', generation: 1 } }),
    botCard({ sharedBotId: 'mine-set-0001', maintainer: { uid: 'alice', generation: 1 } }),
    botCard({ sharedBotId: 'theirs-0001', maintainer: { uid: 'owner', generation: 1 }, autoReply: { daily: 5 } }),
    botCard({ sharedBotId: 'older-0001', maintainer: { uid: 'alice', generation: 0 }, autoReply: { daily: 5 } }),
    botCard({ sharedBotId: 'elsewhere-0001', maintainer: { uid: 'alice', generation: 1 }, autoReply: { daily: 5 } }),
  ];
  const ids = (conversationValue: Pick<TeamConversation, 'participants' | 'autoReply'>, uid = 'alice') => log.triageBots(conversationValue, bots, uid, 1).map(bot => bot.sharedBotId);
  assert.deepEqual(ids(chat()), ['mine-on-0001'], 'a Bot that never answers on its own has no log to read, a Bot of someone else is not theirs, and one that is not in the Chat is not there');
  assert.deepEqual(ids(chat({ autoReply: { revision: 1, targets: ['mine-set-0001'] } })), ['mine-on-0001', 'mine-set-0001'], 'one switched on in this Chat is');
  assert.deepEqual(ids(chat(), 'bob'), [], 'a person who maintains none has none');
  assert.deepEqual(ids(chat({ participants: ['user:alice', 'shared_bot:mine-on-0001'] })), ['mine-on-0001'], 'taking part in the Chat is not needed');
  assert.equal(log.maintainsBot(bots[4]!, 'alice', 1), false, 'by the membership they hold now'); assert.equal(log.maintainsBot(bots[0]!, 'alice', 1), true);
});

// ───────────────────────────── on screen ─────────────────────────────

type Context = Parameters<Parameters<typeof scenario>[1]>[0];
async function open(context: Context, bots: SharedBotCard[]) {
  withDom(context);
  const fake = new FakeTeams('alice'); fake.teams = [team('team-test', { role: 'owner' })]; fake.bots = bots;
  const workspace = new TeamsWorkspace(fake.api, 'alice', { storage: memoryStorage, environment: new EventTarget() }); context.after(() => workspace.close());
  await until(() => workspace.getSnapshot().botsLoaded && !!workspace.getSnapshot().detail, 'the Team');
  return { fake, workspace };
}
const mine = (more: Partial<SharedBotCard> = {}) => botCard({ maintainer: { uid: 'alice', generation: 1 }, autoReply: { daily: 7 }, ...more });
const directory = (workspace: TeamsWorkspace) => (s: ReturnType<TeamsWorkspace['getSnapshot']>) => createElement(BotDirectory, {
  api: workspace.api, uid: s.uid, team: s.team!, members: s.directory, bots: s.bots, locale: 'en', busy: s.messageBusy, status: '', attempt: workspace.attempt, onOpen: bot => workspace.startBotDm(bot), onChanged: workspace.refreshBots });
type Page = Awaited<ReturnType<typeof mount>>;
const entries = (page: Page) => page.find(node => node.type === 'button' && textOf(node) === 'Auto-reply log');
const listed = (calls: MessageCommand[]) => calls.filter((command): command is Listed => command.action === 'listTriage');
/** Serve the log and the other reads the views make; `extra` answers anything else. */
function wire(fake: FakeTeams, service: ReturnType<typeof serve>, extra: (command: MessageCommand) => unknown = () => ({}), tasks: () => unknown[] = () => []) {
  const calls: MessageCommand[] = [];
  fake.messages = async <T>(command: MessageCommand) => {
    calls.push(structuredClone(command));
    if (command.action === 'listTriage') return service.answer(command) as T;
    if (command.action === 'listTasks') return { tasks: tasks() } as T;
    if (command.action === 'listSharedBots') return { bots: structuredClone(fake.bots) } as T;
    return extra(command) as T;
  };
  return calls;
}
const dialogText = (page: Page) => page.text();

scenario('A Bot a person maintains that answers on its own has an Auto-reply log on its card; nobody else is offered it, and nothing is read until it is opened', async context => {
  const { fake, workspace } = await open(context, [mine(), mine({ sharedBotId: 'plain-bot-0001', name: 'Plain Bot', autoReply: undefined }), botCard({ sharedBotId: 'theirs-bot-0001', name: 'Their Bot', maintainer: { uid: 'owner', generation: 1 }, autoReply: { daily: 3 } })]);
  const calls = wire(fake, serve(rows(3)));
  await workspace.refreshBots(); await until(() => workspace.getSnapshot().bots.length === 3, 'the Bots');
  const page = await mount(workspace, directory(workspace)); context.after(() => page.unmount());
  assert.equal(entries(page).length, 1, 'one card of three: the one that is theirs and answers on its own');
  assert.equal(textOf(entries(page)[0]!.parent!.parent!).includes('Fixture Bot'), true, 'on the card of that Bot');
  assert.equal(listed(calls).length, 0, 'nothing is read before the log is opened');
});

scenario('Opening the log shows the newest verdicts first: what was decided, why, how likely, in which Chat, when, and where it ended up; a page at a time', async context => {
  const { fake, workspace } = await open(context, [mine()]);
  const all = rows(25);
  all[0] = row(25, { state: 'completed', conversationTitle: 'Trip' }, { action: 'run', source: 'rule', reason: 'keyword', probability: undefined, at: 1_700_000_500_000 });
  all[1] = row(24, { state: 'failed' }, { action: 'run_may_decline', source: 'decision', reason: 'between_thresholds', probability: 0.5 });
  all[3] = row(22, { state: 'running', conversationTitle: undefined });
  all[4] = row(21, { state: 'awaiting_approval', approval: 'held' }); all[5] = row(20, { state: 'completed', approval: 'approved' }); all[6] = row(19, { state: 'declined', approval: 'timeout' });
  all[7] = row(18, { state: 'declined', approval: 'rejected' }); all[8] = row(17, { state: 'declined', approval: 'superseded' });
  const service = serve(all), calls = wire(fake, service);
  const page = await mount(workspace, directory(workspace)); context.after(() => page.unmount());
  await until(() => entries(page).length === 1, 'the entry');
  await page.click(entries(page)[0]!);
  await until(() => page.find(node => node.type === 'li' && classesOf(node).includes('tw-triage-row')).length === 20, 'the first page');
  const first = listed(calls); assert.equal(first.length, 1);
  assert.deepEqual(first[0]!.payload, { sharedBotId: BOT, limit: 20 }); assert.equal(first[0]!.schemaVersion, 1);
  const items = () => page.find(node => node.type === 'li' && classesOf(node).includes('tw-triage-row'));
  const newest = textOf(items()[0]!);
  assert.match(newest, /^Reply/, 'what was decided'); assert.match(newest, /A keyword matched/, 'why'); assert.match(newest, /Trip/, 'the Chat'); assert.match(newest, /Replied$/, 'where it ended up');
  assert.doesNotMatch(newest, /likely/, 'no probability when the Decision service did not judge');
  assert.ok(newest.includes(new Intl.DateTimeFormat('en-US', { dateStyle: 'short', timeStyle: 'short' }).format(1_700_000_500_000)), 'when, in the language and the zone of the person');
  const second = textOf(items()[1]!);
  assert.match(second, /^Left to the Bot/); assert.match(second, /Not sure a reply was wanted/); assert.match(second, /50% likely/); assert.match(second, /Failed$/);
  const third = textOf(items()[2]!);
  assert.match(third, /^No reply/); assert.match(third, /A reply seemed unlikely to be wanted/); assert.match(third, /12% likely/); assert.match(third, /Planning/); assert.match(third, /Stayed silent$/);
  assert.equal(items()[0]!.props['data-action'], 'run'); assert.equal(items()[2]!.props['data-action'], 'decline');
  const fourth = textOf(items()[3]!); assert.match(fourth, /Chat · /, 'a Chat with no title is still named'); assert.match(fourth, /In progress$/);
  // A reply held for the maintainer says so while it waits, and how it came out after.
  const held = textOf(items()[4]!); assert.match(held, /Waiting for approval$/); assert.doesNotMatch(held, /Approved|Declined|Not approved|Dropped/);
  const approved = textOf(items()[5]!); assert.match(approved, /· ApprovedReplied$/); const timeout = textOf(items()[6]!); assert.match(timeout, /· Not approved in timeStayed silent$/);
  assert.match(textOf(items()[7]!), /· DeclinedStayed silent$/); assert.match(textOf(items()[8]!), /· Dropped for a newer messageStayed silent$/);
  assert.match(dialogText(page), /Only the decision and the reason are kept/, 'it says what is kept');
  assert.equal(page.has('Load more'), true);
  await page.click(page.button('Load more'));
  assert.equal(items().length, 25); assert.equal(page.has('Load more'), false, 'the end of the log');
  const next = listed(calls)[1]!.payload; assert.deepEqual(next, { sharedBotId: BOT, cursor: 'task-0006', limit: 20 });
  // Refresh reads from the newest again.
  await page.click(page.button('Refresh'));
  assert.equal(items().length, 20); assert.deepEqual(listed(calls).at(-1)!.payload, { sharedBotId: BOT, limit: 20 });
  assert.equal(page.has('Close'), true); await page.click(page.button('Close'));
  assert.equal(page.has('Refresh'), false, 'closed');
});

const classesOf = (node: { props: { className?: unknown } }): string[] => String(node.props.className ?? '').split(' ');

scenario('An empty log says so; a log that cannot be read says why and can be tried again, and a page that fails keeps the rows above it', async context => {
  const { fake, workspace } = await open(context, [mine()]);
  const service = serve([]); let failing = 1; const calls = wire(fake, service);
  const inner = fake.messages; fake.messages = async <T>(command: MessageCommand) => { if (command.action === 'listTriage' && failing > 0) { failing--; calls.push(command); throw new Error('SERVICE_UNAVAILABLE'); } return inner<T>(command); };
  const page = await mount(workspace, directory(workspace)); context.after(() => page.unmount());
  await until(() => entries(page).length === 1, 'the entry');
  await page.click(entries(page)[0]!);
  await until(() => page.find(node => node.props.role === 'alert').length === 1, 'the failure');
  assert.equal(textOf(page.find(node => node.props.role === 'alert')[0]!), messageErrorText('SERVICE_UNAVAILABLE', 'en'));
  assert.equal(entries(page).length, 1, 'an error that is not "not the maintainer" keeps the entry');
  await page.click(page.button('Retry'));
  assert.match(dialogText(page), /Nothing yet\. A line appears here for every message the Bot judges\./); assert.equal(page.has('Retry'), false); assert.equal(page.has('Load more'), false);
  // A later page that fails keeps the rows above it and can be asked for again with Load more.
  service.calls.length = 0;
  const big = serve(rows(30)); fake.messages = async <T>(command: MessageCommand) => {
    if (command.action !== 'listTriage') return {} as T;
    calls.push(command); if (command.payload.cursor && failing++ === 0) throw new Error('SERVICE_UNAVAILABLE'); return big.answer(command) as T;
  };
  await page.click(page.button('Refresh'));
  assert.equal(page.find(node => node.type === 'li' && classesOf(node).includes('tw-triage-row')).length, 20);
  await page.click(page.button('Load more'));
  assert.equal(page.find(node => node.type === 'li' && classesOf(node).includes('tw-triage-row')).length, 20, 'the rows read before stay');
  assert.equal(page.find(node => node.props.role === 'alert').length, 1); assert.equal(page.has('Load more'), true, 'and the next page can be asked for again');
  await page.click(page.button('Load more'));
  assert.equal(page.find(node => node.type === 'li' && classesOf(node).includes('tw-triage-row')).length, 30); assert.equal(page.find(node => node.props.role === 'alert').length, 0);
});

scenario('When the service says the person is not the Bot\'s maintainer the entry is gone and nothing of the log stays on screen', async context => {
  const { fake, workspace } = await open(context, [mine()]);
  const calls: MessageCommand[] = [];
  fake.messages = async <T>(command: MessageCommand) => { calls.push(command); if (command.action === 'listTriage') throw new Error('MAINTAINER_REQUIRED'); return {} as T; };
  const page = await mount(workspace, directory(workspace)); context.after(() => page.unmount());
  await until(() => entries(page).length === 1, 'the entry');
  await page.click(entries(page)[0]!);
  await until(() => entries(page).length === 0, 'the entry to go');
  assert.equal(page.has('Refresh'), false, 'the dialog is closed'); assert.equal(page.find(node => node.props.role === 'alert').length, 0, 'and says nothing: there is nothing for this person to retry');
  assert.equal(listed(calls).length, 1);
});

/** A group Chat of Bob's with the Bot of `bots` in it, open on screen for `alice`. */
async function openChat(context: Context, options: { bots: SharedBotCard[]; kind?: TeamConversation['kind']; participants?: string[]; tasks?: unknown[]; rows?: TriageLogItem[]; autoReply?: TeamConversation['autoReply'] }) {
  const { fake, workspace } = await open(context, options.bots);
  const service = serve(options.rows ?? rows(3)), calls = wire(fake, service, () => ({}), () => options.tasks ?? []);
  fake.conversations.set('team-test', [conversation(CHAT, { kind: options.kind ?? 'group', title: 'Planning', participants: options.participants ?? ['user:alice', 'user:bob', `shared_bot:${BOT}`], ...(options.autoReply ? { autoReply: options.autoReply } : {}) })]);
  await workspace.refresh(); await workspace.refreshBots(); workspace.selectConversation(CHAT);
  await until(() => workspace.getSnapshot().selectedConversationId === CHAT && workspace.getSnapshot().botsLoaded && workspace.getSnapshot().bots.length === options.bots.length, 'the Chat');
  const page = await mount(workspace, snapshot => createElement(TeamConversationView, { workspace, snapshot, locale: 'en' })); context.after(() => page.unmount());
  await until(() => page.find(node => node.type === 'header').length > 0, 'the header');
  return { fake, workspace, page, calls, service };
}

scenario('In a Chat the log is one button in the header beside the auto-reply switch, for the maintainer of a Bot of the Chat, and shows only that Chat', async context => {
  const { page, calls } = await openChat(context, { bots: [mine()], rows: [...rows(2, () => ({ conversationId: 'other-chat' })).map((item, index) => ({ ...item, taskId: `task-90${index}0` })), ...rows(3)] });
  assert.equal(page.byLabel('Bots that answer on their own').length, 1, 'the auto-reply setting is there'); assert.equal(page.byLabel('Auto-reply log').length, 1);
  assert.equal(listed(calls).length, 0, 'it is read when opened, not before');
  await page.click(page.oneByLabel('Auto-reply log'));
  await until(() => page.find(node => node.type === 'li' && classesOf(node).includes('tw-triage-row')).length === 3, 'the rows of this Chat');
  assert.deepEqual(listed(calls)[0]!.payload, { sharedBotId: BOT, conversationId: CHAT, limit: 20 });
  assert.match(dialogText(page), /Fixture Bot/); assert.match(dialogText(page), /Planning/);
  assert.equal(page.find(node => node.type === 'select').length, 0, 'one Bot, no choice to make');
});

scenario('A Chat with two Bots of the same maintainer lets them choose whose log to read; each is read for this Chat', async context => {
  const second = mine({ sharedBotId: 'second-bot-0001', name: 'Second Bot' });
  const { page, calls } = await openChat(context, { bots: [mine(), second], participants: ['user:alice', `shared_bot:${BOT}`, 'shared_bot:second-bot-0001'] });
  await page.click(page.oneByLabel('Auto-reply log'));
  await until(() => listed(calls).length === 1, 'the first log');
  await page.type(page.find(node => node.type === 'select')[0]!, 'second-bot-0001');
  await until(() => listed(calls).length === 2, 'the second log');
  assert.deepEqual(listed(calls).map(call => call.payload.sharedBotId), [BOT, 'second-bot-0001']); assert.equal(listed(calls)[1]!.payload.conversationId, CHAT);
});

scenario('A Chat the person does not take part in still offers the log to the maintainer of a Bot in it; nobody else sees the button', async context => {
  const outside = await openChat(context, { bots: [mine()], participants: ['user:bob', `shared_bot:${BOT}`] });
  assert.equal(outside.page.byLabel('Auto-reply log').length, 1, 'a maintainer need not be in the Chat to read it');
  assert.equal(outside.page.byLabel('Bots that answer on their own').length, 0, 'while the switch is for the people in it');
  const others = await openChat(context, { bots: [mine({ maintainer: { uid: 'owner', generation: 1 } })] });
  assert.equal(others.page.byLabel('Auto-reply log').length, 0); assert.equal(listed(others.calls).length, 0);
  const plain = await openChat(context, { bots: [mine({ autoReply: undefined })] });
  assert.equal(plain.page.byLabel('Auto-reply log').length, 0, 'a Bot that never answers on its own has no log to read');
  const switched = await openChat(context, { bots: [mine({ autoReply: undefined })], autoReply: { revision: 1, targets: [BOT] } });
  assert.equal(switched.page.byLabel('Auto-reply log').length, 1, 'one that is switched on in this Chat has');
});

scenario('A Bot DM says why the Bot did not reply to its maintainer, from the log of that Chat, once; the people who only talk to the Bot see the line they always saw', async context => {
  const declined = (taskId: string, more: Record<string, unknown> = {}) => ({ taskId, conversationId: CHAT, sharedBotId: BOT, requester: { uid: 'alice', generation: 1 }, state: 'declined', optional: true, createdAt: 1, deadline: 2, epoch: 1, activeSpentMs: 0, ...more });
  const log = [row(3, { taskId: 'task-0003' }, { reason: 'quiet_hours', source: 'quiet', probability: undefined }), row(2, { taskId: 'task-0002' }), row(1, { taskId: 'task-0001' })];
  const tasks = [declined('task-0003'), declined('task-0002', { usage: { status: 'complete', inputTokens: 1200, outputTokens: 0 } }), declined('task-0009')];
  const maintainer = await openChat(context, { bots: [mine()], kind: 'bot_dm', participants: ['user:alice', `shared_bot:${BOT}`], tasks, rows: log });
  await until(() => /In quiet hours/.test(maintainer.page.text()), 'the reason');
  const lines = maintainer.page.find(node => node.type === 'p' && classesOf(node).includes('team-bot-run-declined')).map(textOf).sort();
  assert.deepEqual(lines, ['Did not reply', 'Did not reply · A reply seemed unlikely to be wanted · 12% likely · 1.2K tokens · cost unknown', 'Did not reply · In quiet hours'],
    'with the reason and how likely a reply was thought, the cost still shown, and a task that has no row only the line it had');
  const asked = listed(maintainer.calls); assert.equal(asked.length, 1, 'one read of the log for the three tasks, not one each');
  assert.deepEqual(asked[0]!.payload, { sharedBotId: BOT, conversationId: CHAT, limit: 50 });
  await new Promise(resolve => setTimeout(resolve, 1200));
  assert.equal(listed(maintainer.calls).length, 1, 'and not again while the same tasks are listed');
  assert.equal(maintainer.page.byLabel('Auto-reply log').length, 1, 'the whole log of the Chat is one button away');

  const others = await openChat(context, { bots: [mine({ maintainer: { uid: 'owner', generation: 1 } })], kind: 'bot_dm', participants: ['user:alice', `shared_bot:${BOT}`], tasks, rows: log });
  await until(() => others.page.find(node => node.type === 'p' && classesOf(node).includes('team-bot-run-declined')).length === 3, 'the quiet lines');
  assert.deepEqual(others.page.find(node => node.type === 'p' && classesOf(node).includes('team-bot-run-declined')).map(textOf).sort(), ['Did not reply', 'Did not reply', 'Did not reply · 1.2K tokens · cost unknown']);
  assert.equal(listed(others.calls).length, 0, 'nothing is read for a person who does not maintain the Bot');

  const quiet = await openChat(context, { bots: [mine()], kind: 'bot_dm', participants: ['user:alice', `shared_bot:${BOT}`], tasks: [declined('task-0003', { sharedBotId: 'another-bot-0001' }), { ...declined('task-0004'), state: 'completed', optional: undefined }], rows: log });
  await new Promise(resolve => setTimeout(resolve, 50));
  assert.equal(listed(quiet.calls).length, 0, 'nothing is read while no task of the Bot has stayed silent');
});

scenario('The reason of a silence is looked for once the connection is there; while it is not nothing is read and nothing is given up on', async context => {
  const declined = { taskId: 'task-0003', conversationId: CHAT, sharedBotId: BOT, requester: { uid: 'alice', generation: 1 }, state: 'declined', optional: true, createdAt: 1, deadline: 2, epoch: 1, activeSpentMs: 0 };
  const { fake, workspace } = await open(context, [mine()]);
  const service = serve([row(3, { taskId: 'task-0003' }, { reason: 'quiet_hours', source: 'quiet', probability: undefined })]), calls = wire(fake, service, () => ({}), () => [declined]);
  fake.conversations.set('team-test', [conversation(CHAT, { kind: 'bot_dm', participants: ['user:alice', `shared_bot:${BOT}`] })]);
  await workspace.refresh(); await workspace.refreshBots(); workspace.selectConversation(CHAT);
  await until(() => workspace.getSnapshot().selectedConversationId === CHAT && workspace.getSnapshot().botsLoaded, 'the Chat');
  let offline = true;
  const page = await mount(workspace, snapshot => createElement(TeamConversationView, { workspace, snapshot: { ...snapshot, offline }, locale: 'en' })); context.after(() => page.unmount());
  await new Promise(resolve => setTimeout(resolve, 80));
  assert.equal(listed(calls).length, 0, 'nothing is read while there is no connection');
  assert.deepEqual(page.find(node => node.type === 'p' && classesOf(node).includes('team-bot-run-declined')).map(textOf), ['Did not reply']);
  await page.during(async () => { offline = false; await workspace.refresh(); });
  await until(() => /In quiet hours/.test(page.text()), 'the reason, once the connection is back');
  assert.equal(listed(calls).length, 1, 'read once');
});

scenario('The reason of a silence is a courtesy: a log that cannot be read leaves the line as it was and shows no error', async context => {
  const declined = { taskId: 'task-0003', conversationId: CHAT, sharedBotId: BOT, requester: { uid: 'alice', generation: 1 }, state: 'declined', optional: true, createdAt: 1, deadline: 2, epoch: 1, activeSpentMs: 0 };
  const { fake, workspace } = await open(context, [mine()]);
  const calls: MessageCommand[] = [];
  fake.messages = async <T>(command: MessageCommand) => { calls.push(command); if (command.action === 'listTasks') return { tasks: [declined] } as T; if (command.action === 'listTriage') throw new Error('MAINTAINER_REQUIRED'); return { bots: fake.bots } as T; };
  fake.conversations.set('team-test', [conversation(CHAT, { kind: 'bot_dm', participants: ['user:alice', `shared_bot:${BOT}`] })]);
  await workspace.refresh(); await workspace.refreshBots(); workspace.selectConversation(CHAT);
  await until(() => workspace.getSnapshot().selectedConversationId === CHAT && workspace.getSnapshot().botsLoaded, 'the Chat');
  const page = await mount(workspace, snapshot => createElement(TeamConversationView, { workspace, snapshot, locale: 'en' })); context.after(() => page.unmount());
  await until(() => listed(calls).length === 1, 'the read');
  await new Promise(resolve => setTimeout(resolve, 30));
  assert.deepEqual(page.find(node => node.type === 'p' && classesOf(node).includes('team-bot-run-declined')).map(textOf), ['Did not reply']);
  assert.equal(page.find(node => node.props.role === 'alert').length, 0);
});

test('the log\'s words are in the shared catalog in every language, each different from the Simplified Chinese they were written from', () => {
  const t = teamsText;
  for (const key of ['teams_triage_title', 'teams_triage_intro', 'teams_triage_loading', 'teams_triage_empty', 'teams_triage_more', 'teams_triage_chat_untitled'] as const) {
    const values = new Set(['en', 'zh-Hans', 'zh-Hant', 'ja', 'ko'].map(locale => t(locale)(key)));
    assert.equal(values.size, 5, key); assert.ok(![...values].some(value => value === key || value === ''), key);
  }
  assert.equal(t('zh-Hant')('teams_triage_title'), '自主回覆紀錄'); assert.equal(t('en')('teams_triage_title'), 'Auto-reply log');
  assert.doesNotMatch(['en', 'zh-Hans', 'zh-Hant', 'ja', 'ko'].map(locale => t(locale)('teams_triage_intro')).join(' '), /assistant|助手|アシスタント|어시스턴트/iu, 'a Bot is never called an assistant');
});
