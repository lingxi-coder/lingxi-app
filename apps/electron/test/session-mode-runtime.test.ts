import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter, once } from 'node:events';
import { chmodSync, mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { SessionRuntime } from '../src/main/bridge.js';
import { SessionRuntimeManager } from '../src/main/sessionRuntimeManager.js';
import { SettingsStore } from '../src/main/settings.js';
import { CH_SESSION_NEW, CH_SESSION_OPEN, HostController } from '../src/main/host.js';
import { CH_SCHEDULED } from '../src/shared/scheduled.js';
import { CH_CONVERSATION_SPACE } from '../src/shared/conversationSpaces.js';
import { CH_NATIVE_AUDIO_CANCEL, CH_NATIVE_AUDIO_FINISH_LISTEN, CH_NATIVE_AUDIO_FLOW_LEASE, CH_NATIVE_AUDIO_OPERATION } from '../src/shared/nativeAudio.js';
import { DiagnosticBuffer } from '../src/main/host-utils.js';
import { validateClientCommand } from '../src/main/validation.js';

const sessionId = '11111111-2222-4333-8444-555555555555';
const targetId = '22222222-3333-4444-8555-666666666666';
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(yes => { resolve = yes; }); return { promise, resolve }; }

test('account hydration keeps an admitted prompt with its original owner after navigation', async t => {
  const accounts = deferred<any>();
  const commits: Array<{ sessionId: string; isActive: boolean }> = [];
  const manager = new SessionRuntimeManager({
    launchConfig: ref => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
    resolveProviderAccounts: () => accounts.promise,
    onFirstPromptSent: (ref, isActive) => { commits.push({ sessionId: ref.sessionId, isActive }); },
  });
  t.after(() => manager.dispose());
  const source = { projectPath: '/workspace', sessionId, mode: 'code' as const, projectId: '/workspace' };
  const target = { projectPath: '/workspace', sessionId: targetId, mode: 'chat' as const, projectId: '/workspace' };
  const runtime = await manager.ensure(source, false);
  const sent: string[] = [];
  const client = Object.assign(new EventEmitter(), {
    close() {},
    sendPrompt: (text: string) => { sent.push(text); },
    sendCommand: (command: any) => {
      if (command.type === 'provider_accounts') queueMicrotask(() => client.emit('event', { type: 'provider_accounts_updated', operation_id: command.operation_id }));
    },
  });
  const internal = runtime as any;
  internal.client = client;
  internal.activeWorkspace = source.projectPath;
  internal.activeWorkspaceTrusted = true;
  internal.credentialRoutingSettings = {};
  internal.selectedModelReference = 'openai/test-model';
  internal.setState({ status: 'connected' });
  internal.wireClient(client, internal.generation);
  manager.activateSession(source);
  const pending = runtime.sendPrompt('keep this turn');
  assert.equal(runtime.hasActiveWork, true);
  await manager.ensure(target, false);
  manager.activateSession(target);
  accounts.resolve({ providerAccounts: { storeScope: 'host', configRevision: 0, accounts: [], health: [] }, providerAccountCredentials: {} });
  await pending;
  assert.deepEqual(sent, ['keep this turn']);
  assert.deepEqual(commits, [{ sessionId, isActive: false }]);
  assert.equal(runtime.turnActive, true);
  assert.equal((manager as any).activeSessionId, targetId);
});

test('draft launch always carries its fixed owner mode', { skip: process.platform === 'win32' }, async t => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-draft-mode-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const serverBin = join(root, 'capture-arguments');
  writeFileSync(serverBin, '#!/usr/bin/env node\nprocess.stdout.write(JSON.stringify(process.argv.slice(2)));\n'); chmodSync(serverBin, 0o700);
  for (const [mode, durableAnchor] of [['chat', true], ['chat', false], ['code', false], [undefined, false]] as const) {
    const runtime = new SessionRuntime({ sessionId, projectPath: root, serverBin, launchConfig: () => ({ workspace: root, trusted: true }) }, mode, durableAnchor);
    const child = (runtime as any).spawnServer({ workspace: root, trusted: true }, join(root, 'bridge'));
    let output = ''; child.stdout.on('data', (chunk: Buffer) => { output += chunk.toString(); });
    assert.equal((await once(child, 'close'))[0], 0);
    const args = JSON.parse(output) as string[], initial = args.indexOf('--initial-conversation-mode');
    assert.equal(args.includes('--restore-draft'), mode !== undefined);
    assert.equal(args.includes('--scheduled-durable-anchor'), durableAnchor);
    if (mode) { assert.ok(initial >= 0); assert.equal(args[initial + 1], mode); } else assert.equal(initial, -1);
    await runtime.dispose();
  }
});

