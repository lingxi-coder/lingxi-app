import assert from 'node:assert/strict';
import { EventEmitter, once } from 'node:events';
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { PassThrough } from 'node:stream';
import { test, type TestContext } from 'node:test';
import { BRIDGE_PROTOCOL_VERSION, CLIENT_PROTOCOL_VERSION } from '@lingxi/bridge-client';
import type { TaskRowDto } from '@lingxi/bridge-client';
import { SessionRuntimeManager } from '../src/main/sessionRuntimeManager.js';

const require = createRequire(new URL('../../../packages/bridge-client/package.json', import.meta.url));
const { WebSocketServer } = require('ws');
const ref = { projectPath: '/controller-workspace', sessionId: '11111111-2222-4333-8444-555555555555' };
const authToken = '0123456789abcdef0123456789abcdef';

function fakeProcess(pid: number) {
  return Object.assign(new EventEmitter(), {
    pid, exitCode: null as number | null, signalCode: null as string | null,
    stdin: new PassThrough(), stdout: new PassThrough(), stderr: new PassThrough(),
  });
}

async function fixture(t: TestContext, external = false) {
  const directory = mkdtempSync(join(tmpdir(), 'lingxi-controller-recovery-'));
  const bridgeRoot = join(directory, 'managed');
  const launchDir = join(directory, 'existing');
  mkdirSync(bridgeRoot, { mode: 0o700 });
  mkdirSync(launchDir, { mode: 0o700 });
  const server = new WebSocketServer({ host: '127.0.0.1', port: 0 });
  await once(server, 'listening');
  const port = server.address().port;
  const lockfilePath = join(launchDir, `${port}.lock`);
  const pid = 999_901;
  const writeLockfile = (path: string, processId: number) => writeFileSync(path, JSON.stringify({
    pid: processId, workspaceFolders: [ref.projectPath], ideName: 'LingXi-Bridge',
    transport: 'ws', runningInWindows: false, authToken,
  }), { mode: 0o600 });
  writeLockfile(lockfilePath, pid);
  let connections = 0;
  let rejectNextHandshake = false;
  let holdCron = false;
  let activeWorkers = 0;
  let holdSnapshot = false;
  let failSnapshot = false;
  let incompleteSnapshot = false;
  let tasks: TaskRowDto[] = [];
  const snapshotReplies: Array<() => void> = [];
  let agents: Array<{ agent_id: string; name: string; agent_type: string; status: string }> = [];
  const received: string[] = [];
  server.on('connection', (socket: any, request: any) => {
    connections++;
    assert.equal(request.headers['x-lingxi-ide-authorization'], authToken);
    socket.on('message', (data: Buffer) => {
      const { payload } = JSON.parse(data.toString());
      received.push(payload.method);
      if (payload.method === 'hello') {
        if (rejectNextHandshake) {
          rejectNextHandshake = false;
          socket.send(JSON.stringify({ type: 'response', payload: { id: payload.id, error: { code: -1, message: 'fixture handshake refusal' } } }));
        } else {
          socket.send(JSON.stringify({ type: 'response', payload: { id: payload.id, result: {
            protocol_version: BRIDGE_PROTOCOL_VERSION, server_name: 'fake-controller',
            capabilities: { supports_streaming: true, supports_tools: true, supports_skills: true,
              supports_commands: true, client_protocol_version: CLIENT_PROTOCOL_VERSION },
          } } }));
        }
      } else if (payload.method === 'desktop_runtime_snapshot') {
        const events = [
          { type: 'session_agent_list', session_id: ref.sessionId, agents },
          ...Array.from({ length: activeWorkers }, (_, index) => ({ type: 'coordinator_worker', worker: { agent_id: `worker-${index}`, name: `Worker ${index}`, agent_type: 'executor', status: 'working' } })),
          { type: 'coordinator_status', active_workers: activeWorkers },
          ...tasks.map((task) => ({ type: 'task_row', task })),
          ...(!incompleteSnapshot ? [{ type: 'task_list_complete', request_id: 'desktop-runtime-snapshot',
            active_count: tasks.filter((task) => ['pending', 'running', 'paused'].includes(task.status.type)).length }] : []),
        ];
        const reply = () => socket.send(JSON.stringify({ type: 'response', payload: failSnapshot
          ? { id: payload.id, error: { code: -1, message: 'fixture snapshot refusal' } }
          : { id: payload.id, result: { events } } }));
        if (holdSnapshot) snapshotReplies.push(reply);
        else reply();
      } else if (payload.method === 'permission_request_scope') {
        socket.send(JSON.stringify({ type: 'response', payload: { id: payload.id,
          result: { request_id: payload.params.request_id, background_owned: false } } }));
      } else if (payload.method === 'cron_manage' && !holdCron) {
        socket.send(JSON.stringify({ type: 'event', payload: {
          type: 'cron_result', request_id: payload.params.request_id, jobs: [],
        } }));
      }
    });
  });
  let launches = 0;
  const manager = new SessionRuntimeManager({
    bridgeRoot,
    accessState: () => ({ workspace: ref.projectPath, trusted: true }),
    launchConfig: () => { launches++; return { workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true, scheduledController: true }; },
    readProcessCommand: () => undefined, listProcessCommands: () => [], lockfileTimeoutMs: 1000,
  });
  const runtime = await manager.ensure(ref, false);
  const internal = runtime as any;
  internal.activeWorkspace = ref.projectPath;
  internal.activeWorkspaceTrusted = true;
  internal.launchDir = launchDir;
  const child = fakeProcess(pid);
  if (external) { internal.adoptedPid = pid; internal.adoptedProcessOwned = false; }
  else internal.child = child;
  let processAlive = true;
  internal.processIsAlive = () => processAlive;
  let kills = 0;
  internal.signalChildTree = () => { kills++; throw new Error('unexpected sidecar termination'); };
  internal.stopAdoptedBridge = () => { kills++; throw new Error('unexpected adopted process termination'); };
  let spawns = 0;
  internal.spawnServer = () => { spawns++; throw new Error('unexpected duplicate sidecar spawn'); };
  await internal.connectBridgeClient(lockfilePath, internal.generation, false);
  const disconnect = async () => {
    const client = internal.client;
    const closed = once(client, 'close');
    for (const socket of server.clients) socket.terminate();
    await closed;
    assert.equal(runtime.connectionState.status, 'disconnected');
  };
  t.after(async () => {
    failSnapshot = true;
    for (const reply of snapshotReplies.splice(0)) reply();
    internal.child = null;
    internal.adoptedPid = null;
    internal.adoptedProcessOwned = false;
    await manager.dispose();
    for (const socket of server.clients) socket.terminate();
    await new Promise<void>((resolve) => server.close(() => resolve()));
    rmSync(directory, { recursive: true, force: true });
  });
  return {
    manager, runtime, internal, child, bridgeRoot, launchDir, lockfilePath, port, writeLockfile, disconnect, received,
    get connections() { return connections; }, get kills() { return kills; },
    get spawns() { return spawns; }, get launches() { return launches; },
    set alive(value: boolean) { processAlive = value; },
    set holdCron(value: boolean) { holdCron = value; },
    set activeWorkers(value: number) { activeWorkers = value; },
    set holdSnapshot(value: boolean) { holdSnapshot = value; },
    get pendingSnapshots() { return snapshotReplies.length; },
    flushSnapshots: (fail = false) => { failSnapshot = fail; for (const reply of snapshotReplies.splice(0)) reply(); },
    set agents(value: typeof agents) { agents = value; },
    set tasks(value: TaskRowDto[]) { tasks = value; },
    set incompleteSnapshot(value: boolean) { incompleteSnapshot = value; },
    rejectNextHandshake: () => { rejectNextHandshake = true; },
  };
}

