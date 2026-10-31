/**
 * A Bot through Remote Control: a viewer that declared the `bots` feature lists
 * the Bots whose switch is on, their chats, and starts a new one; any other
 * viewer sees exactly what it saw before Bots could be reached. These tests use
 * the real `BotService`, policy and gateway, over fake runtimes.
 */

import assert from 'node:assert/strict';
import { mkdirSync, utimesSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';

import { RemoteRpcError, type RemoteBotSummary } from '@lingxi/bridge-client/remote';

import { BotService, type BotCatalog, type BotRuntimes } from '../src/main/bots/service.js';
import type { RemoteViewer } from '../src/main/remoteControl/gateway.js';
import { PushRequester, RemotePush } from '../src/main/remoteControl/pushRequests.js';
import { IDEMPOTENCY_WINDOW_MS } from '../src/main/remoteControl/remoteCommands.js';
import { MAX_BOT_THREADS } from '../src/main/remoteControl/remoteBots.js';
import { LIVE_BATCH_MS } from '../src/main/remoteControl/sessionAttachment.js';
import { LIST_DEBOUNCE_MS } from '../src/main/remoteControl/sessionGateway.js';
import type { ProjectSessionCatalogRow } from '../src/main/session-catalog.js';
import { botFixture } from './bot-helpers.js';
import {
  FakeRuntime,
  LEASE_GRACE_MS,
  RecordingChannel,
  SESSION_A,
  SESSION_B,
  SESSION_C,
  flush,
  gatewayFixture,
  permissionRequest,
  questionRequest,
  rpcCode,
  viewer,
} from './remote-session-helpers.js';

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

/** A phone, with the channel its notifications arrive on. */
interface Who { viewer: RemoteViewer; channel: RecordingChannel }

function newViewer(name: string, botAware: boolean): Who {
  return {
    viewer: viewer({ device_id: `device-${name}`, conn_id: `conn-${name}`, ...(botAware ? { bot_aware: true as const } : {}) }),
    channel: new RecordingChannel(),
  };
}

async function rpcError(promise: Promise<unknown>): Promise<RemoteRpcError> {
  try {
    await promise;
  } catch (error) {
    if (error instanceof RemoteRpcError) return error;
    throw error;
  }
  throw new Error('expected the call to fail');
}

/** Wait for what needs the disk, which `flush()` does not wait for. */
async function until(condition: () => boolean, what: string): Promise<void> {
  for (let attempt = 0; attempt < 500; attempt += 1) {
    if (condition()) return;
    await new Promise<void>((resolve) => setTimeout(resolve, 2));
  }
  assert.fail(`timed out waiting for ${what}`);
}

/** Long enough for a read of the disk that was started to finish, where nothing is expected to come of it. */
const quiet = (): Promise<void> => new Promise<void>((resolve) => setTimeout(resolve, 30));

function row(uuid: string, modified: string, extra: Partial<ProjectSessionCatalogRow> = {}): ProjectSessionCatalogRow {
  return { uuid, title: `Chat ${uuid.slice(-4)}`, modified_rfc3339: modified, message_count: 2, mode: 'code', path: `/transcripts/${uuid}.jsonl`, empty_session: false, ...extra };
}

/** Bots, the policy and the gateway over fake runtimes, wired the way `index.ts` wires them. */
function world(t: { after(fn: () => void): void }, options: { ordinary?: boolean } = {}) {
  const f = botFixture(t);
  const flags = { computer: true, veto: false, failNew: undefined as Error | undefined };
  const hooks: { list?: () => void } = {};
  const rows = new Map<string, ProjectSessionCatalogRow[]>();
  const controllers = new Set<string>();
  const created: string[] = [];
  let holdClose: Promise<void> | undefined;
  let sequence = 0;
  let service!: BotService;
  const fx = gatewayFixture({
    bots: () => service,
    policy: {
      computerEnabled: () => flags.computer,
      managedDirectory: '/managed',
      readFile: () => (flags.veto ? '{"disableRemoteControl": true}' : '{}'),
    },
  });
  const runtimes: BotRuntimes = {
    newSession: async (projectPath) => {
      if (flags.failNew) throw flags.failNew;
      const sessionId = `aaaaaaaa-bbbb-4ccc-8ddd-${String(++sequence).padStart(12, '0')}`;
      created.push(sessionId);
      fx.runtimes.add(new FakeRuntime(sessionId, projectPath, `rt-new-${sequence}`)).resume();
      return { projectPath, sessionId };
    },
    openSession: async () => ({}),
    // Stopping a folder's sessions disposes their runtimes, which the viewers attached to them are told, and forgets them.
    closeProject: async (projectPath) => {
      await holdClose;
      for (const runtime of [...fx.runtimes.runtimes.values()]) {
        if (runtime.projectPath !== projectPath) continue;
        runtime.dispose();
        fx.runtimes.runtimes.delete(runtime.sessionId);
      }
    },
  };
  service = new BotService({
    store: f.store,
    platform: 'darwin',
    runtimes,
    openProject: (sessionId) => fx.runtimes.get(sessionId)?.projectPath,
    changed: () => fx.policy.notifySessionsChanged(),
    catalogFor: (configHome): BotCatalog => ({
      list: async () => {
        hooks.list?.();
        return { sessions: rows.get(configHome) ?? [] };
      },
      find: async (_project, sessionId) => (rows.get(configHome) ?? []).find((candidate) => candidate.uuid === sessionId),
      workspaceSettings: async () => ({ environment: {}, configHome }),
      invalidate: () => undefined,
    }),
    routines: {
      isControllerSession: (workspace, sessionId) => controllers.has(`${workspace}:${sessionId}`),
      releaseScope: () => undefined,
    },
  });
  if (options.ordinary) fx.enable(SESSION_A);
  const revisions = new Map<string, number>();
  const aware = newViewer('aware', true);
  const unaware = newViewer('unaware', false);
  const world = {
    ...f, fx, service, flags, hooks, created, aware, unaware,
    /** Make a Bot; `remote` turns its switch on. */
    bot(name: string, bot: { remote?: boolean; description?: string } = {}): string {
      const record = service.create({ name, ...(bot.description !== undefined ? { description: bot.description } : {}) });
      revisions.set(record.bot_id, record.revision);
      if (bot.remote) world.patch(record.bot_id, { remote: true });
      return record.bot_id;
    },
    patch(botId: string, patch: Parameters<BotService['update']>[2]): void {
      revisions.set(botId, service.update(botId, revisions.get(botId)!, patch).revision);
    },
    workspace: (botId: string): string => f.store.workspaceDir(botId),
    /** A chat of the Bot with a transcript on disk, last written `seconds` after the epoch. */
    transcript(botId: string, sessionId: string, seconds?: number): void {
      const directory = f.store.transcriptsDir(botId);
      mkdirSync(directory, { recursive: true });
      const path = join(directory, `${sessionId}.jsonl`);
      writeFileSync(path, '');
      if (seconds !== undefined) utimesSync(path, seconds, seconds);
    },
    /** A chat of the Bot that is open: its runtime works in the Bot's workspace. */
    open(botId: string, sessionId: string): FakeRuntime {
      const runtime = fx.runtimes.add(new FakeRuntime(sessionId, f.store.workspaceDir(botId), `rt-${sessionId.slice(-4)}`));
      runtime.resume();
      return runtime;
    },
    /** What the Bot's home lists as its chats. */
    catalog(botId: string, list: ProjectSessionCatalogRow[]): void { rows.set(f.store.homeDir(botId), list); },
    markController(botId: string, sessionId: string): void { controllers.add(`${f.store.workspaceDir(botId)}:${sessionId}`); },
    holdClosing(promise: Promise<void>): void { holdClose = promise; },
    call: (method: string, params: unknown, who: Who = aware) => fx.call(method, params, who),
    /** Let the debounce pass and wait for the next `bots.changed` the viewer is sent. */
    async nextBots(who: Who = aware): Promise<RemoteBotSummary[]> {
      const before = who.channel.of('bots.changed').length;
      fx.clock.advance(LIST_DEBOUNCE_MS);
      await until(() => who.channel.of('bots.changed').length > before, 'bots.changed');
      return who.channel.of('bots.changed').at(-1).bots;
    },
    /** Let the debounce pass and make sure no `bots.changed` follows. */
    async noBots(who: Who = aware): Promise<void> {
      const before = who.channel.of('bots.changed').length;
      fx.clock.advance(LIST_DEBOUNCE_MS);
      await quiet();
      assert.equal(who.channel.of('bots.changed').length, before, 'no bots.changed');
    },
  };
  return world;
}

type World = ReturnType<typeof world>;

const botIds = (bots: RemoteBotSummary[]): string[] => bots.map((bot) => bot.bot_id);

// ─────────────────────────────────────────────────────────────────────────────
// The Bots
// ─────────────────────────────────────────────────────────────────────────────

test('bots.list shows the Bots whose switch is on, oldest first, with what their chats say', async (t) => {
  const w = world(t);
  const scribe = w.bot('Scribe', { remote: true, description: 'Writes things down' });
  w.bot('Private');
  const reader = w.bot('Reader', { remote: true });
  assert.deepEqual(await w.call('bots.list', {}), {
    bots: [
      { bot_id: scribe, name: 'Scribe', description: 'Writes things down', status: 'idle', waiting_threads: 0 },
      { bot_id: reader, name: 'Reader', description: '', status: 'idle', waiting_threads: 0 },
    ],
  });

  w.transcript(scribe, SESSION_A, 1_700_000_000);
  w.transcript(scribe, SESSION_B, 1_700_000_500);
  const listed = (await w.call('bots.list', {})).bots as RemoteBotSummary[];
  assert.equal(listed[0]!.last_activity_at, 1_700_000_500_000, 'the newest transcript');
  assert.ok(!('last_activity_at' in listed[1]!), 'a Bot with no chat has no time');

  const first = w.open(scribe, SESSION_A);
  const second = w.open(scribe, SESSION_B);
  first.turnActive = true;
  assert.equal((await w.call('bots.list', {})).bots[0].status, 'running');
  second.askPermission(permissionRequest(1));
  const waiting = (await w.call('bots.list', {})).bots[0];
  assert.equal(waiting.status, 'requires_action', 'one waiting chat outweighs a running one');
  assert.equal(waiting.waiting_threads, 1);
  second.askQuestion(questionRequest(2));
  assert.equal((await w.call('bots.list', {})).bots[0].waiting_threads, 1, 'a chat counts once however much it asks');
  first.askPermission(permissionRequest(3));
  assert.equal((await w.call('bots.list', {})).bots[0].waiting_threads, 2);
  assert.equal((await w.call('bots.list', {})).bots[1].status, 'idle', 'the other Bot is not touched');
});

test('a chat that is not connected, and the hidden routine controller, do not count towards a Bot\'s status', async (t) => {
  const w = world(t);
  const bot = w.bot('Scribe', { remote: true });
  w.markController(bot, SESSION_C);
  const controller = w.open(bot, SESSION_C);
  controller.turnActive = true;
  controller.askPermission(permissionRequest(1));
  const asleep = w.open(bot, SESSION_B);
  asleep.turnActive = true;
  asleep.connectionState = { status: 'disconnected' };
  assert.deepEqual((await w.call('bots.list', {})).bots, [{ bot_id: bot, name: 'Scribe', description: '', status: 'idle', waiting_threads: 0 }]);
});

test('bots.watch answers with the list, then sends the full list when a status changes: once per burst, and only when it differs', async (t) => {
  const w = world(t);
  const bot = w.bot('Scribe', { remote: true });
  const runtime = w.open(bot, SESSION_A);
  const answer = await w.call('bots.watch', {});
  assert.equal(answer.bots[0].status, 'idle');
  w.aware.channel.clear();

  runtime.turnActive = true;
  runtime.push({ type: 'turn_started', turn_id: 1 } as any);
  runtime.push({ type: 'model_changed', model: 'm' } as any);
  w.fx.clock.advance(LIST_DEBOUNCE_MS - 1);
  await flush();
  assert.deepEqual(w.aware.channel.of('bots.changed'), [], 'debounced');
  w.fx.clock.advance(1);
  await until(() => w.aware.channel.of('bots.changed').length > 0, 'bots.changed');
  await quiet();
  assert.equal(w.aware.channel.of('bots.changed').length, 1, 'a burst is one notice');
  assert.equal(w.aware.channel.of('bots.changed')[0].bots[0].status, 'running');

  // A change the Bot's entry does not show is not sent.
  runtime.push({ type: 'model_changed', model: 'other' } as any);
  await w.noBots();

  runtime.askPermission(permissionRequest(1));
  const asking = (await w.nextBots())[0]!;
  assert.deepEqual([asking.status, asking.waiting_threads], ['requires_action', 1]);
  runtime.approvePermission(1);
  runtime.turnActive = false;
  runtime.push({ type: 'turn_ended', outcome: { type: 'completed' }, cost: {} } as any);
  assert.equal((await w.nextBots())[0]!.status, 'idle');
});

test('chats of ordinary sessions never move the Bots, and a viewer that did not ask to watch is sent nothing', async (t) => {
  const w = world(t, { ordinary: true });
  const bot = w.bot('Scribe', { remote: true });
  const ordinary = w.fx.runtimes.add(new FakeRuntime(SESSION_A));
  ordinary.resume();
  await w.call('bots.watch', {});
  const listing = newViewer('listing', true);
  await w.call('bots.list', {}, listing);
  w.aware.channel.clear();

  ordinary.turnActive = true;
  ordinary.push({ type: 'turn_started', turn_id: 1 } as any);
  await w.noBots();

  w.patch(bot, { name: 'Renamed' });
  assert.equal((await w.nextBots())[0]!.name, 'Renamed');
  assert.deepEqual(listing.channel.methods(), [], 'bots.list does not subscribe');

  w.fx.gateway.viewerClosed(w.aware.viewer);
  w.patch(bot, { name: 'Again' });
  await w.noBots();
});

test('a Bot made, renamed, switched on or off, or deleted reaches the watchers with the full list', async (t) => {
  const w = world(t);
  const first = w.bot('First', { remote: true });
  await w.call('bots.watch', {});
  w.aware.channel.clear();

  const second = w.bot('Second');
  await w.noBots(); // a new Bot is off, so there is nothing to show

  w.patch(second, { remote: true });
  assert.deepEqual(botIds(await w.nextBots()), [first, second]);

  w.patch(first, { name: 'Renamed', description: 'Now described' });
  const renamed = (await w.nextBots())[0]!;
  assert.deepEqual([renamed.name, renamed.description], ['Renamed', 'Now described']);

  w.patch(second, { remote: false });
  assert.deepEqual(botIds(await w.nextBots()), [first]);

  await w.service.delete(first);
  assert.deepEqual(await w.nextBots(), []);
});

test('to a viewer that is not Bot-aware the Bot methods are unknown methods, and they start nothing', async (t) => {
  const w = world(t);
  const bot = w.bot('Scribe', { remote: true });
  const calls: Array<[string, unknown]> = [
    ['bots.list', {}],
    ['bots.watch', {}],
    ['bot.threads.list', { bot_id: bot }],
    ['bot.thread.create', { bot_id: bot, client_request_id: 'r1' }],
  ];
  for (const [method, params] of calls) {
    const error = await rpcError(w.call(method, params, w.unaware));
    assert.deepEqual([error.code, error.message], ['invalid_params', `unknown method ${method}`], 'the words for any method that does not exist');
  }
  assert.equal((await rpcError(w.call('nonsense.method', {}, w.unaware))).message, 'unknown method nonsense.method');
  assert.deepEqual(w.created, [], 'no chat was started');
  w.patch(bot, { name: 'Renamed' });
  await w.noBots(w.unaware);
  assert.deepEqual(w.unaware.channel.methods(), [], 'it did not start watching');
});

// ─────────────────────────────────────────────────────────────────────────────
// A Bot's chats
// ─────────────────────────────────────────────────────────────────────────────

test('bot.threads.list gives the Bot\'s chats newest first, at most fifty, named for the Bot and marked with its id', async (t) => {
  const w = world(t);
  const bot = w.bot('Scribe', { remote: true });
  const all = Array.from({ length: 60 }, (_, index) => {
    const uuid = `bbbbbbbb-bbbb-4ccc-8ddd-${String(index).padStart(12, '0')}`;
    return row(uuid, new Date(Date.UTC(2026, 9, 1, 0, index)).toISOString());
  });
  w.catalog(bot, [...all].reverse().slice(0, 30).concat(all.slice(0, 30)));
  const { sessions } = await w.call('bot.threads.list', { bot_id: bot });
  assert.equal(sessions.length, MAX_BOT_THREADS);
  assert.deepEqual(sessions.map((session: any) => session.session_id), all.slice(10).reverse().map((entry) => entry.uuid), 'newest first, the oldest ten left out');
  for (const session of sessions) {
    assert.equal(session.bot_id, bot);
    assert.equal(session.project_label, 'Scribe');
    assert.equal(session.status, 'dormant');
    assert.equal(session.viewers, 0);
  }
  assert.equal(sessions[0].title, `Chat ${all[59]!.uuid.slice(-4)}`);
});

test('bot.threads.list leaves out the routine controller, shows an open chat\'s state, and calls an untitled one so', async (t) => {
  const w = world(t);
  const bot = w.bot('Scribe', { remote: true });
  const controller = 'cccccccc-bbbb-4ccc-8ddd-000000000001';
  w.markController(bot, controller);
  w.catalog(bot, [
    row(controller, '2026-10-26T09:00:00Z'),
    row(SESSION_A, '2026-10-26T08:00:00Z'),
    row(SESSION_B, '2026-10-26T07:00:00Z', { title: '' }),
    row(SESSION_C, 'not a time'),
  ]);
  const open = w.open(bot, SESSION_A);
  open.turnActive = true;
  const { sessions } = await w.call('bot.threads.list', { bot_id: bot });
  assert.deepEqual(sessions.map((session: any) => session.session_id), [SESSION_A, SESSION_B, SESSION_C], 'the controller is no chat; a time that cannot be read sorts last');
  assert.equal(sessions[0].status, 'running');
  assert.equal(sessions[0].model, 'model-one');
  assert.equal(sessions[1].title, 'Untitled session');
});

test('bot.threads.list refuses a Bot that is unknown, switched off or being deleted, and a request that is not one', async (t) => {
  const w = world(t);
  const off = w.bot('Off');
  const on = w.bot('On', { remote: true });
  assert.equal(await rpcCode(w.call('bot.threads.list', { bot_id: off })), 'not_remote_enabled');
  assert.equal(await rpcCode(w.call('bot.threads.list', { bot_id: 'N'.repeat(22) })), 'unknown_session');
  assert.equal(await rpcCode(w.call('bot.threads.list', { bot_id: '../../etc' })), 'unknown_session', 'a name that is no id is simply not there');
  assert.equal(await rpcCode(w.call('bot.threads.list', {})), 'invalid_params');
  assert.equal(await rpcCode(w.call('bot.threads.list', { bot_id: 7 })), 'invalid_params');
  assert.equal(await rpcCode(w.call('bot.threads.list', 'on')), 'invalid_params');
  let release!: () => void;
  w.holdClosing(new Promise<void>((resolve) => { release = resolve; }));
  const deleting = w.service.delete(on);
  await flush();
  const error = await rpcError(w.call('bot.threads.list', { bot_id: on }));
  assert.deepEqual([error.code, error.message], ['unknown_session', 'this Bot is being deleted']);
  release();
  await deleting;
});

test('bot.threads.list shows nothing of a Bot that was switched off while its chats were being read', async (t) => {
  const w = world(t);
  const bot = w.bot('Scribe', { remote: true });
  w.catalog(bot, [row(SESSION_A, '2026-10-26T08:00:00Z')]);
  w.hooks.list = () => w.patch(bot, { remote: false });
  assert.equal(await rpcCode(w.call('bot.threads.list', { bot_id: bot })), 'not_remote_enabled');
});

test('bot.thread.create starts a chat of the Bot, and a retry from the same device gets the same chat', async (t) => {
  const w = world(t);
  const bot = w.bot('Scribe', { remote: true });
  const first = await w.call('bot.thread.create', { bot_id: bot, client_request_id: 'req-1' });
  assert.match(first.session_id, UUID);
  assert.deepEqual(w.created, [first.session_id]);
  assert.deepEqual(await w.call('bot.thread.create', { bot_id: bot, client_request_id: 'req-1' }), first, 'the same request');
  assert.equal(w.created.length, 1, 'one chat');
  const second = await w.call('bot.thread.create', { bot_id: bot, client_request_id: 'req-2' });
  assert.notEqual(second.session_id, first.session_id);
  assert.equal(w.created.length, 2);

  // The same id from another device is another request; so is the same id for another Bot.
  const tablet = newViewer('tablet', true);
  assert.notEqual((await w.call('bot.thread.create', { bot_id: bot, client_request_id: 'req-1' }, tablet)).session_id, first.session_id);
  const other = w.bot('Other', { remote: true });
  assert.notEqual((await w.call('bot.thread.create', { bot_id: other, client_request_id: 'req-1' })).session_id, first.session_id);
  assert.equal(w.created.length, 4);

  // The answer is remembered for the window prompt.send remembers its own for.
  w.fx.clock.advance(IDEMPOTENCY_WINDOW_MS - 1);
  assert.deepEqual(await w.call('bot.thread.create', { bot_id: bot, client_request_id: 'req-1' }), first);
  w.fx.clock.advance(1);
  assert.notEqual((await w.call('bot.thread.create', { bot_id: bot, client_request_id: 'req-1' })).session_id, first.session_id);
});

test('bot.thread.create forgets a chat that could not be started, so that the retry may succeed', async (t) => {
  const w = world(t);
  const bot = w.bot('Scribe', { remote: true });
  w.flags.failNew = new Error('/private/path is not writable');
  const error = await rpcError(w.call('bot.thread.create', { bot_id: bot, client_request_id: 'req-1' }));
  assert.deepEqual([error.code, error.message], ['internal', 'the chat could not be started'], 'no path reaches the viewer');
  assert.ok(w.fx.diagnostics.some((line) => line.includes('/private/path')), 'the log has the reason');
  w.flags.failNew = undefined;
  assert.match((await w.call('bot.thread.create', { bot_id: bot, client_request_id: 'req-1' })).session_id, UUID);
});

test('bot.thread.create refuses a Bot that is unknown, switched off or being deleted, and parameters that are not right', async (t) => {
  const w = world(t);
  const off = w.bot('Off');
  const on = w.bot('On', { remote: true });
  assert.equal(await rpcCode(w.call('bot.thread.create', { bot_id: off, client_request_id: 'r' })), 'not_remote_enabled');
  assert.equal(await rpcCode(w.call('bot.thread.create', { bot_id: 'N'.repeat(22), client_request_id: 'r' })), 'unknown_session');
  assert.equal(await rpcCode(w.call('bot.thread.create', { bot_id: on })), 'invalid_params');
  assert.equal(await rpcCode(w.call('bot.thread.create', { bot_id: on, client_request_id: '' })), 'invalid_params');
  assert.equal(await rpcCode(w.call('bot.thread.create', { bot_id: on, client_request_id: 'x'.repeat(129) })), 'invalid_params');
  assert.equal(await rpcCode(w.call('bot.thread.create', { client_request_id: 'r' })), 'invalid_params');
  assert.equal(await rpcCode(w.call('bot.thread.create', null)), 'invalid_params');
  let release!: () => void;
  w.holdClosing(new Promise<void>((resolve) => { release = resolve; }));
  const deleting = w.service.delete(on);
  await flush();
  const error = await rpcError(w.call('bot.thread.create', { bot_id: on, client_request_id: 'r' }));
  assert.deepEqual([error.code, error.message], ['unknown_session', 'this Bot is being deleted']);
  release();
  await deleting;
  assert.deepEqual(w.created, [], 'none of them started a chat');
});

test('a retry of a chat made before the Bot was switched off is refused too', async (t) => {
  const w = world(t);
  const bot = w.bot('Scribe', { remote: true });
  await w.call('bot.thread.create', { bot_id: bot, client_request_id: 'req-1' });
  w.patch(bot, { remote: false });
  assert.equal(await rpcCode(w.call('bot.thread.create', { bot_id: bot, client_request_id: 'req-1' })), 'not_remote_enabled');
});

// ─────────────────────────────────────────────────────────────────────────────
// Using a chat
// ─────────────────────────────────────────────────────────────────────────────

test('a Bot-aware viewer attaches to a chat, sends prompts, answers questions and stops turns like in any session', async (t) => {
  const w = world(t);
  const bot = w.bot('Scribe', { remote: true });
  w.transcript(bot, SESSION_A);
  const attach = await w.call('session.attach', { session_id: SESSION_A });
  assert.equal(attach.mode, 'snapshot');
  assert.deepEqual(w.fx.runtimes.opened, [{ projectPath: w.workspace(bot), sessionId: SESSION_A }], 'opened in the Bot\'s workspace');
  const runtime = w.fx.runtimes.get(SESSION_A) as FakeRuntime;

  const sent = await w.call('prompt.send', { session_id: SESSION_A, text: 'Hello Bot', client_request_id: 'p1' });
  assert.ok(Number.isSafeInteger(sent.turn_id));
  assert.deepEqual(runtime.callsOf('sendPrompt')[0]!.slice(0, 1), ['Hello Bot']);
  assert.deepEqual(runtime.callsOf('sendPrompt')[0]![3], { origin: 'device', deviceName: 'Test phone' }, 'a phone, as for any session');
  runtime.askPermission(permissionRequest(7));
  await w.call('permission.respond', { session_id: SESSION_A, request_id: 7, decision: { type: 'allow_once' } });
  assert.equal(runtime.callsOf('approvePermission').length, 1);
  runtime.askQuestion(questionRequest(8));
  await w.call('question.answer', { session_id: SESSION_A, request_id: 8, answers: { 'Which one?': 'A' } });
  assert.equal(runtime.callsOf('answerAskUserQuestion').length, 1);
  await w.call('turn.cancel', { session_id: SESSION_A, turn_id: 5 });
  assert.deepEqual(runtime.callsOf('cancelTurn'), [[5]]);
  await w.call('controls.set_model', { session_id: SESSION_A, model: 'model-two' });
  assert.equal(runtime.callsOf('dispatchCommand').length, 1);
  assert.deepEqual((await w.call('models.list', { session_id: SESSION_A })).models, ['model-one', 'model-two']);
  assert.deepEqual((await w.call('session.history', { session_id: SESSION_A, before_index: 0, limit: 5 })).messages, []);
  assert.equal(w.fx.gateway.attachedViewers(SESSION_A), 1);
});

test('a chat started from a phone can be attached and prompted before it has a transcript', async (t) => {
  const w = world(t);
  const bot = w.bot('Scribe', { remote: true });
  const { session_id: sessionId } = await w.call('bot.thread.create', { bot_id: bot, client_request_id: 'r' });
  assert.equal((await w.call('session.attach', { session_id: sessionId })).mode, 'snapshot');
  await w.call('prompt.send', { session_id: sessionId, text: 'First words', client_request_id: 'p' });
  assert.equal((w.fx.runtimes.get(sessionId) as FakeRuntime).callsOf('sendPrompt').length, 1);
});

test('a chat that is neither open nor on disk, the routine controller, and another spelling of an id are not sessions', async (t) => {
  const w = world(t);
  const bot = w.bot('Scribe', { remote: true });
  assert.equal(await rpcCode(w.call('session.attach', { session_id: SESSION_A })), 'unknown_session');
  w.transcript(bot, SESSION_B);
  w.markController(bot, SESSION_B);
  assert.equal(await rpcCode(w.call('session.attach', { session_id: SESSION_B })), 'unknown_session');
  w.transcript(bot, SESSION_C);
  assert.equal(await rpcCode(w.call('session.attach', { session_id: SESSION_C.toUpperCase() })), 'unknown_session', 'only the spelling the engine writes');
  assert.equal((await w.call('session.attach', { session_id: SESSION_C })).mode, 'snapshot');
});

test('a viewer that is not Bot-aware is told there is no such session, whatever the chat\'s state and the Bot\'s switch', async (t) => {
  const w = world(t);
  const bot = w.bot('Scribe', { remote: true });
  w.transcript(bot, SESSION_A); // on disk only
  w.open(bot, SESSION_B); // open
  const quietBot = w.bot('Quiet'); // its switch is off
  w.transcript(quietBot, SESSION_C);
  for (const sessionId of [SESSION_A, SESSION_B, SESSION_C]) {
    const requests: Array<[string, unknown]> = [
      ['session.attach', { session_id: sessionId }],
      ['session.attach', { session_id: sessionId, resume: { epoch: 'rt1:1', after_seq: 1 } }],
      ['session.history', { session_id: sessionId, before_index: 0, limit: 5 }],
      ['prompt.send', { session_id: sessionId, text: 'hi', client_request_id: 'r' }],
      ['turn.cancel', { session_id: sessionId }],
      ['permission.respond', { session_id: sessionId, request_id: 1, decision: { type: 'deny' } }],
      ['question.answer', { session_id: sessionId, request_id: 1, answers: {} }],
      ['controls.set_model', { session_id: sessionId, model: 'm' }],
      ['models.list', { session_id: sessionId }],
      ['image.get', { ref: `${sessionId}:p:1:0` }],
    ];
    for (const [method, params] of requests) {
      const error = await rpcError(w.call(method, params, w.unaware));
      assert.deepEqual([error.code, error.message], ['unknown_session', 'no such session'], `${method} ${sessionId}`);
    }
  }
  assert.deepEqual(w.fx.runtimes.opened, [], 'nothing was opened for it');
  assert.equal(w.fx.gateway.attachedViewers(), 0);
});

test('a Bot-aware viewer is told Remote Control is off for a Bot whose switch is off, and nothing opens', async (t) => {
  const w = world(t);
  const bot = w.bot('Quiet');
  w.transcript(bot, SESSION_A);
  const error = await rpcError(w.call('session.attach', { session_id: SESSION_A }));
  assert.deepEqual([error.code, error.message], ['not_remote_enabled', 'Remote Control is off for this Bot']);
  assert.equal(await rpcCode(w.call('prompt.send', { session_id: SESSION_A, text: 'hi', client_request_id: 'r' })), 'not_remote_enabled');
  assert.deepEqual(w.fx.runtimes.opened, []);
});

test('switching a Bot off closes the attachments to its chats with session.closed, then re-lists the Bots; switching it on lets viewers back', async (t) => {
  const w = world(t);
  const bot = w.bot('Scribe', { remote: true });
  w.transcript(bot, SESSION_A);
  await w.call('session.attach', { session_id: SESSION_A });
  await w.call('bots.watch', {});
  const runtime = w.fx.runtimes.get(SESSION_A) as FakeRuntime;
  w.aware.channel.clear();

  w.patch(bot, { remote: false });
  assert.deepEqual(w.aware.channel.of('session.closed'), [{ session_id: SESSION_A, reason: 'remote_disabled' }], 'at once');
  assert.equal(w.fx.gateway.attachedViewers(SESSION_A), 0);
  assert.equal(w.aware.channel.of('bots.changed').length, 0, 'the list follows after the debounce');
  assert.deepEqual(await w.nextBots(), []);
  assert.deepEqual(w.aware.channel.methods().filter((method) => method === 'session.closed' || method === 'bots.changed'), ['session.closed', 'bots.changed'], 'in that order');
  assert.equal(await rpcCode(w.call('prompt.send', { session_id: SESSION_A, text: 'hi', client_request_id: 'r' })), 'not_remote_enabled');
  assert.equal(await rpcCode(w.call('session.attach', { session_id: SESSION_A })), 'not_remote_enabled');
  w.aware.channel.clear();
  runtime.journal('typed on the desktop');
  w.fx.clock.advance(LIVE_BATCH_MS);
  assert.deepEqual(w.aware.channel.of('session.event'), [], 'nothing more is sent to a viewer that was let go');

  w.patch(bot, { remote: true });
  assert.equal((await w.call('session.attach', { session_id: SESSION_A })).mode, 'snapshot');
  assert.equal(w.fx.gateway.attachedViewers(SESSION_A), 1);
});

test('a viewer that is attaching when its Bot is switched off is turned away, and the lease is given back', async (t) => {
  const w = world(t);
  const bot = w.bot('Scribe', { remote: true });
  w.transcript(bot, SESSION_A);
  let wake!: () => void;
  w.fx.runtimes.openDelay = new Promise<void>((resolve) => { wake = resolve; });
  const pending = w.call('session.attach', { session_id: SESSION_A });
  w.patch(bot, { remote: false });
  wake();
  assert.equal(await rpcCode(pending), 'not_remote_enabled');
  assert.equal(w.fx.gateway.attachedViewers(), 0);
  w.fx.clock.advance(LEASE_GRACE_MS);
  assert.deepEqual(w.fx.runtimes.releases, [SESSION_A]);
});

test('Remote Control off on this computer, or forbidden by the administrator, takes the Bot\'s chats away until it is allowed again', async (t) => {
  const w = world(t);
  const bot = w.bot('Scribe', { remote: true });
  w.transcript(bot, SESSION_A);
  await w.call('session.attach', { session_id: SESSION_A });
  assert.equal(w.fx.policy.botReachable(w.workspace(bot)), true);
  w.aware.channel.clear();

  w.flags.computer = false;
  w.fx.policy.notifySessionsChanged();
  assert.deepEqual(w.aware.channel.of('session.closed'), [{ session_id: SESSION_A, reason: 'remote_disabled' }]);
  assert.equal(w.fx.policy.botReachable(w.workspace(bot)), false);
  assert.equal(await rpcCode(w.call('session.attach', { session_id: SESSION_A })), 'not_remote_enabled');
  assert.equal(await rpcCode(w.call('prompt.send', { session_id: SESSION_A, text: 'hi', client_request_id: 'r' })), 'not_remote_enabled');

  w.flags.computer = true;
  w.fx.policy.notifySessionsChanged();
  assert.equal((await w.call('session.attach', { session_id: SESSION_A })).mode, 'snapshot');
  w.aware.channel.clear();

  w.flags.veto = true;
  assert.equal(w.fx.policy.recheck(), true);
  assert.deepEqual(w.aware.channel.of('session.closed'), [{ session_id: SESSION_A, reason: 'remote_disabled' }]);
  assert.equal(w.fx.policy.botReachable(w.workspace(bot)), false);
  assert.equal(await rpcCode(w.call('session.attach', { session_id: SESSION_A })), 'not_remote_enabled');

  w.flags.veto = false;
  assert.equal(w.fx.policy.recheck(), true);
  assert.equal(w.fx.policy.botReachable(w.workspace(bot)), true);
  assert.equal((await w.call('session.attach', { session_id: SESSION_A })).mode, 'snapshot');
});

test('a Bot that is deleted lets its viewers go', async (t) => {
  const w = world(t);
  const bot = w.bot('Scribe', { remote: true });
  w.transcript(bot, SESSION_A);
  await w.call('session.attach', { session_id: SESSION_A });
  w.aware.channel.clear();
  await w.service.delete(bot);
  assert.deepEqual(w.aware.channel.of('session.closed').map((closed) => closed.session_id), [SESSION_A]);
  assert.equal(w.fx.gateway.attachedViewers(), 0);
  assert.equal(await rpcCode(w.call('session.attach', { session_id: SESSION_A })), 'unknown_session');
});

// ─────────────────────────────────────────────────────────────────────────────
// What viewers that do not know Bots see: the same as before
// ─────────────────────────────────────────────────────────────────────────────

function ordinarySession(w: World): FakeRuntime {
  const runtime = w.fx.runtimes.add(new FakeRuntime(SESSION_A));
  runtime.resume();
  runtime.journal('Hello from the desktop');
  runtime.push({ type: 'text_delta', text: 'Hi' } as any);
  return runtime;
}

/** Everything a viewer that is not Bot-aware is told of the list and of an ordinary session, as one string. */
async function whatAnOlderViewerSees(w: World, name: string): Promise<{ text: string; who: Who }> {
  const who = newViewer(name, false);
  const list = await w.call('sessions.list', {}, who);
  const watch = await w.call('sessions.watch', {}, who);
  const attach = await w.call('session.attach', { session_id: SESSION_A }, who);
  await w.call('session.presence', { session_id: SESSION_A, foreground: true }, who);
  const models = await w.call('models.list', {}, who);
  return { text: JSON.stringify({ list, watch, attach, models, frames: who.channel.sent }), who };
}

test('an older viewer gets byte for byte the same list, watch answer and snapshot with a running Bot whose switch is on as without any Bot', async (t) => {
  const before = world(t, { ordinary: true });
  ordinarySession(before);
  const baseline = await whatAnOlderViewerSees(before, 'older');
  assert.ok(baseline.text.includes('Hello from the desktop'), 'the baseline has substance');

  const after = world(t, { ordinary: true });
  ordinarySession(after);
  const bot = after.bot('Scribe', { remote: true });
  after.transcript(bot, SESSION_B);
  const chat = after.open(bot, SESSION_B);
  chat.turnActive = true;
  chat.askPermission(permissionRequest(3));
  chat.journal('A task from another Bot', { origin: 'bot', fromBotName: 'Sender' });
  after.bot('Quiet');
  assert.equal(after.fx.policy.botChat(SESSION_B)?.reachable, true, 'the Bot is reachable, to viewers that can know it');
  const seen = await whatAnOlderViewerSees(after, 'older');
  assert.equal(seen.text, baseline.text);
  assert.deepEqual(JSON.parse(seen.text).list.sessions.map((session: any) => session.session_id), [SESSION_A], 'only the ordinary session is listed');
  assert.ok(!seen.text.includes('Scribe') && !seen.text.includes(SESSION_B) && !seen.text.includes('Sender'));
});

test('an older viewer that watches the list is not sent a notice because a Bot ran, was renamed or was switched', async (t) => {
  const w = world(t, { ordinary: true });
  ordinarySession(w);
  const bot = w.bot('Scribe', { remote: true });
  const { who } = await whatAnOlderViewerSees(w, 'older');
  w.fx.clock.advance(LIST_DEBOUNCE_MS); // its own attach changed the viewers of the list
  await quiet();
  who.channel.clear();
  const chat = w.open(bot, SESSION_B);
  chat.turnActive = true;
  chat.push({ type: 'turn_started', turn_id: 1 } as any);
  chat.askPermission(permissionRequest(1));
  w.patch(bot, { name: 'Renamed' });
  w.patch(bot, { remote: false });
  w.patch(bot, { remote: true });
  chat.turnActive = false;
  chat.push({ type: 'turn_ended', outcome: { type: 'completed' }, cost: {} } as any);
  w.fx.clock.advance(LIST_DEBOUNCE_MS * 3);
  await quiet();
  assert.deepEqual(who.channel.methods(), [], 'not a single notification');
});

test('a Bot\'s chats are in the session lists of no viewer, aware or not', async (t) => {
  const w = world(t, { ordinary: true });
  ordinarySession(w);
  const bot = w.bot('Scribe', { remote: true });
  w.transcript(bot, SESSION_B);
  w.open(bot, SESSION_C);
  for (const who of [w.aware, w.unaware]) {
    assert.deepEqual((await w.call('sessions.list', {}, who)).sessions.map((session: any) => session.session_id), [SESSION_A]);
    assert.deepEqual((await w.call('sessions.watch', {}, who)).sessions.map((session: any) => session.session_id), [SESSION_A]);
  }
  assert.deepEqual(w.fx.policy.enabledSessions().map((ref) => ref.sessionId), [SESSION_A], 'nor in what the window lists as shared');
  assert.equal(w.fx.policy.isEnabled(SESSION_B), false);
});

// ─────────────────────────────────────────────────────────────────────────────
// Who a task came from
// ─────────────────────────────────────────────────────────────────────────────

/** A session whose journal holds a prompt of every origin, an ordinary session or a Bot's chat. */
function seed(runtime: FakeRuntime): FakeRuntime {
  runtime.resume();
  runtime.journal('typed here', { origin: 'desktop' }, [], 1);
  runtime.journal('earlier task', { origin: 'bot', fromBotName: 'Earlier' }, [], 2);
  runtime.journal('from a phone', { origin: 'device', deviceName: 'Pixel' }, [], 4);
  runtime.journal('a routine', { origin: 'scheduled' }, [], 5);
  return runtime;
}

interface SeenPrompt { origin: string; from_bot_name: string | undefined; device_name: string | undefined }

/** The prompt records a viewer is sent: in the snapshot at attach, live, and in a delta when it resumes. */
async function promptsSeenBy(w: World, who: Who, sessionId: string): Promise<{ snapshot: SeenPrompt[]; live: SeenPrompt[]; delta: SeenPrompt[] }> {
  const runtime = w.fx.runtimes.get(sessionId) as FakeRuntime;
  const prompts = (records: Array<{ record: any }>): SeenPrompt[] => records
    .filter((item) => item.record.type === 'prompt')
    .map((item) => ({ origin: item.record.origin, from_bot_name: item.record.from_bot_name, device_name: item.record.device_name }));
  const attach = await w.call('session.attach', { session_id: sessionId }, who);
  const snapshot = prompts(who.channel.of('session.snapshot').filter((frame) => frame.phase === 'records').flatMap((frame) => frame.records));
  const cursor = { epoch: attach.epoch, after_seq: runtime.lastSequence };
  who.channel.clear();
  runtime.journal('a task handed over', { origin: 'bot', fromBotName: 'Sender' }, [], 3);
  w.fx.clock.advance(LIVE_BATCH_MS);
  const live = prompts(who.channel.records());
  who.channel.clear();
  const resumed = await w.call('session.attach', { session_id: sessionId, resume: cursor }, who);
  assert.equal(resumed.mode, 'delta');
  const delta = prompts(who.channel.records());
  return { snapshot, live, delta };
}

const prompt = (origin: string, extra: Partial<SeenPrompt> = {}): SeenPrompt => ({ origin, from_bot_name: undefined, device_name: undefined, ...extra });

test('a task another Bot handed over shows as scheduled to a viewer that is not Bot-aware: in a snapshot, live and in a delta', async (t) => {
  const w = world(t, { ordinary: true });
  seed(w.fx.runtimes.add(new FakeRuntime(SESSION_A)));
  const seen = await promptsSeenBy(w, w.unaware, SESSION_A);
  assert.deepEqual(seen.snapshot, [prompt('desktop'), prompt('scheduled'), prompt('device', { device_name: 'Pixel' }), prompt('scheduled')]);
  assert.deepEqual(seen.live, [prompt('scheduled')]);
  assert.deepEqual(seen.delta, [prompt('scheduled')]);
});

test('the same task shows as the Bot\'s, with its name, to a Bot-aware viewer: in a snapshot, live and in a delta', async (t) => {
  const w = world(t, { ordinary: true });
  seed(w.fx.runtimes.add(new FakeRuntime(SESSION_A)));
  const seen = await promptsSeenBy(w, w.aware, SESSION_A);
  assert.deepEqual(seen.snapshot, [prompt('desktop'), prompt('bot', { from_bot_name: 'Earlier' }), prompt('device', { device_name: 'Pixel' }), prompt('scheduled')]);
  assert.deepEqual(seen.live, [prompt('bot', { from_bot_name: 'Sender' })]);
  assert.deepEqual(seen.delta, [prompt('bot', { from_bot_name: 'Sender' })]);
});

test('in a Bot\'s own chat, which is where such a task runs, a Bot-aware viewer sees who handed it over', async (t) => {
  const w = world(t);
  const bot = w.bot('Receiver', { remote: true });
  seed(w.open(bot, SESSION_B));
  const seen = await promptsSeenBy(w, w.aware, SESSION_B);
  assert.deepEqual(seen.snapshot[1], prompt('bot', { from_bot_name: 'Earlier' }));
  assert.deepEqual(seen.live, [prompt('bot', { from_bot_name: 'Sender' })]);
  assert.deepEqual(seen.delta, [prompt('bot', { from_bot_name: 'Sender' })]);
});

test('nothing of the sender is on the wire to a viewer that cannot read it', async (t) => {
  const w = world(t, { ordinary: true });
  seed(w.fx.runtimes.add(new FakeRuntime(SESSION_A)));
  await promptsSeenBy(w, w.unaware, SESSION_A);
  const wire = JSON.stringify(w.unaware.channel.sent);
  assert.ok(!wire.includes('Sender') && !wire.includes('Earlier') && !wire.includes('from_bot_name') && !wire.includes('"bot"'));
});

// ─────────────────────────────────────────────────────────────────────────────
// Push
// ─────────────────────────────────────────────────────────────────────────────

test('the push for a Bot\'s chat is asked for like any session\'s, with no title, while the Bot is reachable', async (t) => {
  const w = world(t);
  const bot = w.bot('Scribe', { remote: true });
  w.transcript(bot, SESSION_A);
  const bodies: Array<Record<string, unknown>> = [];
  const requester = new PushRequester({
    baseUrl: 'https://push.example.com/api',
    hostId: () => 'host-1',
    getIdToken: async () => 'token',
    fetch: async (_url, init) => { bodies.push(JSON.parse(init.body)); return { ok: true, status: 200 }; },
    diagnostics: () => undefined,
  });
  const push = new RemotePush({ requester, presence: w.fx.gateway, active: () => true, clock: w.fx.clock });
  assert.equal(w.fx.gateway.isEnabled(SESSION_A), true);
  push.notify('permission', SESSION_A);
  push.notify('question', SESSION_A);
  push.notify('idle', SESSION_A);
  await flush();
  assert.deepEqual(bodies, [
    { kind: 'permission', host_id: 'host-1', session_id: SESSION_A },
    { kind: 'question', host_id: 'host-1', session_id: SESSION_A },
    { kind: 'idle', host_id: 'host-1', session_id: SESSION_A },
  ], 'what the push service needs to address it, and no name: the title is the service\'s');

  // A viewer that is looking at it needs none.
  await w.call('session.attach', { session_id: SESSION_A });
  await w.call('session.presence', { session_id: SESSION_A, foreground: true });
  w.fx.clock.advance(60_000);
  bodies.length = 0;
  push.notify('permission', SESSION_A);
  await flush();
  assert.deepEqual(bodies, []);

  // And none for a Bot that is switched off, or a chat that is nobody's.
  w.patch(bot, { remote: false });
  assert.equal(w.fx.gateway.isEnabled(SESSION_A), false);
  w.fx.clock.advance(60_000);
  push.notify('question', SESSION_A);
  push.notify('question', SESSION_B);
  await flush();
  assert.deepEqual(bodies, []);
});