test('a new scheduled owner is not handed to binding until runtime readiness', async () => {
  const ready = deferred<void>();
  const originalStart = SessionRuntime.prototype.start;
  SessionRuntime.prototype.start = async function () { await ready.promise; };
  const manager = new SessionRuntimeManager({ launchConfig: ref => ({ workspace: ref.projectPath, trusted: true }) });
  let bound = false;
  try {
    const ref = { projectPath: '/workspace', sessionId, mode: 'chat' as const };
    const work = manager.withBackgroundSession(ref, true, undefined, async runtime => {
      assert.equal((runtime as any).scheduledDurableAnchor, true);
      bound = true;
    }, undefined, true);
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(bound, false);
    ready.resolve(undefined);
    await work;
    assert.equal(bound, true);
    await assert.rejects(manager.withBackgroundSession(ref, true, undefined, async () => {}, undefined, true), /already has a runtime owner/);
  } finally {
    ready.resolve(undefined);
    SessionRuntime.prototype.start = originalStart;
    await manager.dispose();
  }
});

test('draft mode admission waits for a matching context manifest', async t => {
  for (const [reportedId, reportedMode, succeeds] of [
    [sessionId, 'chat', true], [targetId, 'chat', false], [sessionId, 'code', false],
  ] as const) {
    const runtime = new SessionRuntime({ sessionId, projectPath: '/workspace',
      launchConfig: () => ({ workspace: '/workspace', trusted: true, sessionId }) }, 'chat');
    const client = new EventEmitter() as EventEmitter & { sendCommand(command: unknown): void; close(): void };
    const commands: unknown[] = [];
    client.sendCommand = command => { commands.push(command); };
    client.close = () => {};
    const internal = runtime as any;
    internal.client = client;
    internal.activeWorkspace = '/workspace';
    internal.activeWorkspaceTrusted = true;
    internal.wireClient(client, internal.generation);
    internal.setState({ status: 'connected' });
    let settled = false;
    const restoring = internal.restoreDraftSessionModeIfNeeded().finally(() => { settled = true; });
    assert.deepEqual(commands, [{ type: 'get_context_manifest' }]);
    assert.equal(runtime.remoteView().sessionMode, undefined, 'a draft mode is not exposed before admission');
    await Promise.resolve();
    assert.equal(settled, false);
    const rejected = succeeds ? undefined : assert.rejects(restoring, /does not match its owner/);
    client.emit('event', { type: 'context_manifest', manifest_json: JSON.stringify({
      session: { session_id: reportedId, mode: reportedMode }, projected: false, blocks: [],
    }) });
    if (rejected) await rejected; else await restoring;
    assert.equal(internal.draftModeRestorationRequired, !succeeds);
    assert.equal(runtime.remoteView().sessionMode, succeeds ? 'chat' : undefined);
    await runtime.dispose();
  }
});

test('a foreign session_started event cannot replace the runtime owner mode', async () => {
  const runtime = new SessionRuntime({ sessionId, projectPath: '/workspace', expectedSessionMode: 'chat',
    launchConfig: () => ({ workspace: '/workspace', trusted: true, sessionId }) });
  const client = Object.assign(new EventEmitter(), { close() {}, sendCommand() {} });
  const internal = runtime as any;
  internal.client = client;
  internal.wireClient(client, internal.generation);
  internal.setState({ status: 'connected' });
  client.emit('event', { type: 'session_started', session_id: targetId, mode: 'code' });
  assert.equal(runtime.sessionMode, 'chat');
  assert.equal(runtime.connectionState.status, 'error');
  await runtime.dispose();
});