test('concurrent controller ensure reconnects the same live sidecar once and resumes cron requests', async (t) => {
  const h = await fixture(t);
  h.holdCron = true;
  const previousClient = h.internal.client;
  const pending = h.runtime.manageCron({ action: 'list' });
  const rejected = assert.rejects(pending, /connection interrupted/);
  await h.disconnect();
  h.holdCron = false;
  const recovered = await Promise.all(Array.from({ length: 8 }, () => h.manager.ensure(ref, true)));
  await rejected;
  for (const runtime of recovered) assert.strictEqual(runtime, h.runtime);
  assert.equal(h.runtime.connectionState.status, 'connected');
  assert.equal(h.connections, 2, 'one initial connection plus one shared reconnect');
  assert.equal(h.kills, 0);
  assert.equal(h.spawns, 0);
  assert.equal(h.launches, 0, 'existing sidecar credentials and configuration are preserved');
  assert.strictEqual(h.internal.child, h.child);
  assert.doesNotThrow(() => previousClient.emit('error', new Error('late socket failure')));
  assert.equal(h.runtime.connectionState.status, 'connected', 'stale errors cannot disconnect the recovered runtime');
  assert.deepEqual(await h.runtime.manageCron({ action: 'list' }), []);
});

test('failed reconnect preserves an external sidecar and retries its original endpoint', async (t) => {
  const h = await fixture(t, true);
  await h.disconnect();
  h.rejectNextHandshake();
  await assert.rejects(h.manager.ensure(ref, true), /fixture handshake refusal/);
  assert.equal(h.kills, 0);
  assert.equal(h.spawns, 0);
  assert.equal(h.launches, 0);
  assert.equal(h.internal.adoptedPid, 999_901);
  assert.equal(h.internal.adoptedProcessOwned, false);
  assert.equal(existsSync(h.lockfilePath), true);
  assert.strictEqual(await h.manager.ensure(ref, true), h.runtime);
  assert.equal(h.connections, 3);
  assert.deepEqual(await h.runtime.manageCron({ action: 'list' }), []);
});

