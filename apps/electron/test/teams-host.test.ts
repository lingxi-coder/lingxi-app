import test from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import { chmodSync, linkSync, mkdirSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { ChildProcessWithoutNullStreams } from 'node:child_process';
import { initializeApp, deleteApp } from 'firebase/app';
import { getFirestore, terminate } from 'firebase/firestore';
import { TeamsService } from '../src/main/teams/service';
import { AccountService } from '../src/main/account';
import { TeamOwnerApprovalBroker } from '../src/main/remoteControl/teamApprovals';
import { TeamConnector, type TeamHostApi } from '../src/main/teams/connector';
import { TeamPrivateStore } from '../src/main/teams/privateStore';
import { resolveTeamBinaries, runTeamHost, TeamHostFailure, type TeamHostProcessOptions, type TeamHostResult } from '../src/main/teams/process';
import { BotContent } from '../src/main/bots/content';
import { botFixture } from './bot-helpers';
import type { TeamBotCard } from '../../../packages/teams/bots';

const tick = () => new Promise<void>(resolve => setImmediate(resolve));
const threadId = 'thread001', teamId = 'team00001';

test('actual main discovery never SDK-reads Bot cards and a failed conversation watch falls back without a retry storm', async t => {
  const f = botFixture(t), privateStore = new TeamPrivateStore(f.userData);
  const bot = f.store.create({ name: 'HTTP-only Bot', model: 'anthropic/claude-synthetic' });
  const botKey = `bot:owner:${privateStore.hostId}:${bot.bot_id}`;
  const snapshot = privateStore.saveSnapshot('owner', { instructions: 'Frozen', model_id: 'claude-synthetic', model_endpoint: 'https://api.anthropic.com' });
  f.store.setTeamBindings(bot.bot_id, [{ ownerUid: 'owner', teamId, generation: 1, botKey, revision: 1, snapshot, use: 'all', uids: [] }]);
  const app = initializeApp({ projectId: 'demo-teams-fallback', apiKey: 'synthetic', appId: 'synthetic' }, `teams-fallback-${Date.now()}`), db = getFirestore(app);
  let watchCalls = 0, heartbeatCalls = 0, requests = 0;
  const service = new TeamsService({ snapshot: () => ({ user: { uid: 'owner' } }), getIdToken: async () => 'synthetic', onChanged: () => () => {} }, {},
    'https://lingxios.net', undefined, ((...args: unknown[]) => {
      watchCalls++;
      const fail = args.at(-1) as (error: unknown) => void;
      setImmediate(() => fail({ code: 'permission-denied' })); return () => {};
    }) as never);
  // A cached authenticated SDK session is the test seam; the actual service,
  // transport, discovery, private store and fallback lifecycle remain in use.
  (service as unknown as { session: unknown }).session = { db, close: async () => {}, watchChanges: () => () => {} };
  t.mock.method(globalThis, 'fetch', async (_url: unknown, init?: RequestInit) => {
    requests++;
    if (init?.method === 'GET') return Response.json({ teams: [{ id: teamId, generation: 1, name: 'Demo', memberCount: 1, role: 'owner' }] });
    const command = JSON.parse(String(init?.body));
    if (command.action === 'bots') return Response.json({ bots: [{ key: botKey, ownerUid: 'owner', hostId: privateStore.hostId, botId: bot.bot_id,
      revision: 1, use: 'all', uids: [], enabled: true, permitted: true, online: true }] });
    if (command.action === 'list') return Response.json({ conversations: [], next: null });
    if (command.action === 'heartbeatBot') { heartbeatCalls++; return Response.json({ ok: true }); }
    throw new Error('unexpected transport action');
  });
  await service.hostMessages(service.hostContext()!, { action: 'bots', teamId, generation: 1 });
  assert.equal(watchCalls, 0, 'raw Bot SDK reads are denied by policy and must never be attempted');
  const connector = new TeamConnector({ service, store: f.store, privateStore, content: new BotContent(f.store), platform: 'darwin', pollMs: 60_000,
    binaries: () => ({ host: '/unused', worker: '/unused' }), defaultModel: () => 'anthropic/claude-synthetic',
    catalog: async () => ({ models: [] }), modelCredential: async () => 'synthetic', permission: async () => false,
    run: async () => { throw new Error('no queued work'); } });
  service.attachHost(connector);
  t.after(async () => { service.close(); await terminate(db); await deleteApp(app); });
  await connector.idle(); await tick(); await connector.idle(); await tick();
  assert.equal(watchCalls, 1, 'failed watch stays fenced for this account/membership, while HTTP fallback continues');
  assert.equal(heartbeatCalls, 1); assert.ok(requests < 12, 'listener failure must not cause immediate infinite discovery');
});

test('Team credential delivery rejects a delayed token after logout and same-UID login', async () => {
  let uid: string | undefined = 'owner', changed!: () => void, release!: (value: string) => void;
  const service = new TeamsService({ snapshot: () => ({ user: uid ? { uid } : null }),
    getIdToken: () => new Promise(resolve => { release = resolve; }), onChanged: callback => { changed = callback; return () => {}; } }, {});
  const context = service.hostContext()!;
  const pending = service.hostToken(context);
  uid = undefined; changed(); uid = 'owner'; changed(); release('synthetic-token');
  await assert.rejects(pending, /INVALID_SESSION/u);
  await assert.rejects(service.hostToken(context), /INVALID_SESSION/u);
  service.close();
});

test('direct same-UID reauthentication revokes late host credentials and owner approvals', async t => {
  const config = { projectId: 'demo-teams-account', apiKey: 'synthetic-key', appId: 'synthetic-app', authDomain: 'demo.invalid' };
  let saved = JSON.stringify({ projectId: config.projectId, refreshToken: 'original-refresh', user: { uid: 'owner' } });
  let oldRefresh!: (response: Response) => void;
  const account = new AccountService(config, { persistent: false, read: async () => saved,
    write: async value => { saved = value; }, remove: async () => { saved = ''; } }, async () => {}, (async input => {
    const url = String(input);
    if (url.includes('securetoken')) return new Promise<Response>(resolve => { oldRefresh = resolve; });
    if (url.includes('lookup')) return Response.json({ users: [{ localId: 'owner' }] });
    return Response.json({ localId: 'owner', idToken: 'replacement-token', refreshToken: 'replacement-refresh', expiresIn: '3600' });
  }) as typeof fetch);
  await account.execute({ type: 'status' });
  const service = new TeamsService(account, {}), broker = new TeamOwnerApprovalBroker({ botRemoteEnabled: () => true });
  t.after(() => { broker.close(); service.close(); account.dispose(); });
  const context = service.hostContext()!, originalEpoch = account.sessionEpoch(), cancellation = new AbortController();
  let invalidations = 0;
  service.attachHost({ localBots: async () => [], bindLocalBot: async () => { throw new Error('unused'); },
    invalidate: () => { invalidations++; cancellation.abort(); }, stopBinding: () => {}, close: () => {} });
  const credential = service.hostToken(context);
  const refused = assert.rejects(credential);
  const target = { ownerUid: 'owner', ownerGeneration: 1, hostId: 'h'.repeat(22), botId: 'b'.repeat(22), botRevision: 1,
    teamId, conversationId: threadId, botKey: `bot:owner:${'h'.repeat(22)}:${'b'.repeat(22)}`, triggerId: 'trigger01' };
  let allow!: (value: boolean) => void;
  const approval = broker.request({ target, accountEpoch: context.epoch, toolName: 'Bash', input: { command: 'pwd' },
    assertCurrent: () => service.assertHostContext(context), showLocal: () => new Promise(resolve => { allow = resolve; }) }, cancellation.signal);
  await tick();
  const authenticated = await account.execute({ type: 'signIn', email: 'owner@example.invalid', password: 'synthetic-password' });
  assert.equal(authenticated.user?.uid, 'owner');
  assert.ok(account.sessionEpoch() > originalEpoch);
  assert.equal(invalidations, 1, 'same-UID accepted authentication must revoke immediately');
  assert.notEqual(service.hostContext()!.epoch, context.epoch);
  assert.throws(() => service.assertHostContext(context), /INVALID_SESSION/u);
  allow(true); assert.equal(await approval, false);
  oldRefresh(Response.json({ id_token: 'old-late-token', refresh_token: 'old-late-refresh', user_id: 'owner', expires_in: '3600' }));
  await refused;
  assert.equal(await account.getIdToken(), 'replacement-token', 'old refresh must not overwrite the replacement session');
});

test('ordinary Firebase token rotation retains the account and Team host epochs', async t => {
  const account = new AccountService({ projectId: 'demo-teams-account', apiKey: 'key', appId: 'app', authDomain: 'demo.invalid' },
    { persistent: false, read: async () => JSON.stringify({ projectId: 'demo-teams-account', refreshToken: 'refresh', user: { uid: 'owner' } }),
      write: async () => {}, remove: async () => {} }, async () => {}, (async () => Response.json({ id_token: 'rotated', refresh_token: 'rotated-refresh', user_id: 'owner', expires_in: '3600' })) as typeof fetch);
  await account.execute({ type: 'status' }); const service = new TeamsService(account, {});
  t.after(() => { service.close(); account.dispose(); });
  const context = service.hostContext()!, epoch = account.sessionEpoch();
  await account.getIdToken(true); await account.getIdToken(true);
  assert.equal(account.sessionEpoch(), epoch); assert.deepEqual(service.hostContext(), context);
});

test('forged renderer host commands fail before SDK/auth transport', async () => {
  let tokenCalls = 0;
  const service = new TeamsService({ snapshot: () => ({ user: { uid: 'owner' } }), getIdToken: async () => { tokenCalls++; return 'synthetic'; }, onChanged: () => () => {} }, {});
  for (const action of ['bindBot', 'claimBot', 'startBot', 'finishBot', 'botQueue', 'heartbeatBot', 'botQuestion', 'botPoll']) {
    await assert.rejects(service.messages({ action, teamId, generation: 1 } as never, 'owner'), /INVALID_INPUT/u);
  }
  assert.equal(tokenCalls, 0); service.close();
});

function hostFixture(t: { after(fn: () => void): void }, options: { lostBind?: boolean; run?: (input: TeamHostProcessOptions) => Promise<TeamHostResult> } = {}) {
  const f = botFixture(t), content = new BotContent(f.store), privateStore = new TeamPrivateStore(f.userData);
  const bot = f.store.create({ name: 'Team Bot', persona: 'Immutable persona', model: 'anthropic/claude-synthetic' });
  let context = { uid: 'owner', epoch: 1 }, cards: TeamBotCard[] = [], cursor = 0;
  let queue = [{ id: 'trigger01', seq: 1 }, { id: 'trigger02', seq: 2 }], lostBind = options.lostBind;
  const events = new EventEmitter(), calls: Array<Record<string, unknown>> = [], runs: TeamHostProcessOptions[] = [];
  let credentialCalls = 0;
  const service: TeamHostApi = {
    hostContext: () => context,
    assertHostContext: value => { if (value.uid !== context.uid || value.epoch !== context.epoch) throw new Error('INVALID_SESSION'); },
    hostToken: async value => { service.assertHostContext(value); return 'synthetic-firebase'; },
    list: async () => ({ teams: [{ id: teamId, name: 'Synthetic', role: 'owner', generation: 1, memberCount: 2 }] }),
    onChanged: callback => { events.on('change', callback); return () => { events.off('change', callback); }; },
    onMessagesChanged: callback => { events.on('messages', callback); return () => { events.off('messages', callback); }; },
    async hostMessages<T>(value, command): Promise<T> {
      service.assertHostContext(value); calls.push(command);
      if (command.action === 'bindBot') {
        cards = [{ key: `bot:owner:${privateStore.hostId}:${bot.bot_id}`, ownerUid: 'owner', hostId: privateStore.hostId,
          botId: bot.bot_id, revision: Number(command.expectedRevision) + 1, name: 'Team Bot', description: '', use: command.use as 'all', uids: command.uids as string[], permitted: true, online: true }];
        if (lostBind) { lostBind = false; throw new Error('SERVICE_UNAVAILABLE'); }
        return { botKey: cards[0]!.key, revision: cards[0]!.revision } as T;
      }
      if (command.action === 'bots') return { bots: cards } as T;
      if (command.action === 'heartbeatBot') return { ok: true } as T;
      if (command.action === 'list') return { conversations: [{ id: threadId, participants: cards.map(card => card.key) }], next: null } as T;
      if (command.action === 'botQueue') return { cursor, triggers: queue, hasMore: false } as T;
      throw new Error('unexpected command');
    },
  };
  const make = () => new TeamConnector({ service, store: f.store, content, privateStore, platform: 'darwin', pollMs: 60_000,
    binaries: () => ({ host: '/fixed/host', worker: '/fixed/worker' }), defaultModel: () => 'wrong-default',
    catalog: async () => ({ models: [{ providerId: 'anthropic', modelId: 'claude-synthetic', canSelect: true }] }),
    modelCredential: async () => { credentialCalls++; return 'synthetic-model'; }, permission: async () => false,
    run: async input => {
      assert.ok(privateStore.pending('owner').some(value => value.target.triggerId === input.bootstrap.target.triggerId), 'durable discovery must precede launching');
      runs.push(input);
      if (options.run) return options.run(input);
      return new Promise((_resolve, reject) => input.signal.addEventListener('abort', () => reject(new Error('cancelled')), { once: true }));
    } });
  const connector = make(); t.after(() => connector.close());
  const bind = () => connector.bindLocalBot('owner', { teamId, generation: 1, botId: bot.bot_id, use: 'all', uids: [], expectedRevision: 0 });
  return { ...f, bot, content, privateStore, service, events, calls, runs, connector, bind, make,
    revoke: () => { cards = []; events.emit('change', {}); },
    switchAccount: () => { context = { uid: 'replacement', epoch: 2 }; connector.invalidate(); },
    settle: () => { cursor = queue[0]!.seq; queue = queue.slice(1); }, credentialCalls: () => credentialCalls,
  };
}

test('lost bind response discovers saved immutable instructions and stable identity after restart', async t => {
  const f = hostFixture(t, { lostBind: true });
  await assert.rejects(f.bind(), /SERVICE_UNAVAILABLE/u);
  await f.connector.idle(); await tick();
  assert.equal(f.runs.length, 1);
  assert.equal(f.runs[0]!.bootstrap.snapshot!.instructions, 'Immutable persona');
  const first = f.runs[0]!.bootstrap;
  f.connector.close(); await tick();
  await f.content.setPersona(f.bot.bot_id, 'New ordinary persona');
  const restarted = f.make(); t.after(() => restarted.close());
  await restarted.idle(); await tick();
  assert.equal(f.runs.length, 2);
  assert.deepEqual(f.runs[1]!.bootstrap.target, first.target);
  assert.equal(f.runs[1]!.bootstrap.journal_root, first.journal_root);
  assert.equal(f.runs[1]!.bootstrap.mode, 'retry_result');
  assert.equal(f.runs[1]!.bootstrap.snapshot, undefined, 'restart observes saved work without a model credential or fresh execution');
  assert.equal(f.privateStore.snapshot('owner', f.store.get(f.bot.bot_id).teams![0]!.snapshot).instructions, 'Immutable persona');
  assert.notEqual(first.runtime_root, f.store.homeDir(f.bot.bot_id));
  assert.equal(new TeamPrivateStore(f.userData).hostId, f.privateStore.hostId);
});

test('a committed finish with lost ACK is durably retried after restart before the advanced queue head', async t => {
  let invocation = 0;
  const f = hostFixture(t, { run: async input => {
    invocation++;
    if (invocation === 1) { f.settle(); throw new TeamHostFailure('RESULT_PENDING'); }
    if (input.bootstrap.mode === 'retry_result') return { status: 'settled', state: 'completed' };
    return new Promise((_resolve, reject) => input.signal.addEventListener('abort', () => reject(new Error('cancelled')), { once: true }));
  } });
  await f.bind(); await f.connector.idle(); await tick();
  assert.equal(f.privateStore.pending('owner')[0]!.target.triggerId, 'trigger01');
  f.connector.close(); const credentialCount = f.credentialCalls();
  const restarted = f.make(); t.after(() => restarted.close());
  await restarted.idle(); await tick(); await restarted.idle(); await tick();
  assert.deepEqual(f.runs.map(run => [run.bootstrap.mode, run.bootstrap.target.triggerId]), [
    ['run', 'trigger01'], ['retry_result', 'trigger01'], ['run', 'trigger02'],
  ]);
  assert.equal(f.runs[1]!.bootstrap.snapshot, undefined);
  assert.equal(f.credentialCalls() - credentialCount, 1, 'only the newer fresh head may resolve a model credential');
  assert.deepEqual(f.privateStore.pending('owner').map(value => value.target.triggerId), ['trigger02']);
});

test('an HTTP await cannot revive a pending entry that an active host settled meanwhile', async t => {
  let finish!: (result: TeamHostResult) => void, invocations = 0;
  const f = hostFixture(t, { run: input => {
    if (++invocations === 1) return new Promise(resolve => { finish = resolve; });
    return new Promise((_resolve, reject) => input.signal.addEventListener('abort', () => reject(new Error('cancelled')), { once: true }));
  } });
  await f.bind(); await f.connector.idle(); await tick();
  const indexed = f.privateStore.pending('owner')[0]!;
  assert.throws(() => f.privateStore.hasPending('owner', { ...indexed, snapshot: 'f'.repeat(64) }), /UNSAFE_TEAM_STORAGE/u);
  let entered!: () => void, resume!: () => void, pause = true;
  const waiting = new Promise<void>(resolve => { entered = resolve; }), gate = new Promise<void>(resolve => { resume = resolve; });
  const actualList = f.service.list.bind(f.service);
  f.service.list = async uid => {
    const value = await actualList(uid);
    if (pause) { pause = false; entered(); await gate; }
    return value;
  };
  f.events.emit('messages'); await waiting;
  f.settle(); finish({ status: 'settled', state: 'completed' }); await tick();
  assert.equal(f.privateStore.pending('owner').length, 0);
  assert.equal(f.privateStore.hasPending('owner', indexed), false);
  resume(); await f.connector.idle(); await tick();
  assert.deepEqual(f.runs.map(run => [run.bootstrap.mode, run.bootstrap.target.triggerId]), [['run', 'trigger01'], ['run', 'trigger02']]);
  assert.equal(f.connector.runStatus('owner', teamId, threadId, 'trigger01'), undefined, 'acknowledged work must not acquire a misleading retrying status');
});

test('prelaunch index recovery needs an unstarted proof before a fresh queue execution', async t => {
  const f = hostFixture(t, { run: async input => input.bootstrap.mode === 'retry_result'
    ? { status: 'unstarted' } : new Promise((_resolve, reject) => input.signal.addEventListener('abort', () => reject(new Error('cancelled')), { once: true })) });
  await f.bind(); await f.connector.idle(); await tick();
  f.connector.close(); // Model/worker mock did not create any Rust start marker.
  const restarted = f.make(); t.after(() => restarted.close());
  await restarted.idle(); await tick(); await restarted.idle(); await tick();
  assert.deepEqual(f.runs.map(run => [run.bootstrap.mode, run.bootstrap.target.triggerId]), [
    ['run', 'trigger01'], ['retry_result', 'trigger01'], ['run', 'trigger01'],
  ]);
  assert.equal(f.runs[1]!.bootstrap.snapshot, undefined);
});

test('remote owner recovery can settle a blocked pending index without restarting Desktop', async t => {
  let recovered = false;
  const f = hostFixture(t, { run: async input => {
    if (input.bootstrap.target.triggerId === 'trigger02') return new Promise((_resolve, reject) => input.signal.addEventListener('abort', () => reject(new Error('cancelled')), { once: true }));
    return recovered ? { status: 'settled', state: 'recovered' } : { status: 'recovery_required' };
  } });
  await f.bind(); await f.connector.idle(); await tick();
  recovered = true; f.settle(); f.events.emit('messages');
  await f.connector.idle(); await tick(); await f.connector.idle(); await tick();
  assert.deepEqual(f.runs.map(run => [run.bootstrap.mode, run.bootstrap.target.triggerId]), [
    ['run', 'trigger01'], ['retry_result', 'trigger01'], ['run', 'trigger02'],
  ]);
  assert.deepEqual(f.privateStore.pending('owner').map(value => value.target.triggerId), ['trigger02']);
});

test('nonadvancing observations wait for polling or external hints instead of their own status echo', async t => {
  for (const outcome of ['recovery_required', 'busy', 'RESULT_PENDING', 'SERVICE_UNAVAILABLE'] as const) {
    let acknowledged = false, notifications = 0;
    const f = hostFixture(t, { run: async input => {
      if (input.bootstrap.target.triggerId === 'trigger02') return new Promise((_resolve, reject) => input.signal.addEventListener('abort', () => reject(new Error('cancelled')), { once: true }));
      if (acknowledged) return { status: 'settled', state: 'recovered' };
      if (outcome === 'RESULT_PENDING' || outcome === 'SERVICE_UNAVAILABLE') throw new TeamHostFailure(outcome);
      return { status: outcome };
    } });
    // Actual TeamsService synchronously emits its UI message hint here. This
    // must not be mistaken for a new server queue or remote recovery event.
    f.service.notifyHostChanged = () => { notifications++; f.events.emit('messages'); };
    await f.bind(); await f.connector.idle();
    for (let tickIndex = 0; tickIndex < 8; tickIndex++) { await tick(); await f.connector.idle(); }
    assert.equal(f.runs.length, 1, `${outcome}: no completion/status feedback launch before the 60-second poll`);
    const firstNotifications = notifications;
    f.events.emit('messages'); await f.connector.idle();
    for (let tickIndex = 0; tickIndex < 8; tickIndex++) { await tick(); await f.connector.idle(); }
    assert.equal(f.runs.length, 2, `${outcome}: one external hint permits one observation, not a hot retry loop`);
    assert.equal(f.runs[1]!.bootstrap.mode, 'retry_result');
    assert.equal(notifications, firstNotifications, 'unchanged status must not emit another local hint');
    acknowledged = true; f.settle(); f.events.emit('messages');
    await f.connector.idle(); await tick(); await f.connector.idle(); await tick();
    assert.deepEqual(f.runs.map(run => [run.bootstrap.mode, run.bootstrap.target.triggerId]), [
      ['run', 'trigger01'], ['retry_result', 'trigger01'], ['retry_result', 'trigger01'], ['run', 'trigger02'],
    ], `${outcome}: remote settlement must still immediately advance to the next fresh head`);
    f.connector.close();
  }
});

test('journal BUSY exceptions preserve known saved-result and recovery diagnostics without a feedback retry', async t => {
  for (const initial of ['result_pending', 'recovery_required'] as const) {
    let attempts = 0, notifications = 0;
    const f = hostFixture(t, { run: async () => {
      if (++attempts > 1) throw new TeamHostFailure('BUSY');
      if (initial === 'result_pending') throw new TeamHostFailure('RESULT_PENDING');
      return { status: 'recovery_required' };
    } });
    f.service.notifyHostChanged = () => { notifications++; f.events.emit('messages'); };
    await f.bind(); await f.connector.idle(); await tick();
    assert.equal(f.connector.runStatus('owner', teamId, threadId, 'trigger01'), initial);
    const priorNotifications = notifications;
    f.events.emit('messages'); await f.connector.idle(); await tick();
    for (let tickIndex = 0; tickIndex < 8; tickIndex++) { await tick(); await f.connector.idle(); }
    assert.equal(f.runs.length, 2, 'journal contention must wait for another external hint or poll');
    assert.equal(f.runs[1]!.bootstrap.mode, 'retry_result');
    assert.equal(f.connector.runStatus('owner', teamId, threadId, 'trigger01'), initial, 'BUSY must not re-enable manual recovery by hiding a pending exact result');
    assert.equal(notifications, priorNotifications, 'BUSY must not emit a changed diagnostic or self-wake');
    f.connector.close();
  }
});

test('overlapping discovery cannot skip authoritative head or launch concurrent thread workers', async t => {
  let release!: (result: TeamHostResult) => void;
  const f = hostFixture(t, { run: () => new Promise(resolve => { release = resolve; }) });
  await f.bind(); await f.connector.idle(); await tick();
  for (let i = 0; i < 10; i++) f.events.emit('messages');
  await f.connector.idle();
  assert.equal(f.runs.length, 1);
  assert.equal(f.runs[0]!.bootstrap.target.triggerId, 'trigger01');
  f.settle(); release({ status: 'settled', state: 'completed' }); await tick(); await f.connector.idle(); await tick();
  assert.equal(f.runs.length, 2);
  assert.equal(f.runs[1]!.bootstrap.target.triggerId, 'trigger02');
  assert.ok(f.calls.filter(call => call.action === 'botQueue').every(call => call.after === 0));
});

test('ambiguous host recovery blocks the head; account or remote binding revocation cancels live work', async t => {
  const f = hostFixture(t, { run: async () => ({ status: 'recovery_required' }) });
  await f.bind(); await f.connector.idle(); await tick();
  f.events.emit('messages'); await f.connector.idle(); await tick();
  assert.equal(f.runs.filter(run => run.bootstrap.mode === 'run').length, 1, 'no fresh replay of an ambiguous head');
  assert.ok(f.runs.filter(run => run.bootstrap.mode === 'retry_result').every(run => run.bootstrap.snapshot === undefined));
  f.connector.close();
  const g = hostFixture(t); await g.bind(); await g.connector.idle(); await tick();
  assert.equal(g.runs.length, 1); g.revoke(); await g.connector.idle();
  assert.equal(g.runs[0]!.signal.aborted, true);
  const h = hostFixture(t); await h.bind(); await h.connector.idle(); await tick(); h.switchAccount();
  assert.equal(h.runs[0]!.signal.aborted, true);
});

test('unsupported selected model fails explicitly without binding or fallback', async t => {
  const f = hostFixture(t);
  f.store.update(f.bot.bot_id, 1, { model: 'openai/other-model' });
  await assert.rejects(f.bind(), /TEAM_MODEL_UNSUPPORTED/u);
  assert.equal(f.calls.some(call => call.action === 'bindBot'), false);
});

test('a stale bind revision cannot replace the immutable snapshot of an existing binding', async t => {
  const f = hostFixture(t); await f.bind(); await f.connector.idle();
  const oldSnapshot = f.store.get(f.bot.bot_id).teams![0]!.snapshot;
  await f.content.setPersona(f.bot.bot_id, 'Different snapshot');
  await assert.rejects(f.bind(), /BOT_REVISION_CONFLICT/u);
  assert.equal(f.store.get(f.bot.bot_id).teams![0]!.snapshot, oldSnapshot);
});

test('snapshot copies only regular bounded persona and SKILL text, refusing links/hardlinks', async t => {
  const f = hostFixture(t), home = f.store.homeDir(f.bot.bot_id), skills = join(home, 'skills');
  mkdirSync(join(skills, 'synthetic')); writeFileSync(join(skills, 'synthetic', 'SKILL.md'), 'Only this skill text');
  writeFileSync(join(skills, 'synthetic', 'secret-data'), 'not copied');
  const text = await f.content.teamInstructions(f.bot.bot_id);
  assert.ok(text.includes('Only this skill text')); assert.ok(!text.includes('not copied'));
  const file = join(skills, 'synthetic', 'SKILL.md'); rmSync(file); symlinkSync(join(home, 'LINGXI.md'), file);
  await assert.rejects(f.content.teamInstructions(f.bot.bot_id));
  rmSync(file); linkSync(join(home, 'LINGXI.md'), file); await assert.rejects(f.content.teamInstructions(f.bot.bot_id));
  const roots = f.privateStore.owner('owner'); chmodSync(roots.journal, 0o755);
  assert.throws(() => f.privateStore.owner('owner'), /UNSAFE_TEAM_STORAGE/u);
});

function fakeProcess() {
  const child = new EventEmitter() as ChildProcessWithoutNullStreams;
  Object.assign(child, { stdin: new PassThrough(), stdout: new PassThrough(), stderr: new PassThrough(), exitCode: null,
    kill: () => { Object.assign(child, { exitCode: 0 }); child.emit('close', 0); return true; } });
  return child;
}
test('JSONL pipe binds delayed tokens to epoch and clears ambient environment', async t => {
  const f = hostFixture(t); await f.bind(); await f.connector.idle(); await tick(); f.connector.close();
  const template = f.runs[0]!, child = fakeProcess(); let current = true, release!: (value: string) => void, stdin = '';
  child.stdin.on('data', chunk => { stdin += chunk.toString(); });
  const pending = runTeamHost({ ...template, signal: new AbortController().signal,
    assertCurrent: () => { if (!current) throw new Error('INVALID_SESSION'); }, token: () => new Promise(resolve => { release = resolve; }),
    spawnProcess: (executable, args, options) => {
      assert.deepEqual(args, []); assert.equal(executable, '/fixed/host');
      assert.deepEqual(Object.keys(options!.env!).sort(), ['CFFIXED_USER_HOME', 'HOME', 'LANG', 'LC_ALL', 'LINGXI_CONFIG_DIR', 'PATH', 'TMPDIR', '__CF_USER_TEXT_ENCODING'].sort());
      return child;
    } });
  child.stdout.write('{"version":1,"type":"credential","id":"credential-1"}\n'); await tick();
  current = false; release('must-never-cross-pipe');
  await assert.rejects(pending); assert.ok(!stdin.includes('must-never-cross-pipe'));
});

test('unexpected/duplicate stdout frames and incomplete EOF refuse success', async t => {
  const f = hostFixture(t); await f.bind(); await f.connector.idle(); await tick(); f.connector.close();
  for (const output of [
    '{"version":1,"type":"result","status":"unstarted"}\n',
    '{"version":1,"type":"result","status":"settled","extra":true}\n',
    '{"version":1,"type":"result","status":"settled"}\n{"version":1,"type":"result","status":"settled"}\n',
    '{"version":1,"type":"result","status":"settled"}',
  ]) {
    const child = fakeProcess();
    const pending = runTeamHost({ ...f.runs[0]!, signal: new AbortController().signal, assertCurrent: () => {}, spawnProcess: () => child });
    child.stdout.write(output); Object.assign(child, { exitCode: 0 }); child.emit('close', 0);
    await assert.rejects(pending);
  }
});

test('credential refresh proceeds while a tool approval waits, and a stale approval cannot cross the pipe', async t => {
  const f = hostFixture(t); await f.bind(); await f.connector.idle(); await tick(); f.connector.close();
  const child = fakeProcess(); let stdin = '', current = true, answer!: (allow: boolean) => void;
  child.stdin.on('data', chunk => { stdin += chunk.toString(); });
  const pending = runTeamHost({ ...f.runs[0]!, signal: new AbortController().signal, spawnProcess: () => child,
    token: async () => 'synthetic-refresh', assertCurrent: () => { if (!current) throw new Error('INVALID_SESSION'); },
    permission: () => new Promise(resolve => { answer = resolve; }) });
  child.stdout.write('{"version":1,"type":"permission","id":"permission-1","tool_name":"Bash","input":{"command":"pwd"}}\n');
  await tick();
  child.stdout.write('{"version":1,"type":"credential","id":"credential-2"}\n'); await tick();
  assert.ok(stdin.includes('synthetic-refresh'), 'authorization polling must not wait for the owner dialog');
  current = false; answer(true); await assert.rejects(pending);
  assert.ok(!stdin.includes('"allow":true'));
});

test('a full question wait can refresh over 600 credentials while an approval overlaps, without a lifetime ID cap', async t => {
  const f = hostFixture(t); await f.bind(); await f.connector.idle(); await tick(); f.connector.close();
  for (const replay of [undefined, 'credential-350', 'permission-1']) {
    const child = fakeProcess(); let stdin = '', answer!: (value: boolean) => void;
    child.stdin.on('data', chunk => { stdin += chunk.toString(); });
    const pending = runTeamHost({ ...f.runs[0]!, signal: new AbortController().signal, spawnProcess: () => child,
      token: async () => 'synthetic-rotation', assertCurrent: () => {}, permission: () => new Promise(resolve => { answer = resolve; }) });
    const rejected = replay ? assert.rejects(pending, /TEAM_HOST_PROTOCOL/u) : undefined;
    child.stdout.write('{"version":1,"type":"permission","id":"permission-1","tool_name":"Bash","input":{"command":"pwd"}}\n');
    await tick();
    for (let sequence = 2; sequence <= 702; sequence++) {
      child.stdout.write(`${JSON.stringify({ version: 1, type: 'credential', id: `credential-${sequence}` })}\n`);
      await tick();
    }
    assert.equal(stdin.split('\n').filter(line => line.includes('"type":"credential"')).length, 701);
    if (replay) {
      child.stdout.write(`${JSON.stringify(replay.startsWith('permission')
        ? { version: 1, type: 'permission', id: replay, tool_name: 'Bash', input: {} }
        : { version: 1, type: 'credential', id: replay })}\n`);
      await rejected; answer(false);
    } else {
      answer(false); await tick();
      child.stdout.write('{"version":1,"type":"result","status":"settled","state":"completed"}\n');
      Object.assign(child, { exitCode: 0 }); child.emit('close', 0);
      assert.equal((await pending).status, 'settled');
    }
  }
});

test('packaged binary resolution ignores all development overrides', t => {
  const f = botFixture(t); mkdirSync(join(f.root, 'bin'));
  for (const name of ['host', 'worker']) writeFileSync(join(f.root, 'bin', `harness-team-${name}`), 'fixture', { mode: 0o700 });
  const environment = { LINGXI_TEAM_RUNTIME_VALIDATION: '1', LINGXI_TEAM_HOST_BIN: '/malicious/host', LINGXI_TEAM_WORKER_BIN: '/malicious/worker' };
  assert.deepEqual(resolveTeamBinaries({ isPackaged: true, resourcesPath: f.root, environment }), {
    host: join(f.root, 'bin', 'harness-team-host'), worker: join(f.root, 'bin', 'harness-team-worker'),
  });
  assert.throws(() => resolveTeamBinaries({ isPackaged: false, environment: {} }), /TEAM_RUNTIME_UNAVAILABLE/u);
});