test('scoped listing and handoff commands are bounded and session mode cannot be changed', () => {
  assert.deepEqual(validateClientCommand({ type: 'list_sessions', limit: 10, mode: 'chat' }), { type: 'list_sessions', limit: 10, mode: 'chat' });
  assert.deepEqual(validateClientCommand({ type: 'prepare_session_handoff', request_id: 'op', target_mode: 'code' }), { type: 'prepare_session_handoff', request_id: 'op', target_mode: 'code' });
  assert.deepEqual(validateClientCommand({ type: 'cancel_session_handoff', request_id: 'op' }), { type: 'cancel_session_handoff', request_id: 'op' });
  assert.throws(() => validateClientCommand({ type: 'list_sessions', mode: 'invalid' }));
  assert.throws(() => validateClientCommand({ type: 'prepare_session_handoff', request_id: 'op', target_mode: 'code', extra: true }));
  assert.throws(() => validateClientCommand({ type: 'set_session_mode', mode: 'code' }), /not allowed/);
});

test('new sessions require an explicit mode and project owner', async () => {
  const manager = new SessionRuntimeManager({ launchConfig: ref => ({ workspace: ref.projectPath, trusted: true }) });
  await assert.rejects((manager as any).newSession('/workspace'), /creation mode is unavailable/);
  await assert.rejects((manager as any).newSession('/workspace', undefined, { mode: 'chat' }), /project owner is unavailable/);
  await manager.dispose();
});

test('a background-created session starts with its explicit creation mode', async () => {
  const originalStart = SessionRuntime.prototype.start;
  SessionRuntime.prototype.start = async function () {};
  const manager = new SessionRuntimeManager({ launchConfig: ref => ({ workspace: ref.projectPath, trusted: true }) });
  try {
    const runtime = await manager.openSession({ projectPath: '/workspace', sessionId, mode: 'chat' }, true, undefined, false);
    assert.equal(runtime.sessionMode, 'chat');
    assert.equal((runtime as any).draftSessionMode, 'chat');
    await assert.rejects(manager.openSession({ projectPath: '/other', sessionId: targetId }, true), /creation mode is unavailable/);
    const selected = await manager.openSession({ projectPath: '/other', sessionId: targetId, mode: 'code' }, true,
      undefined, false, '/saved/selected.jsonl');
    assert.equal(selected.sessionMode, 'code');
    assert.equal((selected as any).draftSessionMode, undefined, 'a selected anchor uses selected-transcript launch');
  } finally {
    SessionRuntime.prototype.start = originalStart;
    await manager.dispose();
  }
});

test('handoff is correlated with its source and leaves a running turn intact, including cancellation', async () => {
  const sent: unknown[] = [];
  const client = Object.assign(new EventEmitter(), { sendCommand: (value: unknown) => sent.push(value) });
  const runtime = new SessionRuntime({ sessionId, projectPath: '/workspace', launchConfig: () => ({ workspace: '/workspace', trusted: true }) }); runtime.sessionMode = 'chat';
  Object.assign(runtime as any, { client, activeWorkspace: '/workspace', activeWorkspaceTrusted: true, activeTurn: true });
  const abort = new AbortController(), pending = runtime.prepareSessionHandoff('op', 'code', abort.signal);
  client.emit('event', { type: 'session_handoff_prepared', request_id: 'other', source_session_id: sessionId, source_mode: 'chat', target_mode: 'code', cutoff: '1', summary: 'ignored' });
  client.emit('event', { type: 'session_handoff_prepared', request_id: 'op', source_session_id: sessionId, source_mode: 'chat', target_mode: 'code', cutoff: '2', summary: 'Progress' });
  assert.equal((await pending).summary, 'Progress'); assert.equal(runtime.turnActive, true);
  const cancelled = runtime.prepareSessionHandoff('cancel', 'code', abort.signal); abort.abort();
  await assert.rejects(cancelled, /cancelled/);
  assert.deepEqual(sent.at(-1), { type: 'cancel_session_handoff', request_id: 'cancel' });
  assert.equal(runtime.turnActive, true);
  const interrupted = runtime.prepareSessionHandoff('interrupted', 'code', new AbortController().signal);
  (runtime as any).clearPendingConnectionOperations();
  await assert.rejects(interrupted, /connection interrupted/i);
});