test('controller starts one replacement only after its former process has exited', async (t) => {
  const h = await fixture(t);
  await h.disconnect();
  h.alive = false;
  h.child.exitCode = 1;
  const replacement = fakeProcess(999_902);
  let replacements = 0;
  h.internal.spawnServer = (_launch: unknown, directory: string) => {
    replacements++;
    h.writeLockfile(join(directory, `${h.port}.lock`), replacement.pid);
    return replacement;
  };
  const recovered = await Promise.all([h.manager.ensure(ref, true), h.manager.ensure(ref, true)]);
  assert.strictEqual(recovered[0], h.runtime);
  assert.strictEqual(recovered[1], h.runtime);
  assert.equal(replacements, 1);
  assert.equal(h.launches, 1);
  assert.equal(h.kills, 0, 'an exited process must not be signalled');
  assert.strictEqual(h.internal.child, replacement);
  assert.equal(h.runtime.connectionState.status, 'connected');
  assert.deepEqual(await h.runtime.manageCron({ action: 'list' }), []);
  await h.disconnect();
  h.alive = true;
  await h.manager.ensure(ref, true);
  assert.equal(replacements, 1, 'reconnecting a live replacement must not spawn again');
  replacement.exitCode = 1;
  replacement.emit('exit', 1, null);
  assert.equal(h.internal.child, null, 'process-exit ownership survives a transport generation change');
  assert.equal(h.runtime.connectionState.status, 'disconnected');
});

test('recovery rejects a changed PID binding instead of connecting or spawning a competing sidecar', async (t) => {
  const h = await fixture(t);
  await h.disconnect();
  h.writeLockfile(h.lockfilePath, 999_903);
  await assert.rejects(h.manager.ensure(ref, true), /pid mismatch/);
  assert.equal(h.connections, 1);
  assert.equal(h.kills, 0);
  assert.equal(h.spawns, 0);
  assert.strictEqual(h.internal.child, h.child);
});

