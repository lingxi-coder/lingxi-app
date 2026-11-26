import assert from 'node:assert/strict';
import test from 'node:test';
import { createElement } from 'react';
import { TeamsWorkspace } from '../../../packages/teams/workspace';
import type { HeldReply, HeldReplyDecision, SharedBotCard } from '../../../packages/teams/execution';
import {
  HELD_REPLY_ENDED, HELD_TICK_MS, HeldRepliesBoard, REPLY_NOT_POSTED, heldEdit, heldReplyBots, placeHeld, readHeldRepliesOf, type HeldBotReply, type HeldRow, type HeldView,
} from '../../../packages/teams/held-replies';
import type { MessageCommand, TeamConversation } from '../../../packages/teams/messages';
import { FakeTeams, botCard, conversation, memoryStorage, team, until } from './support/fakeTeams';
import { loadTeamsModule } from './support/loadTeamsModule';
import { mount, scenario, textOf, withDom } from './support/workspacePage';

// Bot v3 B6b: what the maintainer of a Bot reads and decides when the Bot's reply waits for them: in the Chat, in the list on the Bot's card, and in every language.
const { BotDirectory } = loadTeamsModule<typeof import('../../../packages/teams/BotDirectory')>('BotDirectory');
const { TeamConversationView } = loadTeamsModule<typeof import('../../../packages/teams/TeamConversationView')>('TeamConversationView');
const { teamsText } = loadTeamsModule<typeof import('../../../packages/teams/i18n')>('i18n');
const { heldReplyErrorText, messageErrorText } = loadTeamsModule<typeof import('../../../packages/teams/errors')>('errors');

const BOT = 'assistant-fixture', CHAT = 'test-chat', TASK = 'task-held-0001', SCOPE = { teamId: 'team-test', generation: 1 };
const LOCALES = ['en', 'zh-Hans', 'zh-Hant', 'ja', 'ko'];
const MINUTE = 60_000;