test('handoff creates a global draft and records a durable source backlink; stale completion cannot navigate', async t => {
  for (const stale of [false, true]) {
    const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-handoff-')));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const settings = new SettingsStore(root);
    const source = { projectPath: settings.conversationSpaces.globalWorkspace, projectId: null, sessionId, mode: 'chat' as const };
    settings.conversationSpaces.register(source); settings.setActiveSession(source);
    const result = deferred<any>(); let creates = 0, released = 0, ownership: any;
    const bridge = { get: () => ({ prepareSessionHandoff: () => result.promise }), retainBackgroundSession: () => () => { released++; },
      newSession: async (path: string, _model: unknown, owner: unknown) => { creates++; ownership = owner; return { projectPath: path, sessionId: targetId }; },
      activateSession() {}, closeSession: async () => {} };
    const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), {} as any);
    (host as any).bootstrap = () => ({ activeSession: settings.getPublic().activeSession, conversationSpaces: settings.conversationSpaces.snapshot() });
    const pending = (host as any).prepareHandoff(sessionId, 'code', 'op');
    if (stale) (host as any).navigationGeneration++;
    result.resolve({ cutoff: 'persisted:42', summary: 'Completed work' });
    const output = await pending; assert.equal(released, 1);
    if (stale) { assert.equal(output, null); assert.equal(creates, 0); assert.equal(settings.conversationSpaces.snapshot().activeMode, 'chat'); continue; }
    assert.equal(output.summary, 'Completed work'); assert.equal(ownership.forceNew, true); assert.equal(ownership.projectId, null);
    assert.deepEqual(new SettingsStore(root).conversationSpaces.find({ projectPath: source.projectPath, sessionId: targetId }),
      { projectPath: source.projectPath, projectId: null, sessionId: targetId, mode: 'code', handoff: { sourceSessionId: sessionId,
        sourceProjectPath: source.projectPath, sourceMode: 'chat', cutoff: 'persisted:42', requestId: 'op' } });
  }
});

test('fresh handoff allocation preserves an existing unsent draft in the same scope', async () => {
  const manager = new SessionRuntimeManager({ launchConfig: () => ({ workspace: '/workspace', trusted: true }) });
  (manager as any).ensure = async (ref: any) => ({ sessionId: ref.sessionId, projectPath: ref.projectPath });
  (manager as any).activate = () => {};
  const first = await manager.newSession('/workspace', undefined, { mode: 'code', projectId: null });
  const second = await manager.newSession('/workspace', undefined, { mode: 'code', projectId: null, forceNew: true });
  assert.notEqual(first.sessionId, second.sessionId);
  assert.equal((await manager.newSession('/workspace', undefined, { mode: 'code', projectId: null })).sessionId, first.sessionId);
  assert.equal((await manager.reopenOwnedDraft(second)).sessionId, second.sessionId);
  await manager.dispose();
});

test('explicit retry reuses the same operation and source instead of charging for a new capture', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-handoff-retry-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const settings = new SettingsStore(root);
  const source = { projectPath: settings.conversationSpaces.globalWorkspace, projectId: null, sessionId, mode: 'chat' as const };
  settings.conversationSpaces.register(source); settings.setActiveSession(source);
  const calls: unknown[] = []; let released = 0;
  const runtime = { prepareSessionHandoff: async (id: string, target: string, _signal: AbortSignal, retry: boolean) => {
    calls.push([id, target, retry]); if (!retry) throw new Error('temporary'); return { cutoff: 'same-capture', summary: 'Progress' };
  } };
  const bridge = { get: () => runtime, retainBackgroundSession: () => () => { released++; },
    newSession: async (path: string) => ({ projectPath: path, sessionId: targetId }), activateSession() {} };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), {} as any);
  (host as any).bootstrap = () => ({ activeSession: settings.getPublic().activeSession });
  await assert.rejects((host as any).prepareHandoff(sessionId, 'code', 'same-op'), /temporary/);
  assert.equal(released, 0, 'retry capture must keep its source runtime alive');
  await assert.rejects((host as any).prepareHandoff(sessionId, 'code', 'same-op'), /Use retry/);
  const output = await (host as any).prepareHandoff(sessionId, 'code', 'same-op', true);
  assert.equal(output.summary, 'Progress');
  assert.equal(released, 1);
  assert.deepEqual(calls, [['same-op', 'code', false], ['same-op', 'code', true]]);
  assert.equal(settings.conversationSpaces.find({ projectPath: source.projectPath, sessionId: targetId })?.handoff?.cutoff, 'same-capture');
});