for (const operation of ['open', 'restart'] as const) {
  test(`user ${operation} authenticates the living sidecar and releases only its disconnected foreground owner`, async (t) => {
    const h = await fixture(t);
    const events: any[] = [];
    const broadcast = h.internal.broadcastClientEvent.bind(h.internal);
    h.internal.broadcastClientEvent = (event: any) => { events.push(event); broadcast(event); };
    h.internal.client.emit('event', { type: 'turn_started', turn_id: 17 });
    h.internal.client.emit('event', { type: 'coordinator_status', active_workers: 1 });
    h.activeWorkers = 1;
    h.internal.client.emit('event', { type: 'cost_update', total_usd: 5, input_tokens: 11, output_tokens: 7, api_calls: 2, session_duration_secs: 3, formatted: 'prior cumulative cost' });
    h.internal.client.emit('permission', { request_id: 1, kind: { type: 'exit_plan_mode' } });
    for (let attempt = 0; attempt < 50 && h.internal.permissionScopeChecks.size; attempt++) await new Promise((resolve) => setImmediate(resolve));
    h.internal.client.emit('event', { type: 'ask_user_question', request: { request_id: 2, questions: [] } });
    await h.disconnect();
    assert.equal(h.runtime.turnActive, true, 'unverified disconnect must not discard process ownership');
    assert.equal(h.runtime.hasActiveAgents, true);
    assert.equal(h.runtime.pendingInteractions, 2);
    if (operation === 'open') assert.strictEqual(await h.manager.openSession(ref), h.runtime);
    else await h.manager.restart(ref);
    assert.equal(h.runtime.connectionState.status, 'connected');
    assert.equal(h.runtime.turnActive, false);
    assert.equal(h.runtime.pendingInteractions, 0);
    assert.equal(h.runtime.hasActiveAgents, true, 'living background scopes survive the foreground close barrier');
    assert.equal(h.kills, 0);
    assert.equal(h.spawns, 0);
    assert.equal(h.connections, 2);
    assert.ok(!h.received.includes('resume_session'), 'same-process recovery must not switch the session beneath retained background scopes');
    const terminal = events.findLast((event) => event.type === 'turn_ended');
    assert.equal(terminal?.outcome.type, 'cancelled');
    assert.equal(terminal?.cost.total_usd, 5, 'reconciliation preserves last confirmed cumulative usage');
    assert.equal(terminal?.cost.formatted, '', 'previous turn summaries are not appended twice');
    await assert.rejects(h.manager.closeSession(ref), /active work/);
    await assert.rejects(h.manager.restart(ref), /active work/);
    h.internal.client.emit('event', { type: 'coordinator_status', active_workers: 0 });
    assert.equal(h.runtime.hasActiveWork, false);
    const release = h.runtime.beginArchive();
    release();
  });
}

test('failed user reconnect keeps a living background owner and retries without replacing it', async (t) => {
  const h = await fixture(t, true);
  h.internal.client.emit('event', { type: 'turn_started', turn_id: 18 });
  h.internal.client.emit('event', { type: 'coordinator_status', active_workers: 1 });
  h.activeWorkers = 1;
  await h.disconnect();
  h.rejectNextHandshake();
  await assert.rejects(h.manager.openSession(ref), /fixture handshake refusal/);
  assert.equal(h.runtime.hasActiveAgents, true);
  assert.equal(h.runtime.turnActive, true, 'foreground is reconciled only after the authenticated close barrier');
  assert.equal(h.kills, 0);
  assert.equal(h.spawns, 0);
  await h.manager.openSession(ref);
  assert.equal(h.runtime.turnActive, false);
  assert.equal(h.runtime.hasActiveAgents, true);
  assert.equal(h.kills, 0);
  assert.equal(h.spawns, 0);
});

test('a new foreground turn admitted during hello keeps its owner and interactions', async (t) => {
  const h = await fixture(t);
  h.internal.client.emit('event', { type: 'turn_started', turn_id: 19 });
  await h.disconnect();
  const connect = h.internal.connectBridgeClient.bind(h.internal);
  h.internal.connectBridgeClient = async (...args: unknown[]) => {
    await connect(...args);
    h.internal.client.emit('event', { type: 'turn_started', turn_id: 20 });
    h.internal.client.emit('permission', { request_id: 21, kind: { type: 'exit_plan_mode' } });
  };
  await h.manager.openSession(ref);
  assert.equal(h.runtime.turnActive, true);
  assert.equal(h.internal.activeTurnId, 20);
  assert.equal(h.runtime.pendingInteractions, 1);
  assert.equal(h.kills, 0);
});