/** A reply that waits: made a minute ago, nine minutes left (less the twenty seconds the clock of a slow test may take, so "9 min" stays 9). */
const held = (more: Partial<HeldReply> = {}): HeldReply => ({
  taskId: TASK, conversationId: CHAT, messageId: 'message-2', requester: { uid: 'bob', generation: 1 }, unasked: false, heldAt: Date.now() - MINUTE,
  expiresAt: Date.now() + 9 * MINUTE - 20_000, text: 'Dinner at 7 works for me.', conversationTitle: 'Planning', ...more,
});
const heldBot = (more: Partial<HeldReply> = {}): HeldBotReply => ({ ...held(more), sharedBotId: BOT });
const waitingTask = (more: Record<string, unknown> = {}) => ({
  taskId: TASK, conversationId: CHAT, sharedBotId: BOT, requester: { uid: 'bob', generation: 1 }, state: 'awaiting_approval', createdAt: 1, deadline: Date.now() + 9 * MINUTE, epoch: 1, activeSpentMs: 0, ...more,
});
const deferred = <T>() => { let resolve!: (value: T) => void, reject!: (error: unknown) => void; const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const flush = async (turns = 6) => { for (let i = 0; i < turns; i++) await new Promise(resolve => setImmediate(resolve)); };

// ───────────────────────────── what may be sent, whose replies, and where they sit ─────────────────────────────

test('the maintainer\'s text is what the service takes: trimmed, a character at least and 8192 bytes at most; the Bot\'s own words are sent as no text at all', () => {
  assert.deepEqual(heldEdit('Dinner at 7.', '  Dinner at 8.  '), { ok: true, text: 'Dinner at 8.' }, 'trimmed');
  assert.deepEqual(heldEdit('Dinner at 7.', 'Dinner at 7.'), { ok: true, text: undefined }, 'nothing changed: the reply goes as the Bot made it, and is not recorded as changed');
  assert.deepEqual(heldEdit('Dinner at 7.\n', '\n  Dinner at 7.  '), { ok: true, text: undefined }, 'only spaces and lines at the ends changed');
  for (const edited of ['', '   ', '\n\t ']) assert.deepEqual(heldEdit('Dinner', edited), { ok: false, problem: 'empty' }, JSON.stringify(edited));
  assert.deepEqual(heldEdit('x', '€'.repeat(2731)), { ok: false, problem: 'too_long' }, '2731 euro signs are 8193 bytes');
  assert.deepEqual(heldEdit('x', '€'.repeat(2730)), { ok: true, text: '€'.repeat(2730) }, '2730 are 8190');
  assert.deepEqual(heldEdit('x', `  ${'x'.repeat(8192)}  `), { ok: true, text: 'x'.repeat(8192) }, 'the spaces around are not counted');
  assert.deepEqual(heldEdit('x', 'x'.repeat(8193)), { ok: false, problem: 'too_long' });
  assert.deepEqual([...HELD_REPLY_ENDED].sort(), ['MAINTAINER_REQUIRED', 'REPLY_APPROVED', 'REPLY_EXPIRED', 'REPLY_NOT_HELD', 'REPLY_REJECTED', 'REPLY_SUPERSEDED', 'TASK_REQUIRED']);
});

test('the Bots of a Chat whose replies a person decides on are the ones they maintain that are in it, answering when asked or on their own', () => {
  const chat = conversation(CHAT, { kind: 'group', participants: ['user:bob', 'shared_bot:mine-0001', 'shared_bot:mine-on-0001', 'shared_bot:theirs-0001', 'shared_bot:older-0001'] });
  const bots: SharedBotCard[] = [
    botCard({ sharedBotId: 'mine-0001', maintainer: { uid: 'alice', generation: 1 } }), botCard({ sharedBotId: 'mine-on-0001', maintainer: { uid: 'alice', generation: 1 }, autoReply: { daily: 5 } }),
    botCard({ sharedBotId: 'theirs-0001', maintainer: { uid: 'owner', generation: 1 } }), botCard({ sharedBotId: 'older-0001', maintainer: { uid: 'alice', generation: 0 } }),
    botCard({ sharedBotId: 'elsewhere-0001', maintainer: { uid: 'alice', generation: 1 } }),
  ];
  assert.deepEqual(heldReplyBots(chat, bots, 'alice', 1).map(bot => bot.sharedBotId), ['mine-0001', 'mine-on-0001'], 'a Bot that holds replies need not answer on its own; one of someone else, of an older membership or outside the Chat is not theirs here');
  assert.deepEqual(heldReplyBots(chat, bots, 'bob', 1), [], 'a person who maintains none has none');
  assert.deepEqual(heldReplyBots(conversation(CHAT, { participants: ['user:alice'] }), bots, 'alice', 1), [], 'a Chat with no Bot in it has none');
});

test('a reply sits with its task when the Chat lists the task; a reply nobody asked for has no task there and is a row of its own, oldest first; an asked one waits for its task', () => {
  const row = (taskId: string, more: Partial<HeldReply> = {}): HeldRow => ({ reply: { ...heldBot({ taskId, ...more }) }, busy: undefined, error: undefined, ended: undefined });
  const rows = [row('t-new', { unasked: true, heldAt: 3000 }), row('t-asked-listed', { heldAt: 2500 }), row('t-asked-unlisted', { heldAt: 2200 }), row('t-old', { unasked: true, heldAt: 2000 }), row('t-unasked-listed', { unasked: true, heldAt: 1000 })];
  const placed = placeHeld(rows, new Set(['t-asked-listed', 't-unasked-listed']));
  assert.deepEqual([...placed.byTask.keys()].sort(), ['t-asked-listed', 't-unasked-listed']);
  assert.deepEqual(placed.orphans.map(item => item.reply.taskId), ['t-old', 't-new'], 'the stream reads oldest first; a reply whose task is not listed yet is not shown as a row of its own');
  assert.deepEqual(placeHeld([], new Set()), { byTask: new Map(), orphans: [] });
});

test('the replies of several Bots are read for each of them, tagged with the Bot they came from, newest first; a Bot that is not theirs adds nothing, and any other failure is the failure', async () => {
  const answers: Record<string, () => unknown> = {
    'bot-a-0001': () => ({ replies: [held({ taskId: 'a-old', heldAt: 1000 }), held({ taskId: 'a-new', heldAt: 3000 })] }),
    'bot-b-0001': () => ({ replies: [held({ taskId: 'b-mid', heldAt: 2000 })] }),
    'bot-c-0001': () => { throw new Error('MAINTAINER_REQUIRED'); },
    'bot-d-0001': () => { throw new Error('SERVICE_UNAVAILABLE'); },
  };
  const sent: MessageCommand[] = [];
  const api = { messages: async <T>(command: MessageCommand) => { sent.push(structuredClone(command)); return answers[(command as { payload: { sharedBotId: string } }).payload.sharedBotId]!() as T; } };
  const read = await readHeldRepliesOf(api, SCOPE, ['bot-a-0001', 'bot-b-0001', 'bot-c-0001'], CHAT);
  assert.deepEqual(read.map(reply => [reply.taskId, reply.sharedBotId]), [['a-new', 'bot-a-0001'], ['b-mid', 'bot-b-0001'], ['a-old', 'bot-a-0001']]);
  assert.deepEqual(sent.map(command => (command as { payload: unknown }).payload), [{ sharedBotId: 'bot-a-0001', conversationId: CHAT }, { sharedBotId: 'bot-b-0001', conversationId: CHAT }, { sharedBotId: 'bot-c-0001', conversationId: CHAT }]);
  await assert.rejects(readHeldRepliesOf(api, SCOPE, ['bot-c-0001']), /MAINTAINER_REQUIRED/, 'every Bot refusing is the person not being a maintainer');
  await assert.rejects(readHeldRepliesOf(api, SCOPE, ['bot-a-0001', 'bot-d-0001']), /SERVICE_UNAVAILABLE/);
  assert.deepEqual(await readHeldRepliesOf(api, SCOPE, []).catch(error => (error as Error).message), 'MAINTAINER_REQUIRED', 'no Bot, no replies to be shown');
  sent.length = 0; await readHeldRepliesOf(api, SCOPE, ['bot-b-0001']);
  assert.deepEqual((sent[0] as { payload: unknown }).payload, { sharedBotId: 'bot-b-0001' }, 'without a Chat, all of them');
});

// ───────────────────────────── the list and the decisions on it, without a screen ─────────────────────────────

/** A board over a list the test changes, and decisions it answers; `views` is every view it published. */
function board(initial: HeldBotReply[], more: { clock?: () => number } = {}) {
  const state = { replies: structuredClone(initial), reads: 0, approvals: [] as Array<{ taskId: string; text?: string; commandId: string }>, rejections: [] as Array<{ taskId: string; commandId: string }>,
    approve: async (taskId: string): Promise<HeldReplyDecision> => { state.replies = state.replies.filter(reply => reply.taskId !== taskId); return { taskId, state: 'completed', replyId: 'reply-0001', artifactId: null }; },
    reject: async (taskId: string): Promise<HeldReplyDecision> => { state.replies = state.replies.filter(reply => reply.taskId !== taskId); return { taskId, state: 'declined', declineReason: 'approval_rejected' }; },
    read: async (): Promise<HeldBotReply[]> => structuredClone(state.replies) };
  const views: HeldView[] = [];
  let id = 0;
  const instance = new HeldRepliesBoard(async () => { state.reads++; return state.read(); }, {
    approve: async (taskId, options) => { state.approvals.push({ taskId, ...options }); return state.approve(taskId); },
    reject: async (taskId, options) => { state.rejections.push({ taskId, ...options }); return state.reject(taskId); },
  }, view => views.push(view), more.clock ?? Date.now, () => `command-${++id}`);
  return { board: instance, state, views };
}

test('the list is read when asked, newest first, with each reply\'s Bot; the first answer says it is ready, and a read that fails keeps what was read', async () => {
  const { board: list, state, views } = board([heldBot({ taskId: 'old', heldAt: 1000 }), heldBot({ taskId: 'new', heldAt: 5000 })]);
  assert.equal(list.current.status, 'loading'); assert.equal(list.current.rows.length, 0);
  await list.load();
  assert.equal(list.current.status, 'ready'); assert.deepEqual(list.current.rows.map(row => row.reply.taskId), ['new', 'old']); assert.equal(views.length, 1);
  assert.deepEqual(list.current.rows.map(row => [row.busy, row.error, row.ended]), [[undefined, undefined, undefined], [undefined, undefined, undefined]]);
  state.read = async () => { throw new Error('SERVICE_UNAVAILABLE'); };
  await list.load();
  assert.equal(list.current.status, 'failed'); assert.equal(list.current.error, 'SERVICE_UNAVAILABLE'); assert.equal(list.current.rows.length, 2, 'what was read stays');
  state.read = async () => structuredClone(state.replies);
  await list.load();
  assert.equal(list.current.status, 'ready'); assert.equal(list.current.error, undefined);
  state.read = async () => { throw new Error('MAINTAINER_REQUIRED'); };
  await list.load();
  assert.equal(list.current.status, 'denied'); assert.equal(list.current.rows.length, 0, 'the service says none of it is theirs: nothing of it stays');
});

test('reads do not pile up: one is made at a time, and requests that come while it is going on are answered by one more', async () => {
  const gates = [deferred<void>(), deferred<void>()], [firstGate, secondGate] = gates;
  const { board: list, state } = board([heldBot()]);
  state.read = async () => { const gate = gates.shift(); if (gate) await gate.promise; return structuredClone(state.replies); };
  const [first, second, third] = [list.load(), list.load(), list.load()];
  await flush(); assert.equal(state.reads, 1, 'one read is on its way');
  firstGate!.resolve(); await flush(); assert.equal(state.reads, 2, 'and one more follows it, for all the requests that came meanwhile');
  state.replies = []; secondGate!.resolve(); await Promise.all([first, second, third]); await flush();
  assert.equal(state.reads, 2); assert.equal(list.current.status, 'ready');
  const closing = board([heldBot()]); const stop = deferred<void>(); closing.state.read = async () => { await stop.promise; return structuredClone(closing.state.replies); };
  const pending = closing.board.load(); closing.board.close(); stop.resolve(); await pending;
  assert.equal(closing.views.length, 0, 'a read that answers after the list was closed changes nothing');
  await closing.board.load(); assert.equal(closing.state.reads, 1, 'and a closed list reads no more');
});

test('approving sends the decision once; a retry of the same decision is the same command, a different one is a new command; what the service took leaves the list', async () => {
  const { board: list, state } = board([heldBot(), heldBot({ taskId: 'task-2', heldAt: 1 })]);
  await list.load();
  state.approve = async () => { throw new Error('SERVICE_UNAVAILABLE'); };
  assert.equal(await list.approve(TASK), false);
  assert.equal(list.current.rows.find(row => row.reply.taskId === TASK)!.error, 'SERVICE_UNAVAILABLE'); assert.equal(list.current.rows.find(row => row.reply.taskId === TASK)!.busy, undefined, 'it can be tried again');
  assert.equal(await list.approve(TASK), false);
  assert.equal(await list.approve(TASK, 'Dinner at 8.'), false);
  assert.equal(await list.approve(TASK, 'Dinner at 8.'), false);
  state.approve = async taskId => { state.replies = state.replies.filter(reply => reply.taskId !== taskId); return { taskId, state: 'completed', replyId: 'reply-0001', artifactId: null }; };
  assert.equal(await list.approve(TASK, 'Dinner at 8.'), true);
  assert.deepEqual(state.approvals.map(call => [call.taskId, call.text, call.commandId]), [
    [TASK, undefined, 'command-1'], [TASK, undefined, 'command-1'], [TASK, 'Dinner at 8.', 'command-2'], [TASK, 'Dinner at 8.', 'command-2'], [TASK, 'Dinner at 8.', 'command-2']]);
  assert.deepEqual(list.current.rows.map(row => row.reply.taskId), ['task-2'], 'taken: it is gone');
  await flush();
  assert.equal(state.approvals.length, 5);
  assert.equal(await list.approve(TASK), false, 'a reply that is not on the list is not decided');
  // A decision that is on its way is not sent again, whatever is pressed.
  const slow = deferred<HeldReplyDecision>(); state.approve = () => slow.promise;
  const first = list.approve('task-2'); await flush();
  assert.equal(list.current.rows[0]!.busy, 'approve'); assert.equal(await list.approve('task-2'), false); assert.equal(await list.reject('task-2'), false); assert.equal(state.approvals.length, 6);
  slow.resolve({ taskId: 'task-2', state: 'completed', replyId: null, artifactId: null }); assert.equal(await first, true);
});

test('a reject is one command too, and drops the reply', async () => {
  const { board: list, state } = board([heldBot()]);
  await list.load();
  state.reject = async () => { throw new Error('SERVICE_UNAVAILABLE'); };
  assert.equal(await list.reject(TASK), false); assert.equal(await list.reject(TASK), false);
  state.reject = async taskId => ({ taskId, state: 'declined', declineReason: 'approval_rejected' });
  assert.equal(await list.reject(TASK), true);
  assert.deepEqual(state.rejections, [{ taskId: TASK, commandId: 'command-1' }, { taskId: TASK, commandId: 'command-1' }, { taskId: TASK, commandId: 'command-1' }]);
  assert.deepEqual(list.current.rows, []);
});

test('a reply the service says is no longer there stays on screen saying so until it is closed, and is not decided again; any other failure leaves it as it was', async () => {
  for (const code of ['REPLY_EXPIRED', 'REPLY_SUPERSEDED', 'REPLY_APPROVED', 'REPLY_REJECTED', 'REPLY_NOT_HELD', 'TASK_REQUIRED', 'MAINTAINER_REQUIRED']) {
    const { board: list, state } = board([heldBot()]);
    await list.load();
    state.approve = async () => { throw new Error(code); }; state.replies = [];
    assert.equal(await list.approve(TASK), false, code);
    await flush();
    const row = list.current.rows[0]!;
    assert.deepEqual([row.ended, row.busy, row.error], [code, undefined, undefined], `${code}: it says what became of it, and the list that no longer has it does not take it away`);
    assert.equal(await list.reject(TASK), false, 'nothing more is sent for it'); assert.equal(state.approvals.length, 1); assert.equal(state.rejections.length, 0);
    list.dismiss(TASK); assert.deepEqual(list.current.rows, [], `${code}: read, and gone`);
  }
  const { board: list, state } = board([heldBot()]);
  await list.load(); list.dismiss(TASK); assert.equal(list.current.rows.length, 1, 'a reply that is still to be decided is not dismissed');
  state.approve = async () => { throw new Error('INVALID_INPUT'); };
  await list.approve(TASK); assert.equal(list.current.rows[0]!.error, 'INVALID_INPUT'); assert.equal(list.current.rows[0]!.ended, undefined);
});

test('an approval the service took that posted nothing says so: the reply could no longer be posted', async () => {
  const { board: list, state } = board([heldBot()]);
  await list.load();
  state.approve = async taskId => ({ taskId, state: 'declined', declineReason: 'revoked', replyId: null, artifactId: null });
  state.replies = [];
  assert.equal(await list.approve(TASK), true, 'the decision was taken');
  await flush();
  assert.equal(list.current.rows[0]!.ended, REPLY_NOT_POSTED); assert.equal(heldReplyErrorText(REPLY_NOT_POSTED, 'en'), 'Not posted. The request or its access changed, so the reply was dropped.');
  list.dismiss(TASK); assert.deepEqual(list.current.rows, []);
});

test('a reply that is no longer listed goes quietly, unless it is being decided or its time ran out while it was shown: then it says so', async () => {
  let now = 1_000_000;
  const { board: list, state } = board([heldBot({ taskId: 'gone-elsewhere', expiresAt: now + 5 * MINUTE }), heldBot({ taskId: 'ran-out', expiresAt: now + MINUTE }), heldBot({ taskId: 'deciding', expiresAt: now + 8 * MINUTE })], { clock: () => now });
  await list.load();
  const slow = deferred<HeldReplyDecision>(); state.approve = () => slow.promise;
  const deciding = list.approve('deciding'); await flush();
  now += 2 * MINUTE; state.replies = [];
  await list.load();
  const byId = new Map(list.current.rows.map(row => [row.reply.taskId, row]));
  assert.equal(byId.has('gone-elsewhere'), false, 'dropped for a newer message, or decided on another screen: it simply goes');
  assert.equal(byId.get('ran-out')!.ended, 'REPLY_EXPIRED', 'its time ran out while it was shown: it says so'); assert.equal(heldReplyErrorText('REPLY_EXPIRED', 'en'), 'The time to decide ran out, so the reply was dropped.');
  assert.equal(byId.get('deciding')!.busy, 'approve', 'a decision that is on its way keeps its reply on screen');
  slow.resolve({ taskId: 'deciding', state: 'completed', replyId: null, artifactId: null }); await deciding; await flush();
  assert.deepEqual(list.current.rows.map(row => row.reply.taskId), ['ran-out']);
});

test('the time moves on every 30 seconds, and the list is read again when the first reply runs out', async context => {
  const start = Date.UTC(2026, 10, 25, 9, 0, 0);
  context.mock.timers.enable({ apis: ['setInterval', 'setTimeout', 'Date'], now: start });
  const { board: list, state, views } = board([heldBot({ taskId: 'first', expiresAt: start + 100_000 }), heldBot({ taskId: 'second', expiresAt: start + 400_000 })]);
  assert.equal(HELD_TICK_MS, 30_000);
  list.start(); await list.load(); await flush();
  assert.equal(list.current.now, start); assert.equal(state.reads, 1);
  const seen = views.length;
  context.mock.timers.tick(29_999); await flush(); assert.equal(views.length, seen, 'not before the 30 seconds');
  context.mock.timers.tick(1); await flush(); assert.equal(list.current.now, start + 30_000); assert.equal(views.length, seen + 1);
  context.mock.timers.tick(30_000); await flush(); assert.equal(list.current.now, start + 60_000);
  context.mock.timers.tick(30_000); await flush(); assert.equal(list.current.now, start + 90_000); assert.equal(state.reads, 1, 'a tick reads nothing');
  state.replies = state.replies.filter(reply => reply.taskId !== 'first');
  context.mock.timers.tick(10_499); await flush(); assert.equal(state.reads, 1, 'the first reply runs out at +100 s and is read for half a second later');
  context.mock.timers.tick(1); await flush(); assert.equal(state.reads, 2);
  const row = list.current.rows.find(item => item.reply.taskId === 'first')!;
  assert.equal(row.ended, 'REPLY_EXPIRED'); assert.ok(list.current.now >= start + 100_500, 'and the time is the time it ran out at, or later');
  assert.equal(list.current.rows.find(item => item.reply.taskId === 'second')!.ended, undefined);
  list.close();
  const after = views.length; context.mock.timers.tick(10 * HELD_TICK_MS); await flush(); assert.equal(views.length, after, 'a closed list keeps no time');
});

// ───────────────────────────── on screen ─────────────────────────────

type Context = Parameters<Parameters<typeof scenario>[1]>[0];
type Page = Awaited<ReturnType<typeof mount>>;
const classesOf = (node: { props: { className?: unknown } }): string[] => String(node.props.className ?? '').split(' ');
const mine = (more: Partial<SharedBotCard> = {}) => botCard({ maintainer: { uid: 'alice', generation: 1 }, ...more });
const theirs = (more: Partial<SharedBotCard> = {}) => botCard({ maintainer: { uid: 'owner', generation: 1 }, ...more });

/** What the service answers: the Chat's tasks, the replies that wait (of one Chat when asked), and a decision; every command it was sent is in `calls`. */
function serve(fake: FakeTeams, options: { replies: HeldReply[]; tasks?: () => unknown[]; failures?: string[]; outcome?: (taskId: string) => HeldReplyDecision; ofBot?: (sharedBotId: string) => boolean }) {
  const calls: MessageCommand[] = [], state = { replies: structuredClone(options.replies), failures: [...(options.failures ?? [])], tasks: options.tasks ?? (() => []), decided: [] as string[] };
  fake.messages = async <T>(command: MessageCommand) => {
    calls.push(structuredClone(command));
    if (command.action === 'listTasks') return { tasks: state.tasks() } as T;
    if (command.action === 'listSharedBots') return { bots: structuredClone(fake.bots) } as T;
    if (command.action === 'listHeldReplies') {
      const { conversationId, sharedBotId } = command.payload;
      return { replies: structuredClone(options.ofBot && !options.ofBot(sharedBotId) ? [] : state.replies.filter(reply => !conversationId || reply.conversationId === conversationId)) } as T;
    }
    if (command.action === 'approveReply' || command.action === 'rejectReply') {
      const failure = state.failures.shift();
      if (failure) throw new Error(failure);
      const { taskId } = command.payload;
      state.replies = state.replies.filter(reply => reply.taskId !== taskId); state.decided.push(`${command.action}:${taskId}`);
      return (options.outcome?.(taskId) ?? (command.action === 'approveReply' ? { taskId, state: 'completed', replyId: 'reply-0001', artifactId: null } : { taskId, state: 'declined', declineReason: 'approval_rejected' })) as T;
    }
    return {} as T;
  };
  return { calls, state, sent: (action: string) => calls.filter(command => command.action === action) as Array<MessageCommand & { payload: Record<string, unknown>; commandId: string; schemaVersion: number; teamId: string }> };
}

async function open(context: Context, bots: SharedBotCard[]) {
  withDom(context);
  const fake = new FakeTeams('alice'); fake.teams = [team('team-test', { role: 'owner' })]; fake.bots = bots;
  const workspace = new TeamsWorkspace(fake.api, 'alice', { storage: memoryStorage, environment: new EventTarget() }); context.after(() => workspace.close());
  await until(() => workspace.getSnapshot().botsLoaded && !!workspace.getSnapshot().detail, 'the Team');
  return { fake, workspace };
}
/** A Chat open on screen for Alice, with the replies and tasks given. A Bot DM by default. */
async function openChat(context: Context, options: { bots: SharedBotCard[]; replies: HeldReply[]; tasks?: () => unknown[]; failures?: string[]; kind?: TeamConversation['kind']; participants?: string[]; locale?: string; outcome?: (taskId: string) => HeldReplyDecision }) {
  const { fake, workspace } = await open(context, options.bots);
  const service = serve(fake, options);
  fake.conversations.set('team-test', [conversation(CHAT, { kind: options.kind ?? 'bot_dm', title: 'Planning', participants: options.participants ?? ['user:alice', `shared_bot:${BOT}`] })]);
  await workspace.refresh(); await workspace.refreshBots(); workspace.selectConversation(CHAT);
  await until(() => workspace.getSnapshot().selectedConversationId === CHAT && workspace.getSnapshot().botsLoaded && workspace.getSnapshot().bots.length === options.bots.length, 'the Chat');
  const page = await mount(workspace, snapshot => createElement(TeamConversationView, { workspace, snapshot, locale: options.locale ?? 'en' })); context.after(() => page.unmount());
  await until(() => page.find(node => node.type === 'header').length > 0, 'the header');
  return { fake, workspace, page, service };
}
const cards = (page: Page) => page.find(node => node.type === 'section' && classesOf(node).includes('tw-held'));
const buttons = (page: Page, text: string) => page.find(node => node.type === 'button' && textOf(node) === text);
const dialogs = (page: Page) => page.find(node => node.type === 'dialog');
const alerts = (page: Page) => page.find(node => node.props.role === 'alert');
const inDialog = (dialog: { findAll(match: (node: any) => boolean): any[] }, text: string) => dialog.findAll(node => node.type === 'button' && textOf(node) === text);

scenario('A reply that waits is shown to the Bot\'s maintainer where the Bot\'s task is: the words, the document, the time left and the three decisions; everyone else sees only that it waits, and nothing is read for them', async context => {
  const reply = held({ document: { title: 'Dinner plan', body: '1. Book a table' } });
  const maintainer = await openChat(context, { bots: [mine()], replies: [reply], tasks: () => [waitingTask()] });
  await until(() => cards(maintainer.page).length === 1, 'the reply');
  const text = textOf(cards(maintainer.page)[0]!);
  assert.match(text, /Waiting for your approval/); assert.match(text, /Dropped in 9 min if not approved/); assert.match(text, /Dinner at 7 works for me\./); assert.match(text, /Includes the document “Dinner plan”/);
  assert.doesNotMatch(text, /Book a table/, 'the title of the document, not its body');
  assert.deepEqual(['Approve', 'Edit', 'Reject'].map(label => buttons(maintainer.page, label).length), [1, 1, 1]);
  assert.doesNotMatch(maintainer.page.text(), /The Bot’s maintainer reads the reply before it is posted/, 'the line everyone else sees is the card\'s place here');
  await new Promise(resolve => setTimeout(resolve, 100));
  const reads = maintainer.service.sent('listHeldReplies'); assert.ok(reads.length >= 1 && reads.length <= 2, 'read when the Chat opened, and again when its task was known to wait for approval');
  assert.ok(reads.every(read => JSON.stringify(read.payload) === JSON.stringify({ sharedBotId: BOT, conversationId: CHAT })));
  await new Promise(resolve => setTimeout(resolve, 1300));
  assert.equal(maintainer.service.sent('listHeldReplies').length, reads.length, 'and no more while nothing changes: not on a timer, though the tasks of the Chat are read every second');

  const other = await openChat(context, { bots: [theirs()], replies: [reply], tasks: () => [waitingTask()] });
  await until(() => /Waiting for approval/.test(other.page.text()), 'the line');
  assert.match(other.page.text(), /The Bot’s maintainer reads the reply before it is posted/);
  assert.equal(cards(other.page).length, 0); assert.doesNotMatch(other.page.text(), /Dinner at 7 works for me|Dinner plan/); assert.equal(buttons(other.page, 'Approve').length, 0);
  assert.equal(other.service.sent('listHeldReplies').length, 0, 'nothing is read for a person who maintains no Bot of the Chat');
});

scenario('A reply nobody asked for has no task in the Chat: it is a row of its own at the end, with the Bot\'s name, and goes when the service no longer lists it', async context => {
  const reply = held({ taskId: 'task-unasked-0001', unasked: true, text: 'Maybe we should book earlier?' });
  const { fake, page, service } = await openChat(context, { bots: [mine({ name: 'Plan Bot' })], replies: [reply], kind: 'group', participants: ['user:alice', 'user:bob', `shared_bot:${BOT}`] });
  await until(() => cards(page).length === 1, 'the reply');
  const row = page.find(node => node.type === 'li' && node.props['data-held'] === 'true');
  assert.equal(row.length, 1); assert.match(textOf(row[0]!), /Plan Bot/); assert.match(textOf(row[0]!), /Maybe we should book earlier\?/); assert.match(textOf(row[0]!), /Auto-reply/, 'it says nobody asked');
  assert.equal(service.sent('listTasks').length > 0, true);
  // A newer message of the Chat drops it; the service says so with a signal and the next read does not have it.
  service.state.replies = []; fake.signal({ conversationId: CHAT });
  await until(() => cards(page).length === 0, 'the reply to go');
  assert.equal(page.find(node => node.type === 'li' && node.props['data-held'] === 'true').length, 0);
  assert.equal(alerts(page).length, 0, 'nothing is said about it: it was dropped, not decided');
  // A Bot's change comes the same way.
  service.state.replies = [held({ taskId: 'task-unasked-0002', unasked: true, text: 'Another thought.' })]; fake.signal({ bots: true });
  await until(() => cards(page).length === 1 && /Another thought\./.test(page.text()), 'the next reply');
  const reads = service.sent('listHeldReplies').length;
  fake.signal({ conversationId: 'another-chat' }); await new Promise(resolve => setTimeout(resolve, 30));
  assert.equal(service.sent('listHeldReplies').length, reads, 'a message of another Chat says nothing about this one');
});

scenario('Approve posts the reply as the Bot made it; the tasks are fetched again so the card moves on', async context => {
  let state = 'awaiting_approval';
  const { page, service } = await openChat(context, { bots: [mine()], replies: [held()], tasks: () => [waitingTask({ state })] });
  await until(() => cards(page).length === 1, 'the reply');
  const before = service.sent('listTasks').length;
  state = 'completed';
  await page.click(buttons(page, 'Approve')[0]!);
  await until(() => cards(page).length === 0, 'the card to move on');
  const approvals = service.sent('approveReply'); assert.equal(approvals.length, 1);
  assert.deepEqual(approvals[0]!.payload, { taskId: TASK }, 'as it is: no text'); assert.equal(approvals[0]!.schemaVersion, 1); assert.equal(approvals[0]!.teamId, 'team-test');
  assert.ok(service.sent('listTasks').length > before, 'the tasks were fetched again');
  await until(() => /Completed/.test(page.text()), 'the task\'s new state');
  assert.equal(alerts(page).length, 0);
});

scenario('Edit opens the text in an editor; Send posts the trimmed text, Cancel goes back, text that is the Bot\'s own is sent as it is, and nothing the service would refuse can be sent', async context => {
  const { page, service } = await openChat(context, { bots: [mine()], replies: [held({ text: 'Dinner at 7 works for me.' })], tasks: () => [waitingTask()] });
  await until(() => cards(page).length === 1, 'the reply');
  await page.click(buttons(page, 'Edit')[0]!);
  const editor = page.oneByLabel('Reply text');
  assert.equal(editor.type, 'textarea'); assert.equal(editor.props.value, 'Dinner at 7 works for me.', 'prefilled with the Bot\'s text');
  assert.deepEqual(['Send', 'Cancel'].map(label => buttons(page, label).length), [1, 1]); assert.equal(buttons(page, 'Approve').length, 0); assert.equal(buttons(page, 'Reject').length, 0);
  // Cancel returns to the reply as it was, with the decisions.
  await page.type(editor, 'Something else entirely');
  await page.click(buttons(page, 'Cancel')[0]!);
  assert.equal(page.byLabel('Reply text').length, 0); assert.match(textOf(cards(page)[0]!), /Dinner at 7 works for me\./); assert.equal(service.sent('approveReply').length, 0);
  // Nothing to send: empty, and too long, say why and cannot be sent.
  await page.click(buttons(page, 'Edit')[0]!);
  await page.type(page.oneByLabel('Reply text'), '   ');
  assert.equal(buttons(page, 'Send')[0]!.props.disabled, true); assert.match(page.text(), /The reply can’t be empty\./);
  await page.type(page.oneByLabel('Reply text'), 'x'.repeat(8193));
  assert.equal(buttons(page, 'Send')[0]!.props.disabled, true); assert.match(page.text(), /This reply is too long to send\. Shorten it\./);
  await page.type(page.oneByLabel('Reply text'), '€'.repeat(2730));
  assert.equal(buttons(page, 'Send')[0]!.props.disabled, false); assert.doesNotMatch(page.text(), /too long|can’t be empty/);
  // The edited text goes as it was written, trimmed.
  await page.type(page.oneByLabel('Reply text'), '  Dinner at 8 works better.  ');
  await page.click(buttons(page, 'Send')[0]!);
  await until(() => service.sent('approveReply').length === 1, 'the decision');
  assert.deepEqual(service.sent('approveReply')[0]!.payload, { taskId: TASK, text: 'Dinner at 8 works better.' });
  await until(() => cards(page).length === 0, 'the card to move on');
});

scenario('Send with the Bot\'s own words changed back is an approval as it is', async context => {
  const { page, service } = await openChat(context, { bots: [mine()], replies: [held({ text: 'Dinner at 7 works for me.\n' })], tasks: () => [waitingTask()] });
  await until(() => cards(page).length === 1, 'the reply');
  await page.click(buttons(page, 'Edit')[0]!);
  await page.type(page.oneByLabel('Reply text'), 'changed'); await page.type(page.oneByLabel('Reply text'), '  Dinner at 7 works for me.  ');
  await page.click(buttons(page, 'Send')[0]!);
  await until(() => service.sent('approveReply').length === 1, 'the decision');
  assert.deepEqual(service.sent('approveReply')[0]!.payload, { taskId: TASK });
});

scenario('Reject asks first in the app, not in the browser; Cancel there sends nothing, Reject drops the reply', async context => {
  const { page, service } = await openChat(context, { bots: [mine()], replies: [held()], tasks: () => [waitingTask()] });
  await until(() => cards(page).length === 1, 'the reply');
  assert.equal(dialogs(page).length, 0);
  await page.click(buttons(page, 'Reject')[0]!);
  assert.equal(dialogs(page).length, 1, 'a dialog of the app'); const dialog = dialogs(page)[0]!;
  assert.equal(dialog.props.role, 'alertdialog'); assert.match(textOf(dialog), /Reject this reply\?/); assert.match(textOf(dialog), /Nothing is posted\. The Bot’s reply is dropped and can’t be brought back\./);
  assert.equal(service.sent('rejectReply').length, 0, 'asking sends nothing');
  await page.click(inDialog(dialog, 'Cancel')[0]!);
  assert.equal(dialogs(page).length, 0); assert.equal(service.sent('rejectReply').length, 0); assert.equal(cards(page).length, 1);
  await page.click(buttons(page, 'Reject')[0]!);
  await page.click(inDialog(dialogs(page)[0]!, 'Reject')[0]!);
  await until(() => cards(page).length === 0, 'the card to go');
  assert.deepEqual(service.sent('rejectReply').map(command => command.payload), [{ taskId: TASK }]); assert.equal(service.sent('approveReply').length, 0); assert.equal(dialogs(page).length, 0);
});

scenario('A decision that failed can be tried again as the same command, and the card says why meanwhile; a reply that is no longer there says what became of it, and goes when it is closed', async context => {
  const { page, service } = await openChat(context, { bots: [mine()], replies: [held()], tasks: () => [waitingTask()], failures: ['SERVICE_UNAVAILABLE', 'REPLY_SUPERSEDED'] });
  await until(() => cards(page).length === 1, 'the reply');
  await page.click(buttons(page, 'Approve')[0]!);
  await until(() => alerts(page).length === 1, 'the failure');
  assert.equal(textOf(alerts(page)[0]!), messageErrorText('SERVICE_UNAVAILABLE', 'en')); assert.equal(buttons(page, 'Approve')[0]!.props.disabled, false, 'it can be tried again'); assert.equal(cards(page).length, 1);
  await page.click(buttons(page, 'Approve')[0]!);
  await until(() => /Someone wrote in the chat again, so the reply was dropped\./.test(page.text()), 'what became of it');
  const attempts = service.sent('approveReply'); assert.equal(attempts.length, 2); assert.equal(attempts[0]!.commandId, attempts[1]!.commandId, 'a retry of the same decision is the same command');
  assert.equal(buttons(page, 'Approve').length, 0); assert.equal(buttons(page, 'Edit').length, 0); assert.equal(buttons(page, 'Reject').length, 0); assert.doesNotMatch(page.text(), /Dinner at 7 works for me/, 'a reply that is gone is not shown');
  assert.equal(alerts(page).length, 0);
  assert.equal(service.sent('approveReply').length, 2, 'and it is not decided again');
  await page.click(buttons(page, 'Close')[0]!);
  assert.equal(cards(page).length, 0);
});

scenario('Every way the service refuses a decision has words, in the language of the person', async context => {
  const refusals: Array<[string, RegExp]> = [
    ['REPLY_EXPIRED', /The time to decide ran out, so the reply was dropped\./], ['REPLY_SUPERSEDED', /Someone wrote in the chat again, so the reply was dropped\./],
    ['REPLY_APPROVED', /This reply was already decided\./], ['REPLY_REJECTED', /This reply was already decided\./], ['REPLY_NOT_HELD', /This reply was already decided\./],
    ['TASK_REQUIRED', /This reply is no longer available\./], ['MAINTAINER_REQUIRED', /Only the Bot’s maintainer can decide on its replies\./],
  ];
  for (const [code, words] of refusals) {
    const { page } = await openChat(context, { bots: [mine()], replies: [held()], tasks: () => [waitingTask()], failures: [code] });
    await until(() => cards(page).length === 1, 'the reply');
    await page.click(buttons(page, 'Approve')[0]!);
    await until(() => words.test(page.text()), code);
    assert.equal(buttons(page, 'Approve').length, 0, `${code}: nothing more to decide`);
  }
  const refused = new Set<string>(), generic = messageErrorText('SOMETHING_ELSE', 'en');
  for (const locale of LOCALES) {
    for (const [code] of refusals) { const words = heldReplyErrorText(code, locale); refused.add(`${locale} ${words}`); assert.ok(words && words !== generic || locale !== 'en', `${locale} ${code}`); assert.notEqual(words, 'teams_error_reply_decided'); }
    assert.equal(heldReplyErrorText('SERVICE_UNAVAILABLE', locale), messageErrorText('SERVICE_UNAVAILABLE', locale), 'the rest is what every message command says');
  }
  assert.equal(refused.size, 5 * 5, 'five different words in each language (the three decided codes share one)');
  assert.equal(heldReplyErrorText('MAINTAINER_REQUIRED', 'en'), 'Only the Bot’s maintainer can decide on its replies.');
  assert.notEqual(heldReplyErrorText('MAINTAINER_REQUIRED', 'en'), messageErrorText('MAINTAINER_REQUIRED', 'en'), 'the words of a reply, not those of an operation');
});

scenario('An approval the service took that posted nothing says so on the card', async context => {
  const { page, service } = await openChat(context, { bots: [mine()], replies: [held()], tasks: () => [waitingTask()], outcome: taskId => ({ taskId, state: 'declined', declineReason: 'revoked', replyId: null, artifactId: null }) });
  await until(() => cards(page).length === 1, 'the reply');
  await page.click(buttons(page, 'Approve')[0]!);
  await until(() => /Not posted\. The request or its access changed, so the reply was dropped\./.test(page.text()), 'the words');
  assert.equal(service.sent('approveReply').length, 1); assert.equal(buttons(page, 'Approve').length, 0);
});

scenario('When the time runs out the card says the reply was dropped and offers no decision', async context => {
  const { page, service } = await openChat(context, { bots: [mine()], replies: [held({ taskId: 'task-unasked-0001', unasked: true, expiresAt: Date.now() + 1100 })], kind: 'group', participants: ['user:alice', 'user:bob', `shared_bot:${BOT}`] });
  await until(() => cards(page).length === 1 && buttons(page, 'Approve').length === 1, 'the reply');
  service.state.replies = [];
  await until(() => /The time to decide ran out, so the reply was dropped\./.test(page.text()), 'the end', );
  for (const label of ['Approve', 'Edit', 'Reject']) assert.equal(buttons(page, label).length, 0, label);
  assert.doesNotMatch(page.text(), /Dinner at 7 works for me/);
  assert.ok(service.sent('listHeldReplies').length >= 2, 'the list was read again when the time was up');
  await page.click(buttons(page, 'Close')[0]!);
  assert.equal(cards(page).length, 0);
});

scenario('A reply whose time is already up is shown as dropped, without its decisions, even before the list is read again', async context => {
  const { page } = await openChat(context, { bots: [mine()], replies: [held({ expiresAt: Date.now() - 1000 })], tasks: () => [waitingTask()] });
  await until(() => cards(page).length === 1, 'the card');
  assert.match(textOf(cards(page)[0]!), /The time to decide ran out, so the reply was dropped\./);
  for (const label of ['Approve', 'Edit', 'Reject']) assert.equal(buttons(page, label).length, 0, label);
});

scenario('A list that cannot be read says so and does not hide the line everyone sees; one the service says is not theirs is quiet', async context => {
  const failing = await openChat(context, { bots: [mine()], replies: [], tasks: () => [waitingTask()] });
  const inner = failing.fake.messages;
  let attempts = 0;
  failing.fake.messages = async <T>(command: MessageCommand) => { if (command.action === 'listHeldReplies') { attempts += 1; throw new Error('SERVICE_UNAVAILABLE'); } return inner<T>(command); };
  failing.fake.signal({ bots: true });
  await until(() => alerts(failing.page).length === 1, 'the failure');
  assert.equal(textOf(alerts(failing.page)[0]!), `${messageErrorText('SERVICE_UNAVAILABLE', 'en')}Retry`); assert.match(failing.page.text(), /Waiting for approval/);
  // Retry asks again (the notice stays while the service still fails), and a read that works takes the notice away.
  const asked = attempts;
  await failing.page.click(buttons(failing.page, 'Retry')[0]!);
  await until(() => attempts > asked && buttons(failing.page, 'Retry').length === 1, 'the read that Retry asks for, with the notice still there');
  failing.fake.messages = inner;
  await failing.page.click(buttons(failing.page, 'Retry')[0]!);
  await until(() => alerts(failing.page).length === 0, 'the notice gone once the list can be read');
  const denied = await openChat(context, { bots: [mine()], replies: [], tasks: () => [waitingTask()], failures: [] });
  const keep = denied.fake.messages;
  denied.fake.messages = async <T>(command: MessageCommand) => { if (command.action === 'listHeldReplies') throw new Error('MAINTAINER_REQUIRED'); return keep<T>(command); };
  denied.fake.signal({ bots: true }); await new Promise(resolve => setTimeout(resolve, 50));
  assert.equal(alerts(denied.page).length, 0, 'there is nothing for this person to retry'); assert.match(denied.page.text(), /Waiting for approval/);
});

scenario('Nothing is read while there is no connection, and the list is read when it returns', async context => {
  const { fake, workspace } = await open(context, [mine()]);
  const service = serve(fake, { replies: [held()], tasks: () => [waitingTask()] });
  fake.conversations.set('team-test', [conversation(CHAT, { kind: 'bot_dm', participants: ['user:alice', `shared_bot:${BOT}`] })]);
  await workspace.refresh(); await workspace.refreshBots(); workspace.selectConversation(CHAT);
  await until(() => workspace.getSnapshot().selectedConversationId === CHAT && workspace.getSnapshot().botsLoaded, 'the Chat');
  let offline = true;
  const page = await mount(workspace, snapshot => createElement(TeamConversationView, { workspace, snapshot: { ...snapshot, offline }, locale: 'en' })); context.after(() => page.unmount());
  await new Promise(resolve => setTimeout(resolve, 80));
  assert.equal(service.sent('listHeldReplies').length, 0); assert.equal(cards(page).length, 0);
  await page.during(async () => { offline = false; await workspace.refresh(); });
  await until(() => cards(page).length === 1, 'the reply, once the connection is back');
  assert.equal(service.sent('listHeldReplies').length, 1);
});

scenario('A task that starts to wait for approval makes the Chat read the list again', async context => {
  let state = 'running';
  const { page, service } = await openChat(context, { bots: [mine()], replies: [], tasks: () => [waitingTask({ state })] });
  await until(() => service.sent('listHeldReplies').length === 1, 'the first read');
  assert.equal(cards(page).length, 0);
  service.state.replies = [held()]; state = 'awaiting_approval';
  await until(() => cards(page).length === 1, 'the reply, from the task that now waits');
  assert.equal(service.sent('listHeldReplies').length, 2);
});

scenario('The card renders the Bot\'s words through the host\'s Markdown, as its messages are', async context => {
  withDom(context);
  const { fake, workspace } = await open(context, [mine()]);
  serve(fake, { replies: [held({ text: '**Dinner** at 7' })], tasks: () => [waitingTask()] });
  fake.conversations.set('team-test', [conversation(CHAT, { kind: 'bot_dm', participants: ['user:alice', `shared_bot:${BOT}`] })]);
  await workspace.refresh(); await workspace.refreshBots(); workspace.selectConversation(CHAT);
  await until(() => workspace.getSnapshot().selectedConversationId === CHAT && workspace.getSnapshot().botsLoaded, 'the Chat');
  const rendered: string[] = [];
  const page = await mount(workspace, snapshot => createElement(TeamConversationView, { workspace, snapshot, locale: 'en', renderMarkdown: (text: string) => { rendered.push(text); return createElement('div', { className: 'host-md' }, `rendered:${text}`); } })); context.after(() => page.unmount());
  await until(() => cards(page).length === 1, 'the reply');
  assert.match(page.text(), /rendered:\*\*Dinner\*\* at 7/); assert.ok(rendered.includes('**Dinner** at 7'));
});

// ───────────────────────────── the Bot's card ─────────────────────────────

const directory = (workspace: TeamsWorkspace, onOpenChat?: (id: string) => void) => (s: ReturnType<TeamsWorkspace['getSnapshot']>) => createElement(BotDirectory, {
  api: workspace.api, uid: s.uid, team: s.team!, members: s.directory, bots: s.bots, locale: 'en', busy: s.messageBusy, status: '', attempt: workspace.attempt,
  onOpen: bot => workspace.startBotDm(bot), onChanged: workspace.refreshBots, ...(onOpenChat ? { onOpenChat } : {}),
});
const entry = (page: Page) => page.find(node => node.type === 'button' && classesOf(node).includes('tw-held-entry'));

scenario('The Bot\'s card of its maintainer offers "Replies waiting" with a count while replies wait across Chats; nobody else is offered it, and nothing is read for them', async context => {
  const { fake, workspace } = await open(context, [mine(), mine({ sharedBotId: 'quiet-bot-0001', name: 'Quiet Bot' }), theirs({ sharedBotId: 'their-bot-0001', name: 'Their Bot' })]);
  const service = serve(fake, { ofBot: id => id === BOT, replies: [held(), held({ taskId: 'task-held-0002', conversationId: 'chat-2', conversationTitle: 'Trip', heldAt: Date.now() }), held({ taskId: 'task-held-0003', conversationId: 'chat-2', conversationTitle: 'Trip', heldAt: Date.now() - 5000 })] });
  await workspace.refreshBots(); await until(() => workspace.getSnapshot().bots.length === 3, 'the Bots');
  const page = await mount(workspace, directory(workspace)); context.after(() => page.unmount());
  await until(() => entry(page).length === 1, 'the entry');
  assert.equal(textOf(entry(page)[0]!), 'Replies waiting 3', 'the words and the number of replies, in every Chat');
  const card = page.find(node => node.type === 'li' && classesOf(node).includes('teams-card') && node.findAll((inner: any) => inner.type === 'button' && classesOf(inner).includes('tw-held-entry')).length === 1);
  assert.equal(card.length, 1); assert.match(textOf(card[0]!), /Fixture Bot/, 'on the card of the Bot they are the replies of');
  const reads = service.sent('listHeldReplies').map(command => command.payload);
  assert.deepEqual(reads.map(payload => payload.sharedBotId).sort(), [BOT, 'quiet-bot-0001'], 'read for each Bot the person maintains and for no other'); assert.ok(reads.every(payload => payload.conversationId === undefined), 'across Chats');
  assert.equal(service.sent('listHeldReplies').length, 2);
});

scenario('The list opens from the card: replies grouped by Chat, each with the same decisions, and a way to the Chat', async context => {
  const { fake, workspace } = await open(context, [mine()]);
  const service = serve(fake, { replies: [
    held({ taskId: 'task-held-aaa1', text: 'First in Planning', heldAt: Date.now() - 4000 }), held({ taskId: 'task-held-bbb1', conversationId: 'chat-2', conversationTitle: 'Trip', text: 'Newest, in Trip', heldAt: Date.now(), unasked: true }),
    held({ taskId: 'task-held-ccc1', conversationId: 'chat-3', conversationTitle: undefined, text: 'In a Chat with no title', heldAt: Date.now() - 8000 }), held({ taskId: 'task-held-ddd1', text: 'Second in Planning', heldAt: Date.now() - 2000, document: { title: 'Plan', body: 'x' } })] });
  const opened: string[] = [];
  await workspace.refreshBots();
  const page = await mount(workspace, directory(workspace, id => opened.push(id))); context.after(() => page.unmount());
  await until(() => entry(page).length === 1, 'the entry');
  assert.equal(dialogs(page).length, 0); await page.click(entry(page)[0]!);
  assert.equal(dialogs(page).length, 1); const text = textOf(dialogs(page)[0]!);
  assert.match(text, /Replies waiting/); assert.match(text, /Replies the Bot made that wait for your approval before they are posted/); assert.match(text, /Fixture Bot/);
  const groups = page.find(node => node.type === 'section' && classesOf(node).includes('tw-held-group'));
  assert.deepEqual(groups.map(group => textOf(group.findAll((node: any) => node.type === 'h3')[0])), ['Trip', 'Planning', 'Chat'], 'by Chat, the Chat of the newest reply first; a Chat without a title is still named');
  assert.match(textOf(groups[1]!), /Second in Planning[\s\S]*First in Planning/, 'newest first within a Chat'); assert.match(textOf(groups[1]!), /Includes the document “Plan”/);
  assert.match(textOf(groups[0]!), /Auto-reply/);
  assert.equal(cards(page).length, 4); assert.equal(buttons(page, 'Approve').length, 4); assert.equal(buttons(page, 'Reject').length, 4);
  assert.doesNotMatch(text, /Waiting for your approval/, 'a card of the list is already under its Chat: it leaves out the line that says what it is');
  assert.match(text, /Dropped in 9 min if not approved/);
  assert.equal(entry(page).length, 1);
  // Approve one from the list: the same command, and the entry counts one fewer.
  await page.click(buttons(page, 'Approve')[0]!);
  await until(() => service.sent('approveReply').length === 1, 'the decision');
  assert.deepEqual(service.sent('approveReply')[0]!.payload, { taskId: 'task-held-bbb1' }); await until(() => cards(page).length === 3, 'the card to go');
  assert.equal(textOf(entry(page)[0]!), 'Replies waiting 3');
  // Reject another through the dialog of the app, above the list.
  await page.click(buttons(page, 'Reject')[0]!);
  const asking = dialogs(page).find(dialog => dialog.props.role === 'alertdialog')!; assert.ok(asking, 'the question is the app\'s own');
  await page.click(inDialog(asking, 'Reject')[0]!);
  await until(() => service.sent('rejectReply').length === 1, 'the rejection'); await until(() => cards(page).length === 2, 'the card to go');
  // Edit one in the list.
  await page.click(buttons(page, 'Edit')[0]!);
  assert.equal(page.oneByLabel('Reply text').props.value.length > 0, true); await page.type(page.oneByLabel('Reply text'), 'Changed');
  await page.click(buttons(page, 'Send')[0]!);
  await until(() => service.sent('approveReply').length === 2, 'the edit'); assert.equal(service.sent('approveReply')[1]!.payload.text, 'Changed');
  // The Chat is one button away.
  await until(() => cards(page).length === 1, 'the last');
  await page.click(buttons(page, 'Open chat')[0]!);
  assert.equal(opened.length, 1); assert.equal(dialogs(page).length, 0, 'the list gives way to the Chat');
  assert.match(opened[0]!, /^(chat-3|test-chat|chat-2)$/);
});

scenario('The list says when nothing waits, that it cannot be read, and refreshes; a Bot that is not theirs after all takes the entry away', async context => {
  const { fake, workspace } = await open(context, [mine()]);
  const service = serve(fake, { replies: [held()] });
  await workspace.refreshBots();
  const page = await mount(workspace, directory(workspace)); context.after(() => page.unmount());
  await until(() => entry(page).length === 1, 'the entry'); await page.click(entry(page)[0]!);
  service.state.replies = []; await page.click(buttons(page, 'Refresh')[0]!);
  await until(() => /No replies are waiting\./.test(page.text()), 'the empty list');
  assert.equal(entry(page).length, 1, 'while the list is open the entry stays, with nothing to count'); assert.equal(textOf(entry(page)[0]!), 'Replies waiting');
  const inner = fake.messages; fake.messages = async <T>(command: MessageCommand) => { if (command.action === 'listHeldReplies') throw new Error('SERVICE_UNAVAILABLE'); return inner<T>(command); };
  await page.click(buttons(page, 'Refresh')[0]!);
  await until(() => alerts(page).length === 1, 'the failure'); assert.equal(textOf(alerts(page)[0]!), messageErrorText('SERVICE_UNAVAILABLE', 'en')); assert.equal(buttons(page, 'Retry').length, 1);
  fake.messages = async <T>(command: MessageCommand) => { if (command.action === 'listHeldReplies') throw new Error('MAINTAINER_REQUIRED'); return inner<T>(command); };
  await page.click(buttons(page, 'Retry')[0]!);
  await until(() => entry(page).length === 0, 'the entry to go'); assert.equal(dialogs(page).length, 0, 'and the list with it');
});

scenario('A reply that is decided elsewhere leaves the list when the service says the Bot changed', async context => {
  const { fake, workspace } = await open(context, [mine()]);
  const service = serve(fake, { replies: [held(), held({ taskId: 'task-held-0002', heldAt: Date.now() })] });
  await workspace.refreshBots();
  const page = await mount(workspace, directory(workspace)); context.after(() => page.unmount());
  await until(() => entry(page).length === 1 && textOf(entry(page)[0]!) === 'Replies waiting 2', 'two replies');
  service.state.replies = service.state.replies.filter(reply => reply.taskId !== 'task-held-0002'); fake.signal({ bots: true });
  await until(() => textOf(entry(page)[0]!) === 'Replies waiting 1', 'one reply');
  service.state.replies = []; fake.signal({ bots: true });
  await until(() => entry(page).length === 0, 'none');
});

// ───────────────────────────── the words ─────────────────────────────

test('the words of this feature are in every language, each different from the others, with a Bot never called an assistant and the Traditional Chinese of Taiwan', () => {
  const keys = ['teams_held_title', 'teams_held_expires_fmt', 'teams_held_document_fmt', 'teams_held_reject', 'teams_held_reject_confirm', 'teams_held_reject_detail', 'teams_held_edit_label', 'teams_held_empty_text',
    'teams_held_too_long', 'teams_held_unasked', 'teams_held_waiting', 'teams_held_intro', 'teams_held_none', 'teams_held_revoked', 'teams_error_reply_decided', 'teams_error_reply_expired', 'teams_error_reply_superseded',
    'teams_error_reply_gone', 'teams_error_held_maintainer'] as const;
  for (const key of keys) {
    const values = LOCALES.map(locale => teamsText(locale)(key, 'x'));
    assert.equal(new Set(values).size, 5, key); assert.ok(!values.some(value => value === key || value === ''), key);
    assert.doesNotMatch(values.join(' '), /assistant|助手|アシスタント|어시스턴트/iu, key);
    assert.doesNotMatch(teamsText('zh-Hant')(key, 'x'), /[这个为对们会说开关现点样经动种无与发过还进见网设户务页题请谢议记订许证评识词调负费资购货质转软较载输边达运远选递邮钟钱铁银链锁错长门问间队阅阳阶险际随难项须顺预领风飞馆验]/u, `${key}: Simplified characters in Traditional Chinese`);
  }
  const en = teamsText('en');
  assert.equal(en('teams_held_expires_fmt', 'in 4 min'), 'Dropped in 4 min if not approved'); assert.equal(teamsText('zh-Hant')('teams_held_expires_fmt', '4 分鐘後'), '若未核准，4 分鐘後捨棄');
  assert.equal(teamsText('ja')('teams_held_expires_fmt', '4分後'), '4分後に承認されていない場合は破棄されます'); assert.equal(teamsText('ko')('teams_held_expires_fmt', '4분 후'), '승인하지 않으면 4분 후 폐기됩니다');
  assert.equal(en('teams_held_document_fmt', 'Plan'), 'Includes the document “Plan”'); assert.equal(teamsText('zh-Hans')('teams_held_waiting'), '待批准的回复'); assert.equal(teamsText('zh-Hant')('teams_held_waiting'), '待核准的回覆');
  assert.equal(teamsText('ja')('teams_held_waiting'), '承認待ちの返信'); assert.equal(teamsText('ko')('teams_held_waiting'), '승인 대기 중인 답장');
  // The words of a decision a person is already used to are the ones they read here.
  assert.equal(teamsText('zh-Hant')('teams_approval_approve'), '核准'); assert.equal(en('teams_approval_approve'), 'Approve');
  assert.match(teamsText('ja')('teams_error_held_maintainer'), /メンテナー/); assert.match(teamsText('ko')('teams_error_held_maintainer'), /메인테이너/);
});