test('navigation while a target draft is starting cannot activate the late draft', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-handoff-start-race-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const settings = new SettingsStore(root);
  const source = { projectPath: settings.conversationSpaces.globalWorkspace, projectId: null, sessionId, mode: 'chat' as const };
  settings.conversationSpaces.register(source); settings.setActiveSession(source);
  const started = deferred<any>(), closed: unknown[] = [], activated: unknown[] = [];
  const bridge = { get: () => ({ prepareSessionHandoff: async () => ({ cutoff: '1', summary: 'Progress' }) }),
    retainBackgroundSession: () => () => {}, newSession: async () => started.promise,
    closeSession: async (ref: unknown) => closed.push(ref), activateSession: (ref: unknown) => activated.push(ref) };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), {} as any);
  const pending = (host as any).prepareHandoff(sessionId, 'code', 'op');
  await new Promise<void>(resolve => setImmediate(resolve));
  (host as any).navigationGeneration++;
  const target = { projectPath: source.projectPath, sessionId: targetId };
  started.resolve(target);
  assert.equal(await pending, null); assert.deepEqual(closed, [target]); assert.deepEqual(activated, []);
  assert.equal(settings.conversationSpaces.snapshot().activeMode, 'chat');
});

test('switching spaces keeps the source turn, listener and pending permission owned by its runtime', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-space-switch-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const settings = new SettingsStore(root), path = settings.conversationSpaces.globalWorkspace;
  const source = { projectPath: path, projectId: null, sessionId, mode: 'chat' as const };
  const target = { projectPath: path, projectId: null, sessionId: targetId, mode: 'code' as const };
  settings.conversationSpaces.register(source); settings.conversationSpaces.register(target); settings.setActiveSession(source);
  const sourceRuntime = new EventEmitter(); let events = 0, closes = 0, cancels = 0;
  sourceRuntime.on('permission', () => { events++; });
  Object.assign(sourceRuntime, { activeTurn: true, pendingPermission: 7, cancel: () => { cancels++; } });
  const bridge = { reopenOwnedDraft: async () => target, activateSession() {}, closeSession: () => { closes++; } };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), {} as any);
  (host as any).bootstrap = () => ({ activeSession: settings.getPublic().activeSession });
  (host as any).draftSelections.set('code', target);
  settings.conversationSpaces.setMode('code');
  await (host as any).switchConversationSpace('code', 0);
  sourceRuntime.emit('permission');
  assert.equal(events, 1); assert.equal((sourceRuntime as any).pendingPermission, 7);
  assert.equal((sourceRuntime as any).activeTurn, true); assert.equal(closes, 0); assert.equal(cancels, 0);
  assert.equal(settings.getPublic().activeSession?.sessionId, targetId);
});

test('New Chat remains global when a project is selected', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-global-new-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const project = join(root, 'project'); mkdirSync(project);
  const settings = new SettingsStore(join(root, 'settings'));
  settings.addProject(project); settings.conversationSpaces.addProject(project, 'chat');
  settings.activateProject(project); settings.conversationSpaces.setNavigation('chat', project);
  let created: any;
  const bridge = { registerWindow() {}, registerIpc() {}, newSession: async (path: string, _model: unknown, owner: any) => {
    created = { path, owner }; return { projectPath: path, sessionId: targetId };
  }, activateSession() {} };
  const handlers = new Map<string, (...args: any[]) => any>();
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), {} as any,
    { handle: (name: string, handler: any) => handlers.set(name, handler), removeHandler() {} } as any);
  (host as any).bootstrap = () => ({ activeSession: settings.getPublic().activeSession });
  const frame = { url: 'http://127.0.0.1:4242' };
  const sender = { mainFrame: frame, isDestroyed: () => false, once() {}, send() {} };
  host.registerWindow(sender as any, frame.url); host.registerIpc();
  await handlers.get(CH_SESSION_NEW)!({ sender, senderFrame: frame }, null, undefined);
  assert.equal(created.path, settings.conversationSpaces.globalWorkspace);
  assert.equal(created.owner.projectId, null); assert.equal(created.owner.mode, 'chat');
  assert.equal(settings.getPublic().activeSession?.projectId, null);
});