test('an authenticated roster releases workers and agents that finished while disconnected', async (t) => {
  const h = await fixture(t);
  h.internal.client.emit('event', { type: 'turn_started', turn_id: 30 });
  h.internal.client.emit('event', { type: 'coordinator_worker', worker: { agent_id: 'completed-offline', name: 'Prior worker', agent_type: 'executor', status: 'working' } });
  h.internal.client.emit('event', { type: 'coordinator_status', active_workers: 1 });
  h.internal.client.emit('event', { type: 'session_agent_updated', session_id: ref.sessionId, agent: { agent_id: 'old-agent', name: 'Prior agent', agent_type: 'executor', status: 'running' } });
  await h.disconnect();
  assert.equal(h.runtime.hasActiveAgents, true);
  h.activeWorkers = 0;
  h.agents = [{ agent_id: 'old-agent', name: 'Prior agent', agent_type: 'executor', status: 'completed' }];
  await h.manager.openSession(ref);
  assert.equal(h.runtime.hasActiveWork, false, 'only a complete authenticated snapshot releases missed background completions');
  assert.equal(h.kills, 0);
  assert.equal(h.spawns, 0);
  const release = h.runtime.beginArchive(); release();
});

test('same-process recovery keeps running shell tasks and releases tasks completed while disconnected', async (t) => {
  const h = await fixture(t);
  const shell: TaskRowDto = { task_id: 'background-shell', task_type: 'local_bash',
    description: 'Build project output', status: { type: 'running' } };
  h.internal.client.emit('event', { type: 'task_row', task: shell });
  await h.disconnect();
  h.tasks = [shell];
  await h.manager.openSession(ref);
  assert.equal(h.runtime.hasActiveWork, true);
  assert.throws(() => h.runtime.beginArchive(), /active work/i);
  await assert.rejects(h.manager.closeSession(ref), /active work/i);
  assert.equal(h.kills, 0);
  await h.disconnect();
  h.tasks = [{ ...shell, status: { type: 'completed' } }];
  await h.manager.openSession(ref);
  assert.equal(h.runtime.hasActiveWork, false);
  const release = h.runtime.beginArchive(); release();
  assert.equal(h.kills, 0);
});

test('incomplete task snapshots fail recovery and preserve task ownership until a complete retry', async (t) => {
  const h = await fixture(t);
  h.internal.client.emit('event', { type: 'task_lifecycle', event_json: JSON.stringify({
    type: 'system', subtype: 'task_started', task_id: 'background-fusion', task_type: 'local_fusion',
  }) });
  await h.disconnect();
  h.incompleteSnapshot = true;
  await assert.rejects(h.manager.openSession(ref), /incomplete runtime snapshot|snapshot is incomplete/);
  assert.equal(h.runtime.hasActiveWork, true);
  assert.equal(h.kills, 0);
  assert.equal(h.spawns, 0);
  h.incompleteSnapshot = false;
  await h.manager.openSession(ref);
  assert.equal(h.runtime.hasActiveWork, false, 'a complete empty process task registry releases offline completion');
});