test('a linked session checks its fixed mode before switching spaces and rolls back failed opens', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-linked-mode-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const settings = new SettingsStore(root), path = settings.conversationSpaces.globalWorkspace;
  settings.conversationSpaces.register({ projectPath: path, projectId: null, sessionId: targetId, mode: 'code' });
  const handlers = new Map<string, (...args: any[]) => any>();
  const bridge = { registerWindow() {}, registerIpc() {} };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), {} as any,
    { handle: (name: string, handler: any) => handlers.set(name, handler), removeHandler() {} } as any);
  const frame = { url: 'http://127.0.0.1:4242' }, sender = { mainFrame: frame, isDestroyed: () => false, once() {}, send() {} };
  host.registerWindow(sender as any, frame.url); host.registerIpc();
  const event = { sender, senderFrame: frame };
  let opens = 0;
  (host as any).openSessionAndActivateInternal = async (_ref: unknown, _selection: unknown, _generation: unknown, mode: unknown) => {
    opens++; assert.equal(mode, 'code'); throw new Error('open failed');
  };
  await assert.rejects(handlers.get(CH_SESSION_OPEN)!(event, path, targetId, undefined, 'chat'), /different conversation space/);
  assert.equal(settings.conversationSpaces.snapshot().activeMode, 'chat'); assert.equal(opens, 0);
  await assert.rejects(handlers.get(CH_SESSION_OPEN)!(event, path, targetId, undefined, 'code'), /open failed/);
  assert.equal(settings.conversationSpaces.snapshot().activeMode, 'chat'); assert.equal(opens, 1);
});

test('voice flow retains its owner and provider context across navigation and rejects another lane', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-audio-flow-owner-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const settings = new SettingsStore(root), path = settings.conversationSpaces.globalWorkspace;
  const source = { projectPath: path, projectId: null, sessionId, mode: 'chat' as const };
  const target = { projectPath: path, projectId: null, sessionId: targetId, mode: 'code' as const };
  settings.conversationSpaces.register(source); settings.conversationSpaces.register(target); settings.setActiveSession(source);
  const handlers = new Map<string, (...args: any[]) => any>(), released: string[] = [], contexts: unknown[] = [];
  let cancels = 0, finishes = 0;
  const bridge = { registerWindow() {}, registerIpc() {}, get: (id: string) => ({ projectPath: path, sessionId: id,
    audioSessionContext: { sessionId: id, profileId: `profile-${id}` } }),
    retainBackgroundSession: (ref: { sessionId: string }) => () => released.push(ref.sessionId) };
  const audio = { onEvent: () => () => {}, executeUiAudioOperation: async (_operation: unknown, _window: unknown, _revision: unknown, _override: unknown, context: unknown) => { contexts.push(context); return {}; },
    cancelUiAudioOperations: async () => { cancels++; }, finishUiAudioListen: async () => { finishes++; } };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), {} as any,
    { handle: (name: string, handler: any) => handlers.set(name, handler), removeHandler() {} } as any, undefined, undefined, audio as any);
  const frame = { url: 'http://127.0.0.1:4242' }, sender = { id: 7, mainFrame: frame, isDestroyed: () => false, once() {}, send() {} };
  host.registerWindow(sender as any, frame.url); host.registerIpc();
  const event = { sender, senderFrame: frame }, lease = handlers.get(CH_NATIVE_AUDIO_FLOW_LEASE)!;
  const first = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', second = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb';
  lease(event, sessionId, first, true);
  settings.setActiveSession(target);
  const execute = handlers.get(CH_NATIVE_AUDIO_OPERATION)!, cancel = handlers.get(CH_NATIVE_AUDIO_CANCEL)!, finish = handlers.get(CH_NATIVE_AUDIO_FINISH_LISTEN)!;
  const owner = { sessionId, leaseId: first }, other = { sessionId: targetId, leaseId: second };
  await assert.rejects(execute(event, { type: 'speak', text: 'other' }), /another conversation/);
  await assert.rejects(execute(event, { type: 'speak', text: 'other' }, undefined, undefined, other), /another conversation/);
  await assert.rejects(cancel(event), /another conversation/);
  await assert.rejects(finish(event, other), /another conversation/);
  await execute(event, { type: 'speak', text: 'original' }, undefined, undefined, owner);
  await cancel(event, owner); await finish(event, owner);
  assert.deepEqual(contexts, [{ session: { sessionId, profileId: `profile-${sessionId}` } }]);
  assert.equal(cancels, 1); assert.equal(finishes, 1);
  lease(event, sessionId, second, false);
  assert.deepEqual(released, []);
  assert.throws(() => lease(event, targetId, second, true), /another conversation/);
  lease(event, sessionId, first, false);
  lease(event, targetId, second, true);
  lease(event, sessionId, first, false);
  assert.deepEqual(released, [sessionId]);
  lease(event, targetId, second, false);
  assert.deepEqual(released, [sessionId, targetId]);
  await execute(event, { type: 'speak', text: 'current' });
  assert.deepEqual(contexts[1], { session: { sessionId: targetId, profileId: `profile-${targetId}` } });
});

test('a failed latest switch restores the confirmed space after rapid opposing choices', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-space-failure-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const settings = new SettingsStore(root), path = settings.conversationSpaces.globalWorkspace;
  const source = { projectPath: path, projectId: null, sessionId, mode: 'chat' as const };
  settings.conversationSpaces.register(source); settings.setActiveSession(source);
  const first = deferred<void>(), handlers = new Map<string, (...args: any[]) => any>();
  const bridge = { registerWindow() {}, registerIpc() {} };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), {} as any,
    { handle: (name: string, handler: any) => handlers.set(name, handler), removeHandler() {} } as any);
  (host as any).bootstrap = () => ({});
  (host as any).switchConversationSpace = async (mode: string) => {
    if (mode === 'code') { await first.promise; return {}; }
    throw new Error('target unavailable');
  };
  const frame = { url: 'http://127.0.0.1:4242' }, sender = { mainFrame: frame, isDestroyed: () => false, once() {}, send() {} };
  host.registerWindow(sender as any, frame.url); host.registerIpc();
  const event = { sender, senderFrame: frame };
  const code = handlers.get(CH_CONVERSATION_SPACE)!(event, 'code');
  const chat = handlers.get(CH_CONVERSATION_SPACE)!(event, 'chat');
  const failure = assert.rejects(chat, /target unavailable/);
  first.resolve(); await code; await failure;
  assert.equal(settings.conversationSpaces.snapshot().activeMode, 'chat');
  assert.equal(settings.getPublic().activeSession?.sessionId, sessionId);
});

test('failed captures keep one bounded lease until retry is discarded', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-retry-lease-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const settings = new SettingsStore(root), path = settings.conversationSpaces.globalWorkspace;
  const source = { projectPath: path, projectId: null, sessionId, mode: 'chat' as const };
  settings.conversationSpaces.register(source); settings.setActiveSession(source);
  let released = 0;
  const bridge = { get: () => ({ prepareSessionHandoff: async () => { throw new Error('model failed'); } }),
    retainBackgroundSession: () => () => { released++; } };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), {} as any);
  await assert.rejects((host as any).prepareHandoff(sessionId, 'code', 'op'), /model failed/);
  const retained = (host as any).handoffRetries.get('op');
  assert.ok(retained.expires > Date.now() && retained.expires <= Date.now() + 5 * 60_000);
  assert.equal(released, 0);
  (host as any).discardHandoff('op'); (host as any).discardHandoff('op');
  assert.equal(released, 1);
});


test('a slow scheduled history open cannot reactivate its owner after a newer space choice', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-scheduled-navigation-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const settings = new SettingsStore(root), path = settings.conversationSpaces.globalWorkspace;
  const source = { projectPath: path, projectId: null, sessionId, mode: 'chat' as const };
  const target = { ...source, sessionId: targetId };
  settings.conversationSpaces.register(source); settings.conversationSpaces.register(target); settings.setActiveSession(source);
  const ready = deferred<void>(), entered = deferred<void>(), handlers = new Map<string, (...args: any[]) => any>();
  const activated: unknown[] = [];
  const bridge = { registerWindow() {}, registerIpc() {}, isProjectClosing: () => false, get: (id: string) => id === targetId ? { projectPath: path, sessionMode: 'chat' } : undefined,
    openSession: async (_ref: unknown, _empty: unknown, _model: unknown, activate: boolean) => {
      assert.equal(activate, false, 'a loading history must not activate before its navigation generation is checked');
      entered.resolve(); await ready.promise;
    }, activateSession: (ref: unknown) => activated.push(ref) };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), {} as any,
    { handle: (name: string, handler: any) => handlers.set(name, handler), removeHandler() {} } as any);
  host.attachScheduled({ resolveSession: () => target } as any);
  (host as any).bootstrap = () => ({ conversationSpaces: settings.conversationSpaces.snapshot() });
  (host as any).trustSessionSource = async () => {};
  (host as any).switchConversationSpace = async () => (host as any).bootstrap();
  const frame = { url: 'http://127.0.0.1:4242' }, sender = { mainFrame: frame, isDestroyed: () => false, once() {}, send() {} };
  host.registerWindow(sender as any, frame.url); host.registerIpc();
  const event = { sender, senderFrame: frame };
  const opening = handlers.get(CH_SCHEDULED)!(event, { action: 'open', scopeId: 'global', sessionId: targetId });
  await entered.promise;
  const switching = handlers.get(CH_CONVERSATION_SPACE)!(event, 'code');
  ready.resolve(); await opening; await switching;
  assert.equal(settings.conversationSpaces.snapshot().activeMode, 'code');
  assert.equal(settings.getPublic().activeSession?.sessionId, sessionId);
  assert.deepEqual(activated, []);
});