for (const navigatesDuringRecovery of [false, true]) {
  test(navigatesDuringRecovery
    ? 'host restart rechecks the captured active identity after authentication'
    : 'host restart IPC lets a disconnected foreground owner reach authenticated recovery', async (t) => {
    const { CH_BRIDGE_RESTART, HostController } = await import('../src/main/host.js');
    const { SettingsStore } = await import('../src/main/settings.js');
    const { DiagnosticBuffer } = await import('../src/main/host-utils.js');
    const h = await fixture(t);
    const settings = new SettingsStore(join(h.bridgeRoot, 'settings'));
    settings.addProject(ref.projectPath); settings.setActiveSession(ref);
    const handles = new Map<string, (...args: any[]) => any>();
    const ipc = { handle: (channel: string, handler: (...args: any[]) => any) => handles.set(channel, handler), removeHandler: (channel: string) => handles.delete(channel) };
    const host = new HostController(settings, h.manager, new DiagnosticBuffer(), {} as any, ipc as any);
    h.manager.registerIpc = () => undefined;
    const window = new EventEmitter() as any;
    window.mainFrame = { url: 'app://desktop/index.html' }; window.isDestroyed = () => false; window.send = () => undefined;
    host.registerWindow(window, window.mainFrame.url);
    host.registerIpc();
    t.after(() => host.dispose());
    h.internal.client.emit('event', { type: 'turn_started', turn_id: 31 });
    h.internal.client.emit('permission', { request_id: 32, kind: { type: 'exit_plan_mode' } });
    await h.disconnect();
    if (navigatesDuringRecovery) {
      const connect = h.internal.connectBridgeClient.bind(h.internal);
      h.internal.connectBridgeClient = async (...args: any[]) => {
        await connect(...args);
        settings.setActiveSession({ ...ref, sessionId: '22222222-3333-4444-8555-666666666666' });
      };
    }
    const event = { sender: window, senderFrame: window.mainFrame };
    const recovered = handles.get(CH_BRIDGE_RESTART)!(event, ref.sessionId);
    if (navigatesDuringRecovery) await assert.rejects(recovered, /no longer active/);
    else await recovered;
    assert.equal(h.runtime.connectionState.status, 'connected');
    assert.equal(h.runtime.turnActive, false);
    assert.equal(h.runtime.pendingInteractions, 0);
    assert.equal(h.kills, 0);
    assert.equal(h.spawns, 0);
  });
}

test('same-process recovery ends a scheduled foreground whose failed promise already cleared its local latch', async (t) => {
  const h = await fixture(t);
  const events: any[] = [];
  const broadcast = h.internal.broadcastClientEvent.bind(h.internal);
  h.internal.broadcastClientEvent = (event: any) => { events.push(event); broadcast(event); };
  h.internal.credentialRoutingSettings = {};
  h.internal.scheduledModelCatalog = async () => ({ details: [{ reference: 'provider/model', reasoning: { options: [], provider_default: { type: 'automatic' } } }] });
  const work = h.runtime.runScheduledTurn('scheduled-foreground', { prompt: 'Scheduled work', automation: { model: 'provider/model', reasoning: { type: 'automatic' } } } as any);
  const rejected = assert.rejects(work, /Scheduled connection closed/);
  for (let attempt = 0; attempt < 50 && !h.internal.pendingScheduledTurns.size; attempt++) await new Promise((resolve) => setImmediate(resolve));
  assert.equal(h.internal.pendingScheduledTurns.size, 1);
  h.internal.client.emit('event', { type: 'turn_started', turn_id: 40 });
  await h.disconnect();
  await rejected;
  assert.equal(h.runtime.turnActive, false, 'existing scheduled promise cleanup has already released the latch');
  await h.manager.openSession(ref);
  assert.equal(events.filter((event) => event.type === 'turn_ended').length, 1, 'the renderer still needs the interrupted foreground terminal');
  assert.equal(h.runtime.hasActiveWork, false);
  assert.equal(h.kills, 0);
  assert.equal(h.spawns, 0);
});