test('a queued scheduled Chat open superseded by New Chat preserves the visible Code mode', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-scheduled-queued-mode-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const settings = new SettingsStore(root), path = settings.conversationSpaces.globalWorkspace;
  settings.conversationSpaces.setMode('code');
  const target = { projectPath: path, projectId: null, sessionId, mode: 'chat' as const };
  settings.conversationSpaces.register(target);
  const handlers = new Map<string, (...args: any[]) => any>();
  let created: any;
  const bridge = { registerWindow() {}, registerIpc() {}, newSession: async (directory: string, _model: unknown, owner: any) => {
    created = owner; return { projectPath: directory, sessionId: targetId };
  }, activateSession() {} };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), {} as any,
    { handle: (name: string, handler: any) => handlers.set(name, handler), removeHandler() {} } as any);
  host.attachScheduled({ resolveSession: () => target } as any);
  (host as any).bootstrap = () => ({ conversationSpaces: settings.conversationSpaces.snapshot() });
  const hold = deferred<void>(), entered = deferred<void>();
  const blocked = (host as any).enqueueNavigation(async () => { entered.resolve(); await hold.promise; });
  await entered.promise;
  const frame = { url: 'http://127.0.0.1:4242' }, sender = { mainFrame: frame, isDestroyed: () => false, once() {}, send() {} };
  host.registerWindow(sender as any, frame.url); host.registerIpc();
  const event = { sender, senderFrame: frame };
  const opening = handlers.get(CH_SCHEDULED)!(event, { action: 'open', scopeId: 'global', sessionId });
  assert.equal(settings.conversationSpaces.snapshot().activeMode, 'code');
  const creating = handlers.get(CH_SESSION_NEW)!(event, null, undefined);
  hold.resolve(); await blocked; await opening; await creating;
  assert.equal(created.mode, 'code'); assert.equal(created.projectId, null);
  assert.equal(settings.conversationSpaces.snapshot().activeMode, 'code');
});

for (const afterStart of [false, true]) {
  test(`a scheduled Bot open superseded ${afterStart ? 'while starting' : 'while queued'} cannot activate`, async t => {
    const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-scheduled-bot-')));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const settings = new SettingsStore(root);
    const handlers = new Map<string, (...args: any[]) => any>();
    const bridge = { registerWindow() {}, registerIpc() {} };
    const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), {} as any,
      { handle: (name: string, handler: any) => handlers.set(name, handler), removeHandler() {} } as any);
    const target = { projectPath: '/bots/bot', sessionId: targetId, botId: 'bot' };
    const entered = deferred<void>(), ready = deferred<void>();
    let starts = 0, activations = 0;
    (host as any).botsUi = { handlers: () => [], botIdFor: () => 'bot', openSession: async (_bot: unknown, _id: unknown, _path: unknown, activate: boolean) => { assert.equal(activate, false); starts++; entered.resolve(); await ready.promise; return target; } };
    (host as any).showBotSession = () => { activations++; };
    (host as any).bootstrap = () => ({});
    host.attachScheduled({ resolveSession: () => target } as any);
    const frame = { url: 'http://127.0.0.1:4242' }, sender = { mainFrame: frame, isDestroyed: () => false, once() {}, send() {} };
    host.registerWindow(sender as any, frame.url); host.registerIpc();
    let blocker: Promise<void> | undefined;
    if (!afterStart) blocker = (host as any).enqueueNavigation(() => ready.promise);
    const opening = handlers.get(CH_SCHEDULED)!({ sender, senderFrame: frame }, { action: 'open', scopeId: '/bots/bot', sessionId: targetId });
    if (afterStart) await entered.promise;
    (host as any).navigationGeneration++;
    ready.resolve(); await blocker; await opening;
    assert.equal(starts, afterStart ? 1 : 0);
    assert.equal(activations, 0);
  });
}