for (const failing of [false, true]) {
  test(failing
    ? 'every concurrent user entrypoint observes a failed runtime snapshot recovery'
    : 'concurrent start, open and restart wait until foreground and background recovery completes', async (t) => {
    const h = await fixture(t);
    h.internal.client.emit('event', { type: 'turn_started', turn_id: 50 });
    h.internal.client.emit('event', { type: 'coordinator_status', active_workers: 1 });
    await h.disconnect();
    h.holdSnapshot = true;
    const starting = h.runtime.start();
    for (let attempt = 0; attempt < 50 && !h.pendingSnapshots; attempt++) await new Promise((resolve) => setImmediate(resolve));
    assert.equal(h.pendingSnapshots, 1);
    assert.equal(h.runtime.connectionState.status, 'connecting');
    const callers = [starting, h.runtime.start(), h.manager.ensure(ref, true), h.manager.openSession(ref), h.manager.restart(ref)];
    let settled = 0;
    for (const call of callers) void call.then(() => { settled++; }, () => { settled++; });
    const checked = failing ? Promise.all(callers.map((call) => assert.rejects(call, /fixture snapshot refusal/))) : Promise.all(callers);
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(settled, 0, 'transport handshake alone cannot complete runtime recovery');
    assert.equal(h.runtime.hasActiveAgents, true, 'unverified snapshot cannot release the previous owner');
    h.flushSnapshots(failing);
    await checked;
    assert.equal(h.kills, 0);
    assert.equal(h.spawns, 0);
    if (failing) {
      assert.equal(h.runtime.connectionState.status, 'error');
      assert.equal(h.runtime.hasActiveAgents, true);
      h.holdSnapshot = false; h.flushSnapshots(false);
      await h.manager.openSession(ref);
    }
    assert.equal(h.runtime.connectionState.status, 'connected');
    assert.equal(h.runtime.hasActiveWork, false);
  });
}

for (const outcome of ['restart', 'background', 'fresh'] as const) {
  test(`launch-only plugin material after disconnected recovery: ${outcome}`, async (t) => {
    const { CH_PLUGIN_SECRET_SET, HostController } = await import('../src/main/host.js');
    const { SettingsStore } = await import('../src/main/settings.js');
    const { DiagnosticBuffer } = await import('../src/main/host-utils.js');
    const h = await fixture(t);
    const settings = new SettingsStore(join(h.bridgeRoot, 'launch-settings'));
    settings.addProject(ref.projectPath); settings.setActiveSession(ref);
    const handles = new Map<string, (...args: any[]) => any>();
    const ipc = { handle: (channel: string, handler: (...args: any[]) => any) => handles.set(channel, handler), removeHandler: (channel: string) => handles.delete(channel) };
    let updated = false;
    const broker = { setPluginSecret: async (pluginId: string, key: string) => { updated = true; return { pluginId, key, configured: true }; } };
    const host = new HostController(settings, h.manager, new DiagnosticBuffer(), {} as any, ipc as any, undefined, broker as any);
    h.manager.registerIpc = () => undefined;
    const window = new EventEmitter() as any;
    window.mainFrame = { url: 'app://desktop/index.html' }; window.isDestroyed = () => false; window.send = () => undefined;
    host.registerWindow(window, window.mainFrame.url); host.registerIpc();
    t.after(() => host.dispose());
    const launch = h.internal.opts.launchConfig;
    h.internal.opts.launchConfig = () => ({ ...launch(), pluginSecrets: { fixture: { token: updated ? 'fake-new-material' : 'fake-old-material' } } });
    let replacements = 0;
    let stops = 0;
    h.internal.spawnServer = (config: any, directory: string) => {
      replacements++;
      assert.equal(config.pluginSecrets.fixture.token, 'fake-new-material');
      const child = fakeProcess(999_910 + replacements);
      h.writeLockfile(join(directory, `${h.port}.lock`), child.pid);
      return child;
    };
    h.internal.signalChildTree = (child: ReturnType<typeof fakeProcess>) => { stops++; child.exitCode = 0; child.emit('exit', 0, null); };
    await h.disconnect();
    if (outcome === 'background') h.activeWorkers = 1;
    if (outcome === 'fresh') { h.alive = false; h.child.exitCode = 1; }
    const result = await handles.get(CH_PLUGIN_SECRET_SET)!({ sender: window, senderFrame: window.mainFrame }, 'fixture', 'token', 'fake-value');
    assert.equal(updated, true);
    assert.equal(result.configured, true);
    if (outcome === 'background') {
      assert.equal(result.restartRequired, true);
      assert.equal(h.runtime.hasActiveAgents, true);
      assert.equal(replacements, 0);
      assert.equal(stops, 0);
    } else {
      assert.equal(result.restartRequired, undefined);
      assert.equal(replacements, 1, 'fresh configuration is applied once, without double replacement');
      assert.equal(stops, outcome === 'restart' ? 1 : 0);
    }
    assert.equal(h.runtime.connectionState.status, 'connected');
  });
}
