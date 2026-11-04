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
import { AuthoringSessionIntents } from '../src/main/local-apps/authoringSessionIntents.js';

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

test('Cron and failed Fusion do not complete a pending Local App draft before an admitted user prompt', async () => {
  const originalStart = SessionRuntime.prototype.start;
  const admissions: string[] = [];
  SessionRuntime.prototype.start = async function () {
    (this as any).activeWorkspace = '/workspace'; (this as any).activeWorkspaceTrusted = true;
    (this as any).state = { status: 'connected' }; (this as any).draftModeRestorationRequired = false;
  };
  const manager = new SessionRuntimeManager({ launchConfig: ref => ({ workspace: ref.projectPath, trusted: true }),
    onPromptAdmitted: ref => admissions.push(ref.sessionId) });
  try {
    const ref = await manager.newSession('/workspace', undefined, { mode: 'code', projectId: '/workspace' });
    const runtime = manager.require(ref);
    const client = new EventEmitter() as EventEmitter & { sendCommand(command: unknown): void; sendPrompt(prompt: string): void };
    let rejectPrompt = false;
    client.sendCommand = () => {};
    client.sendPrompt = () => { if (rejectPrompt) throw new Error('bridge failed'); };
    (runtime as any).client = client; (runtime as any).wireClient(client, 0);
    const cron = runtime.dispatchCommand({ type: 'cron_manage', request_id: 'created', request: { action: 'create', cron: '0 9 * * *', prompt: 'Check' } });
    client.emit('event', { type: 'cron_result', request_id: 'created', jobs: [] }); await cron;
    assert.deepEqual(admissions, []);
    (runtime as any).sessionIdentityCommitted = false;
    (runtime as any).recordFusionPreparationFailure({ type: 'run_slash_command', raw: '/fusion review', images: [] }, new Error('prepare failed'));
    assert.deepEqual(admissions, []);
    runtime.sendPrompt('scheduled', [], undefined, undefined, { origin: 'scheduled' });
    assert.deepEqual(admissions, []); (runtime as any).activeTurn = false;
    rejectPrompt = true; assert.throws(() => runtime.sendPrompt('not admitted'), /bridge failed/);
    assert.deepEqual(admissions, []); rejectPrompt = false;
    runtime.sendPrompt('user prompt');
    assert.deepEqual(admissions, [ref.sessionId]);
  } finally { SessionRuntime.prototype.start = originalStart; await manager.dispose(); }
});

test('draft launch always carries its fixed owner mode', { skip: process.platform === 'win32' }, async t => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-draft-mode-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const serverBin = join(root, 'capture-arguments');
  writeFileSync(serverBin, '#!/usr/bin/env node\nprocess.stdout.write(JSON.stringify(process.argv.slice(2)));\n'); chmodSync(serverBin, 0o700);
  for (const [mode, durableAnchor] of [['chat', 'Scheduled task'], ['chat', undefined], ['code', 'Reading garden'], [undefined, undefined]] as const) {
    const runtime = new SessionRuntime({ sessionId, projectPath: root, serverBin, launchConfig: () => ({ workspace: root, trusted: true }) }, mode, durableAnchor);
    const child = (runtime as any).spawnServer({ workspace: root, trusted: true }, join(root, 'bridge'));
    let output = ''; child.stdout.on('data', (chunk: Buffer) => { output += chunk.toString(); });
    assert.equal((await once(child, 'close'))[0], 0);
    const args = JSON.parse(output) as string[], initial = args.indexOf('--initial-conversation-mode');
    assert.equal(args.includes('--restore-draft'), mode !== undefined);
    assert.equal(args.includes('--durable-draft-title'), durableAnchor !== undefined);
    if (durableAnchor) assert.equal(args[args.indexOf('--durable-draft-title') + 1], durableAnchor);
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
      assert.equal((runtime as any).durableDraftTitle, 'Scheduled task');
      bound = true;
    }, undefined, 'Scheduled task');
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(bound, false);
    ready.resolve(undefined);
    await work;
    assert.equal(bound, true);
    await assert.rejects(manager.withBackgroundSession(ref, true, undefined, async () => {}, undefined, 'Scheduled task'), /already has a runtime owner/);
  } finally {
    ready.resolve(undefined);
    SessionRuntime.prototype.start = originalStart;
    await manager.dispose();
  }
});

test('a durable Local App draft persists its allocated owner before launching the runtime', async () => {
  const gate = deferred<void>(); const events: string[] = [];
  const originalStart = SessionRuntime.prototype.start;
  SessionRuntime.prototype.start = async function () { events.push('start'); assert.equal((this as any).durableDraftTitle, 'Local App'); };
  const manager = new SessionRuntimeManager({ launchConfig: ref => ({ workspace: ref.projectPath, trusted: true }) });
  try {
    const work = manager.newSession('/workspace', undefined, { mode: 'code', projectId: '/workspace', forceNew: true,
      durableDraftTitle: 'Local App', onAllocated: async ref => { assert.ok(ref.sessionId); events.push('allocated'); await gate.promise; events.push('persisted'); } });
    await new Promise(resolve => setImmediate(resolve));
    assert.deepEqual(events, ['allocated']);
    gate.resolve(undefined); await work;
    assert.deepEqual(events, ['allocated', 'persisted', 'start']);
  } finally { gate.resolve(undefined); SessionRuntime.prototype.start = originalStart; await manager.dispose(); }
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
    const source = { projectPath: settings.conversationSpaces.globalWorkspaces.chat, projectId: null, sessionId, mode: 'chat' as const };
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
    const targetPath = settings.conversationSpaces.globalWorkspaces.code;
    assert.deepEqual(new SettingsStore(root).conversationSpaces.find({ projectPath: targetPath, sessionId: targetId }),
      { projectPath: targetPath, projectId: null, sessionId: targetId, mode: 'code', handoff: { sourceSessionId: sessionId,
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
  const source = { projectPath: settings.conversationSpaces.globalWorkspaces.chat, projectId: null, sessionId, mode: 'chat' as const };
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
  assert.equal(settings.conversationSpaces.find({ projectPath: settings.conversationSpaces.globalWorkspaces.code, sessionId: targetId })?.handoff?.cutoff, 'same-capture');
});

test('navigation while a target draft is starting cannot activate the late draft', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-handoff-start-race-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const settings = new SettingsStore(root);
  const source = { projectPath: settings.conversationSpaces.globalWorkspaces.chat, projectId: null, sessionId, mode: 'chat' as const };
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
  const settings = new SettingsStore(root), path = settings.conversationSpaces.globalWorkspaces.chat;
  const source = { projectPath: path, projectId: null, sessionId, mode: 'chat' as const };
  const target = { projectPath: settings.conversationSpaces.globalWorkspaces.code, projectId: null, sessionId: targetId, mode: 'code' as const };
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
  assert.equal(created.path, settings.conversationSpaces.globalWorkspaces.chat);
  assert.equal(created.owner.projectId, null); assert.equal(created.owner.mode, 'chat');
  assert.equal(settings.getPublic().activeSession?.projectId, null);
});

test('Global New Chat chooses the fixed mode directory without inheriting the selected project', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-global-mode-directories-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const project = join(root, 'project'); mkdirSync(project);
  const settings = new SettingsStore(join(root, 'settings'));
  settings.addProject(project);
  for (const mode of ['chat', 'code'] as const) settings.conversationSpaces.addProject(project, mode);
  const created: Array<{ path: string; owner: any }> = [];
  const bridge = { registerWindow() {}, registerIpc() {}, newSession: async (path: string, _model: unknown, owner: any) => {
    created.push({ path, owner }); return { projectPath: path, sessionId: owner.mode === 'chat' ? sessionId : targetId };
  }, activateSession() {} };
  const handlers = new Map<string, (...args: any[]) => any>();
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), {} as any,
    { handle: (name: string, handler: any) => handlers.set(name, handler), removeHandler() {} } as any);
  (host as any).bootstrap = () => ({ activeSession: settings.getPublic().activeSession });
  const frame = { url: 'http://127.0.0.1:4242' };
  const sender = { mainFrame: frame, isDestroyed: () => false, once() {}, send() {} };
  host.registerWindow(sender as any, frame.url); host.registerIpc();
  for (const mode of ['chat', 'code'] as const) {
    settings.conversationSpaces.setMode(mode);
    settings.conversationSpaces.setNavigation(mode, project);
    settings.activateProject(project);
    await handlers.get(CH_SESSION_NEW)!({ sender, senderFrame: frame }, null, undefined);
    assert.equal(created.at(-1)?.path, settings.conversationSpaces.globalWorkspaces[mode]);
    assert.equal(created.at(-1)?.owner.projectId, null);
    assert.equal(created.at(-1)?.owner.mode, mode);
    assert.equal(settings.getPublic().activeSession?.projectId, null);
  }
  assert.notEqual(created[0]?.path, created[1]?.path);
});

test('each explicit Local App creation allocates a separate Code Project draft', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-local-app-new-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const project = join(root, 'project'); mkdirSync(project);
  const settings = new SettingsStore(join(root, 'settings'));
  settings.addProject(project); settings.conversationSpaces.addProject(project, 'code'); settings.conversationSpaces.setMode('code');
  const owners: any[] = [];
  const bridge = { registerWindow() {}, registerIpc() {}, newSession: async (path: string, _model: unknown, owner: any) => {
    owners.push(owner); const ref = { projectPath: path, sessionId: owners.length === 1 ? sessionId : targetId };
    await owner.onAllocated(ref); return ref;
  }, retainBackgroundSession: () => () => {}, get: () => ({}), activateSession() {} };
  const handlers = new Map<string, (...args: any[]) => any>();
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), {} as any,
    { handle: (name: string, handler: any) => handlers.set(name, handler), removeHandler() {} } as any);
  host.attachLocalAppSessions({ begin: () => ({ allocate: async () => {}, recordSource: async () => {}, finish: async () => {} }), get: async () => { throw new Error('unused'); }, recordSource: async () => { throw new Error('unused'); }, finish: async () => {} });
  (host as any).rememberCreatedSessionSource = async (ref: { projectPath: string; sessionId: string }, _runtime: unknown, _scheduled: boolean, capture: (source: unknown) => Promise<void>) => {
    const path = join(root, `${ref.sessionId}.jsonl`); await capture({ scope: 'current', transcriptPath: path });
    settings.rememberSessionTranscriptSelection(ref, { catalogProjectPath: project, scope: 'current', transcriptPath: path }); return path;
  };
  (host as any).assertLocalAppSessionRuntime = async () => {};
  (host as any).bootstrap = () => ({ activeSession: settings.getPublic().activeSession });
  const frame = { url: 'http://127.0.0.1:4242' }, sender = { mainFrame: frame, isDestroyed: () => false, once() {}, send() {} };
  host.registerWindow(sender as any, frame.url); host.registerIpc();
  const event = { sender, senderFrame: frame };
  const purpose = { kind: 'local-app', intent: { name: 'App', setCurrent: false } };
  await handlers.get(CH_SESSION_NEW)!(event, project, undefined, purpose);
  await handlers.get(CH_SESSION_NEW)!(event, project, undefined, purpose);
  assert.equal(owners.length, 2);
  assert.ok(owners.every(owner => owner.forceNew === true && owner.mode === 'code' && owner.projectId === project));
  assert.ok(owners.every(owner => owner.durableDraftTitle === 'App'));
  await assert.rejects(handlers.get(CH_SESSION_NEW)!(event, null, undefined, purpose), /Code Project/);
  settings.conversationSpaces.setMode('chat');
  await assert.rejects(handlers.get(CH_SESSION_NEW)!(event, project, undefined, purpose), /local_app_development_requires_code|Code/);
  await assert.rejects(handlers.get(CH_SESSION_OPEN)!(event, project, targetId, undefined, 'code', `a${'1'.repeat(32)}`, 0), /local_app_development_requires_code|Code/);
  assert.equal(owners.length, 2);
  assert.equal(settings.conversationSpaces.snapshot().activeMode, 'chat');
});

test('an admitted Local App Code session is anchored and registered after Chat navigation without reclaiming the display', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-local-app-park-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const project = join(root, 'project'); mkdirSync(project);
  const settings = new SettingsStore(join(root, 'settings'));
  settings.addProject(project); settings.conversationSpaces.addProject(project, 'code'); settings.conversationSpaces.setMode('code');
  const ledger = new AuthoringSessionIntents(join(root, 'intents'));
  const entered = deferred<void>(), ready = deferred<void>();
  const finished: string[] = [], activated: string[] = [];
  const bridge = { registerWindow() {}, registerIpc() {}, retainBackgroundSession: () => () => {}, get: () => ({}),
    activateSession: (ref: { sessionId: string }) => activated.push(ref.sessionId),
    async newSession(path: string, _model: unknown, owner: any) {
      const ref = { projectPath: path, sessionId, projectId: path, mode: 'code' as const };
      assert.equal(owner.durableDraftTitle, 'Reading garden');
      await owner.onAllocated(ref); entered.resolve(undefined); await ready.promise;
      assert.equal(owner.activate(), false);
      return ref;
    } };
  const handlers = new Map<string, (...args: any[]) => any>();
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), {} as any,
    { handle: (name: string, handler: any) => handlers.set(name, handler), removeHandler() {} } as any);
  host.attachLocalAppSessions({ begin: input => {
    let id = '';
    return { allocate: async ref => { id = (await ledger.create(ref, input)).requestId; }, recordSource: async source => { await ledger.recordSource(id, source); },
      finish: async ref => { finished.push(ref.sessionId); await ledger.ready(id); } };
  }, get: async id => { const saved = await ledger.get(id); if (!saved) throw new Error('missing'); return saved; },
  recordSource: (id, source) => ledger.recordSource(id, source), finish: async () => {} });
  (host as any).rememberCreatedSessionSource = async (ref: { projectPath: string; sessionId: string }, _runtime: unknown, _scheduled: boolean, capture: (source: unknown) => Promise<void>) => {
    assert.equal(ref.sessionId, sessionId); const path = join(root, `${ref.sessionId}.jsonl`);
    await capture({ scope: 'current', transcriptPath: path });
    settings.rememberSessionTranscriptSelection(ref, { catalogProjectPath: project, scope: 'current', transcriptPath: path }); return path;
  };
  (host as any).assertLocalAppSessionRuntime = async () => {};
  (host as any).bootstrap = () => ({ settings: settings.getPublic() });
  (host as any).switchConversationSpace = async () => ({ settings: settings.getPublic() });
  const frame = { url: 'http://127.0.0.1:4242' }, sender = { mainFrame: frame, isDestroyed: () => false, once() {}, send() {} };
  host.registerWindow(sender as any, frame.url); host.registerIpc();
  const event = { sender, senderFrame: frame };
  const creating = handlers.get(CH_SESSION_NEW)!(event, project, undefined, { kind: 'local-app', intent: { name: 'Reading garden', setCurrent: false } });
  await entered.promise;
  const [before] = await ledger.pending();
  assert.equal(before?.sessionId, sessionId); assert.equal(before?.state, 'pending');
  const switching = handlers.get(CH_CONVERSATION_SPACE)!(event, 'chat');
  assert.equal(settings.conversationSpaces.snapshot().activeMode, 'chat');
  ready.resolve(undefined); await Promise.all([creating, switching]);
  assert.deepEqual(finished, [sessionId]); assert.deepEqual(activated, []);
  assert.equal(settings.conversationSpaces.findBySessionId(sessionId)?.mode, 'code');
  assert.equal((host as any).draftSelections.get('code'), undefined, 'a parked operation cannot become automatic navigation');
  const [reloaded] = await new AuthoringSessionIntents(join(root, 'intents')).pending();
  assert.equal(reloaded?.requestId, before?.requestId); assert.equal(reloaded?.state, 'ready');
  await assert.rejects(handlers.get(CH_SESSION_NEW)!(event, null, undefined,
    { kind: 'recover-local-app', requestId: before!.requestId }), /local_app_development_requires_code|Code/);
});

test('a Local App source changing while registration waits is never activated', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-local-app-cwd-race-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const project = join(root, 'project'), other = join(root, 'other'); mkdirSync(project); mkdirSync(other);
  const path = join(root, `${sessionId}.jsonl`), entered = deferred<void>(), finish = deferred<void>();
  const settings = new SettingsStore(join(root, 'settings'));
  settings.addProject(project); settings.conversationSpaces.addProject(project, 'code'); settings.conversationSpaces.setMode('code');
  let cwd = project, activated = false;
  const runtime = { projectPath: project, sessionId, sessionMode: 'code', getWorkspaceState: async () => ({ cwd, transcript_path: path }) };
  const bridge = { registerWindow() {}, registerIpc() {}, retainBackgroundSession: () => () => {}, get: () => runtime, isProjectClosing: () => false,
    activateSession: () => { activated = true; }, newSession: async (_path: string, _model: unknown, owner: any) => {
      const ref = { projectPath: project, projectId: project, sessionId, mode: 'code' }; await owner.onAllocated(ref); return ref;
    } };
  const handlers = new Map<string, (...args: any[]) => any>();
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), {} as any,
    { handle: (name: string, handler: any) => handlers.set(name, handler), removeHandler() {} } as any);
  host.attachLocalAppSessions({ begin: () => ({ allocate: async () => {}, recordSource: async () => {},
    finish: async () => { entered.resolve(undefined); await finish.promise; } }), get: async () => { throw new Error('unused'); },
    recordSource: async () => { throw new Error('unused'); }, finish: async () => {} });
  (host as any).rememberCreatedSessionSource = async (ref: { projectPath: string; sessionId: string }, _runtime: unknown, _scheduled: boolean, capture: (source: unknown) => Promise<void>) => {
    await capture({ scope: 'current', transcriptPath: path });
    settings.rememberSessionTranscriptSelection(ref, { catalogProjectPath: project, scope: 'current', transcriptPath: path }); return path;
  };
  const frame = { url: 'http://127.0.0.1:4242' }, sender = { mainFrame: frame, isDestroyed: () => false, once() {}, send() {} };
  host.registerWindow(sender as any, frame.url); host.registerIpc();
  const creating = handlers.get(CH_SESSION_NEW)!({ sender, senderFrame: frame }, project, undefined,
    { kind: 'local-app', intent: { name: 'App', setCurrent: false } });
  await entered.promise; cwd = other; finish.resolve(undefined);
  await assert.rejects(creating, /source changed/); assert.equal(activated, false);
});

test('manual Code recovery reopens the original pending UUID and keeps its saved source', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-local-app-recover-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const project = join(root, 'project'); mkdirSync(project);
  const settings = new SettingsStore(join(root, 'settings'));
  settings.addProject(project); settings.conversationSpaces.addProject(project, 'code'); settings.conversationSpaces.setMode('code');
  const ledger = new AuthoringSessionIntents(join(root, 'intents'));
  const saved = await ledger.create({ projectPath: project, sessionId }, { name: 'Draft', setCurrent: false });
  const calls: string[] = [];
  const bridge = { registerWindow() {}, registerIpc() {}, retainBackgroundSession: () => () => {}, get: () => undefined,
    restoreDurableDraft: async (ref: { sessionId: string; projectPath: string }, title: string) => {
      assert.deepEqual([ref.projectPath, ref.sessionId, title], [project, sessionId, 'Draft']); calls.push('restore'); return {};
    }, activateSession: (ref: { sessionId: string }) => { assert.equal(ref.sessionId, sessionId); calls.push('activate'); } };
  const handlers = new Map<string, (...args: any[]) => any>();
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), { invalidate() {}, find: async () => undefined } as any,
    { handle: (name: string, handler: any) => handlers.set(name, handler), removeHandler() {} } as any);
  host.attachLocalAppSessions({ begin: () => { throw new Error('unexpected'); },
    get: async id => { const entry = await ledger.get(id); if (!entry) throw new Error('missing'); return entry; },
    recordSource: (id, source) => ledger.recordSource(id, source),
    finish: async entry => { assert.equal(entry.requestId, saved.requestId); calls.push('register'); await ledger.ready(entry.requestId); } });
  (host as any).rememberCreatedSessionSource = async (ref: { projectPath: string; sessionId: string }, _runtime: unknown, _scheduled: boolean, capture: (source: unknown) => Promise<void>) => {
    calls.push('verify'); const path = join(root, `${ref.sessionId}.jsonl`);
    await capture({ scope: 'current', transcriptPath: path });
    settings.rememberSessionTranscriptSelection(ref, { catalogProjectPath: project, scope: 'current', transcriptPath: path }); return path;
  };
  (host as any).assertLocalAppSessionRuntime = async () => {};
  (host as any).bootstrap = () => ({ settings: settings.getPublic() });
  const frame = { url: 'http://127.0.0.1:4242' }, sender = { mainFrame: frame, isDestroyed: () => false, once() {}, send() {} };
  host.registerWindow(sender as any, frame.url); host.registerIpc();
  const result = await handlers.get(CH_SESSION_NEW)!({ sender, senderFrame: frame }, null, undefined, { kind: 'recover-local-app', requestId: saved.requestId });
  assert.deepEqual(calls, ['restore', 'verify', 'register', 'activate']);
  assert.equal(result.settings.activeSession.sessionId, sessionId);
  assert.equal((await ledger.get(saved.requestId))?.state, 'ready');
});

test('Local App recovery selects its recorded transcript when another copy has the same UUID', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-local-app-exact-source-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const project = join(root, 'project'); mkdirSync(project);
  const settings = new SettingsStore(join(root, 'settings'));
  settings.addProject(project); settings.conversationSpaces.addProject(project, 'code'); settings.conversationSpaces.setMode('code');
  const ledger = new AuthoringSessionIntents(join(root, 'intents'));
  const original = join(root, 'copy-a', `${sessionId}.jsonl`), other = join(root, 'copy-b', `${sessionId}.jsonl`);
  let saved = await ledger.create({ projectPath: project, sessionId }, { name: 'Draft', setCurrent: false });
  saved = await ledger.recordSource(saved.requestId, { scope: 'repository', transcriptPath: original });
  const opened: string[] = [];
  const catalog = { invalidate() {}, find: async () => { throw new Error(`must not pick ${other}`); },
    select: async (_project: string, _id: string, selection: { transcriptPath: string; scope: string }) => {
      assert.deepEqual(selection, { scope: 'repository', transcriptPath: original });
      return { uuid: sessionId, path: original, mode: 'code', workspace_path: project, empty_session: true, message_count: 0 };
    } };
  const bridge = { registerWindow() {}, registerIpc() {}, retainBackgroundSession: () => () => {}, get: () => undefined,
    openSession: async (_ref: unknown, _empty: boolean, _model: unknown, _activate: boolean, path: string) => { opened.push(path); return {}; },
    activateSession() {} };
  const handlers = new Map<string, (...args: any[]) => any>();
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), catalog as any,
    { handle: (name: string, handler: any) => handlers.set(name, handler), removeHandler() {} } as any);
  host.attachLocalAppSessions({ begin: () => { throw new Error('unused'); }, get: async id => (await ledger.get(id))!,
    recordSource: (id, source) => ledger.recordSource(id, source), finish: async entry => { await ledger.ready(entry.requestId); } });
  (host as any).rememberCreatedSessionSource = async (ref: { projectPath: string; sessionId: string }, _runtime: unknown, _scheduled: boolean, capture: (source: unknown) => Promise<void>) => {
    await capture({ scope: 'repository', transcriptPath: original });
    settings.rememberSessionTranscriptSelection(ref, { catalogProjectPath: project, scope: 'repository', transcriptPath: original }); return original;
  };
  (host as any).assertLocalAppSessionRuntime = async () => {};
  (host as any).bootstrap = () => ({ settings: settings.getPublic() });
  const frame = { url: 'http://127.0.0.1:4242' }, sender = { mainFrame: frame, isDestroyed: () => false, once() {}, send() {} };
  host.registerWindow(sender as any, frame.url); host.registerIpc();
  await handlers.get(CH_SESSION_NEW)!({ sender, senderFrame: frame }, null, undefined, { kind: 'recover-local-app', requestId: saved.requestId });
  assert.deepEqual(opened, [original]);
  await assert.rejects(ledger.recordSource(saved.requestId, { scope: 'repository', transcriptPath: other }), /source changed/i);
});

test('an unconfirmed Local App source cannot adopt an unrelated same-UUID selection or runtime', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-local-app-unconfirmed-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  for (const collision of ['selection', 'runtime'] as const) {
    const project = join(root, collision); mkdirSync(project);
    const settings = new SettingsStore(join(root, `settings-${collision}`));
    settings.addProject(project); settings.conversationSpaces.addProject(project, 'code'); settings.conversationSpaces.setMode('code');
    const ledger = new AuthoringSessionIntents(join(root, `intents-${collision}`));
    const saved = await ledger.create({ projectPath: project, sessionId }, { name: 'Draft', setCurrent: false });
    if (collision === 'selection') settings.rememberSessionTranscriptSelection({ projectPath: project, sessionId },
      { catalogProjectPath: project, scope: 'all', transcriptPath: join(root, 'other', `${sessionId}.jsonl`) });
    let started = false;
    const bridge = { registerWindow() {}, registerIpc() {}, retainBackgroundSession: () => () => {},
      get: () => collision === 'runtime' ? {} : undefined, restoreDurableDraft: async () => { started = true; return {}; } };
    const handlers = new Map<string, (...args: any[]) => any>();
    const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), { invalidate() {} } as any,
      { handle: (name: string, handler: any) => handlers.set(name, handler), removeHandler() {} } as any);
    host.attachLocalAppSessions({ begin: () => { throw new Error('unused'); }, get: async id => (await ledger.get(id))!,
      recordSource: (id, source) => ledger.recordSource(id, source), finish: async () => { throw new Error('unexpected'); } });
    const frame = { url: 'http://127.0.0.1:4242' }, sender = { mainFrame: frame, isDestroyed: () => false, once() {}, send() {} };
    host.registerWindow(sender as any, frame.url); host.registerIpc();
    await assert.rejects(handlers.get(CH_SESSION_NEW)!({ sender, senderFrame: frame }, null, undefined,
      { kind: 'recover-local-app', requestId: saved.requestId }), /source was not confirmed/);
    assert.equal(started, false);
    assert.equal((await ledger.get(saved.requestId))?.source, undefined);
  }
});

test('recovering Local App source A never retargets a live same-UUID source B', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-local-app-live-alias-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const project = join(root, 'project'); mkdirSync(project);
  const original = join(root, 'a', `${sessionId}.jsonl`), livePath = join(root, 'b', `${sessionId}.jsonl`);
  mkdirSync(join(root, 'a')); mkdirSync(join(root, 'b')); writeFileSync(original, '{}\n'); writeFileSync(livePath, '{}\n');
  const settings = new SettingsStore(join(root, 'settings'));
  settings.addProject(project); settings.conversationSpaces.addProject(project, 'code'); settings.conversationSpaces.setMode('code');
  const ledger = new AuthoringSessionIntents(join(root, 'intents'));
  const created = await ledger.create({ projectPath: project, sessionId }, { name: 'Draft', setCurrent: false });
  const saved = await ledger.recordSource(created.requestId, { scope: 'repository', transcriptPath: original });
  let opened = false, stopped = false;
  const runtime = { projectPath: project, sessionId, sessionMode: 'code', connectionState: { status: 'connected' },
    isStarting: false, needsSessionResume: false, turnActive: true, remembered: livePath,
    getWorkspaceState: async () => ({ cwd: project, transcript_path: livePath }), dispose: () => { stopped = true; } };
  const bridge = { registerWindow() {}, registerIpc() {}, retainBackgroundSession: () => () => {}, get: () => runtime,
    openSession: async () => { opened = true; runtime.remembered = original; return runtime; } };
  const catalog = { invalidate() {}, select: async () => ({ uuid: sessionId, path: original, mode: 'code', workspace_path: project, empty_session: true }) };
  const handlers = new Map<string, (...args: any[]) => any>();
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), catalog as any,
    { handle: (name: string, handler: any) => handlers.set(name, handler), removeHandler() {} } as any);
  host.attachLocalAppSessions({ begin: () => { throw new Error('unused'); }, get: async () => saved,
    recordSource: (id, source) => ledger.recordSource(id, source), finish: async () => { throw new Error('unexpected'); } });
  const frame = { url: 'http://127.0.0.1:4242' }, sender = { mainFrame: frame, isDestroyed: () => false, once() {}, send() {} };
  host.registerWindow(sender as any, frame.url); host.registerIpc();
  await assert.rejects(handlers.get(CH_SESSION_NEW)!({ sender, senderFrame: frame }, null, undefined,
    { kind: 'recover-local-app', requestId: saved.requestId }), /source changed/);
  assert.equal(opened, false); assert.equal(stopped, false);
  assert.equal(runtime.turnActive, true); assert.equal(runtime.remembered, livePath);
  assert.equal(settings.conversationSpaces.findBySessionId(sessionId), undefined, 'a rejected alias cannot claim logical ownership');
});

test('an unknown catalog row cannot claim a scope from its workspace path', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-unknown-owner-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const settings = new SettingsStore(root);
  settings.addProject(root);
  const path = join(root, `${sessionId}.jsonl`);
  const row = { uuid: sessionId, path, mode: 'chat', workspace_path: root, message_count: 1 };
  let opened = false, captured = false;
  const bridge = { get: () => undefined, isProjectClosing: () => false, openSession: async () => { opened = true; } };
  const catalog = { select: async () => row, invalidate() {}, captureMetadataSource: async () => { captured = true; } };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), catalog as any);
  const selection = { scope: 'current', transcriptPath: path };
  const ref = { projectPath: root, sessionId };
  await assert.rejects((host as any).openSessionAndActivateInternal(ref, selection), /no authenticated conversation scope/);
  await assert.rejects((host as any).prepareMetadataSource(ref, selection), /no authenticated conversation scope/);
  assert.equal(opened, false); assert.equal(captured, false);
  assert.equal(settings.conversationSpaces.findBySessionId(sessionId), undefined);
});

test('a registered Global owner keeps its scope after its cwd moves into a project', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-global-owner-project-cwd-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const project = join(root, 'project'); mkdirSync(project);
  const settings = new SettingsStore(join(root, 'settings'));
  settings.addProject(project);
  const original = { projectPath: settings.conversationSpaces.globalWorkspaces.chat, projectId: null, sessionId, mode: 'chat' as const };
  settings.conversationSpaces.register(original);
  const moved = settings.conversationSpaces.relocate(original, project);
  const path = join(root, `${sessionId}.jsonl`);
  const row = { uuid: sessionId, path, mode: 'chat', workspace_path: project, message_count: 1 };
  let opened: any;
  const bridge = { get: () => undefined, isProjectClosing: () => false, openSession: async (owner: unknown) => { opened = owner; } };
  const catalog = { select: async () => row, invalidate() {} };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), catalog as any);
  (host as any).bootstrap = () => ({});
  await (host as any).openSessionAndActivateInternal(moved, { scope: 'current', transcriptPath: path });
  assert.equal(opened.projectId, null);
  assert.equal(settings.conversationSpaces.findBySessionId(sessionId)?.projectId, null);
});

for (const changedAt of ['before-open', 'after-open'] as const) {
  test(`Local App authoring checks its fixed source ${changedAt} without activating or stopping either owner`, async t => {
    const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-authoring-open-')));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const project = join(root, 'source'), moved = join(root, 'moved'); mkdirSync(project); mkdirSync(moved);
    const settings = new SettingsStore(join(root, 'settings'));
    settings.addProject(project); settings.conversationSpaces.addProject(project, 'code');
    const prior = { projectPath: project, projectId: project, sessionId, mode: 'code' as const };
    const selected = { ...prior, sessionId: targetId };
    settings.conversationSpaces.register(prior); settings.conversationSpaces.register(selected);
    settings.conversationSpaces.setMode('code'); settings.setActiveSession(prior);
    let cwd = changedAt === 'before-open' ? moved : project;
    let opened = 0, activated = 0, closed = 0, cancelled = 0, verified = 0;
    const runtime = { projectPath: project, sessionId: targetId, sessionMode: 'code', connectionState: { status: 'connected' },
      turnActive: true, getWorkspaceState: async () => ({ cwd }), cancel: () => { cancelled++; } };
    const bridge = { get: () => runtime, isProjectClosing: () => false,
      openSession: async (_ref: unknown, _empty: unknown, _model: unknown, activate: boolean) => {
        opened++; assert.equal(activate, false); if (changedAt === 'after-open') cwd = moved;
      }, activateSession: () => { activated++; }, closeSession: () => { closed++; } };
    const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), {} as any);
    (host as any).trustSessionSource = async () => {};
    (host as any).bootstrap = () => ({ activeSession: settings.getPublic().activeSession });
    await assert.rejects((host as any).openSessionAndActivateInternal(selected, undefined, undefined, 'code',
      { sourceDirectory: project, commit: async (operation: () => unknown) => { verified++; return operation(); } }), /authoring workspace changed/);
    assert.equal(opened, changedAt === 'after-open' ? 1 : 0);
    assert.equal(settings.getPublic().activeSession?.sessionId, sessionId);
    assert.equal(runtime.turnActive, true); assert.equal(activated, 0); assert.equal(closed, 0); assert.equal(cancelled, 0); assert.equal(verified, 0);
  });
}

test('Local App navigation rechecks the captured source receipt before activating an acknowledged session', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-authoring-receipt-open-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const project = join(root, 'source'); mkdirSync(project);
  const settings = new SettingsStore(join(root, 'settings'));
  settings.addProject(project); settings.conversationSpaces.addProject(project, 'code');
  const prior = { projectPath: project, projectId: project, sessionId, mode: 'code' as const };
  const selected = { ...prior, sessionId: targetId };
  settings.conversationSpaces.register(prior); settings.conversationSpaces.register(selected); settings.setActiveSession(prior);
  const runtime = { projectPath: project, sessionId: targetId, sessionMode: 'code', connectionState: { status: 'connected' },
    getWorkspaceState: async () => ({ cwd: project }) };
  let activated = false, closed = false;
  const bridge = { get: () => runtime, isProjectClosing: () => false, openSession: async () => {},
    activateSession: () => { activated = true; }, closeSession: () => { closed = true; } };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), {} as any);
  (host as any).trustSessionSource = async () => {};
  await assert.rejects((host as any).openSessionAndActivateInternal(selected, undefined, undefined, 'code',
    { sourceDirectory: project, commit: async () => { throw new Error('captured owner replaced'); } }), /captured owner replaced/);
  assert.equal(activated, false); assert.equal(closed, false);
  assert.equal(settings.getPublic().activeSession?.sessionId, sessionId);
});

test('a verified Local App Code owner activates only after its workspace and receipt are confirmed', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-authoring-confirmed-open-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const project = join(root, 'source'); mkdirSync(project);
  const settings = new SettingsStore(join(root, 'settings'));
  settings.addProject(project); settings.conversationSpaces.addProject(project, 'code');
  const prior = { projectPath: project, projectId: project, sessionId, mode: 'code' as const };
  const selected = { ...prior, sessionId: targetId };
  settings.conversationSpaces.register(prior); settings.conversationSpaces.register(selected); settings.setActiveSession(prior);
  const order: string[] = [];
  const runtime = { projectPath: project, sessionId: targetId, sessionMode: 'code', connectionState: { status: 'connected' },
    getWorkspaceState: async () => { order.push('cwd'); return { cwd: project }; } };
  const bridge = { get: () => runtime, isProjectClosing: () => false, openSession: async () => { order.push('ack'); },
    activateSession: () => { order.push('activate'); } };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), {} as any);
  (host as any).trustSessionSource = async () => {};
  (host as any).bootstrap = () => ({});
  await (host as any).openSessionAndActivateInternal(selected, undefined, undefined, 'code',
    { sourceDirectory: project, commit: async (operation: () => unknown) => { order.push('receipt'); return operation(); } });
  assert.deepEqual(order, ['cwd', 'ack', 'cwd', 'receipt', 'activate']);
  assert.equal(settings.getPublic().activeSession?.sessionId, targetId);
});

test('a linked session checks its fixed mode before switching spaces and rolls back failed opens', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-linked-mode-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const settings = new SettingsStore(root), path = settings.conversationSpaces.globalWorkspaces.code;
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
  const settings = new SettingsStore(root), path = settings.conversationSpaces.globalWorkspaces.chat;
  const source = { projectPath: path, projectId: null, sessionId, mode: 'chat' as const };
  const target = { projectPath: settings.conversationSpaces.globalWorkspaces.code, projectId: null, sessionId: targetId, mode: 'code' as const };
  settings.conversationSpaces.register(source); settings.conversationSpaces.register(target); settings.setActiveSession(source);
  const handlers = new Map<string, (...args: any[]) => any>(), released: string[] = [], contexts: unknown[] = [];
  let cancels = 0, finishes = 0;
  const runtimes = new Map([sessionId, targetId].map(id => [id, { projectPath: path, sessionId: id,
    audioSessionContext: { sessionId: id, profileId: `profile-${id}` }, retainAudioOperation: () => () => {} }]));
  const bridge = { registerWindow() {}, registerIpc() {}, get: (id: string) => runtimes.get(id), isProjectClosing: () => false,
    retainBackgroundSession: (ref: { sessionId: string }) => () => released.push(ref.sessionId) };
  const audio = { onEvent: () => () => {}, executeUiAudioOperation: async (_operation: unknown, _window: unknown, _revision: unknown, _override: unknown, context: unknown, release?: () => void) => { contexts.push(context); release?.(); return {}; },
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
  const original = runtimes.get(sessionId)!;
  runtimes.set(sessionId, { ...original });
  await assert.rejects(execute(event, { type: 'speak', text: 'replacement' }, undefined, undefined, owner), /owner changed/);
  runtimes.set(sessionId, original);
  lease(event, sessionId, second, false);
  assert.deepEqual(released, [sessionId], 'the completed execute released its own reservation');
  assert.throws(() => lease(event, targetId, second, true), /another conversation/);
  lease(event, sessionId, first, false);
  lease(event, targetId, second, true);
  lease(event, sessionId, first, false);
  assert.deepEqual(released, [sessionId, sessionId]);
  lease(event, targetId, second, false);
  assert.deepEqual(released, [sessionId, sessionId, targetId]);
  await execute(event, { type: 'speak', text: 'current' });
  assert.deepEqual(contexts[1], { session: { sessionId: targetId, profileId: `profile-${targetId}` } });
});

test('a native operation retains its owner until it settles even if its flow lease is released', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-audio-operation-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const settings = new SettingsStore(root), path = settings.conversationSpaces.globalWorkspaces.chat;
  settings.conversationSpaces.register({ projectPath: path, projectId: null, sessionId, mode: 'chat' });
  settings.setActiveSession({ projectPath: path, sessionId });
  const finished = deferred<any>(), handlers = new Map<string, (...args: any[]) => any>();
  let reservations = 0;
  const runtime = { projectPath: path, sessionId, retainAudioOperation: () => { reservations++; return () => { reservations--; }; } };
  const bridge = { registerWindow() {}, registerIpc() {}, get: () => runtime, isProjectClosing: () => false,
    retainBackgroundSession: () => () => {} };
  const audio = { onEvent: () => () => {}, executeUiAudioOperation: async (_operation: unknown, _window: unknown, _revision: unknown,
    _override: unknown, _context: unknown, release?: () => void) => { try { return await finished.promise; } finally { release?.(); } } };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), {} as any,
    { handle: (name: string, handler: any) => handlers.set(name, handler), removeHandler() {} } as any, undefined, undefined, audio as any);
  const frame = { url: 'http://127.0.0.1:4242' };
  const sender = { id: 7, mainFrame: frame, isDestroyed: () => false, once() {}, send() {} };
  host.registerWindow(sender as any, frame.url); host.registerIpc();
  try {
    const event = { sender, senderFrame: frame }, leaseId = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';
    const lease = handlers.get(CH_NATIVE_AUDIO_FLOW_LEASE)!;
    lease(event, sessionId, leaseId, true);
    const executing = handlers.get(CH_NATIVE_AUDIO_OPERATION)!(event, { type: 'listen' }, undefined, undefined, { sessionId, leaseId });
    assert.equal(reservations, 2);
    lease(event, sessionId, leaseId, false);
    assert.equal(reservations, 1);
    finished.resolve({ result: { type: 'transcript', text: 'done' } }); await executing;
    assert.equal(reservations, 0);
  } finally { host.dispose(); }
});

test('renderer teardown holds the audio runtime until native cancellation is confirmed', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-audio-detach-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const settings = new SettingsStore(root), path = settings.conversationSpaces.globalWorkspaces.chat;
  settings.conversationSpaces.register({ projectPath: path, projectId: null, sessionId, mode: 'chat' });
  settings.setActiveSession({ projectPath: path, sessionId });
  const cancelled = deferred<void>(), handlers = new Map<string, (...args: any[]) => any>();
  let reservations = 0, background = 0;
  const runtime = { projectPath: path, sessionId, retainAudioOperation: () => { reservations++; return () => { reservations--; }; } };
  const bridge = { registerWindow() {}, registerIpc() {}, get: () => runtime, isProjectClosing: () => false,
    retainBackgroundSession: () => { background++; return () => { background--; }; } };
  const audio = { onEvent: () => () => {}, cancelUiAudioOperations: () => cancelled.promise };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), {} as any,
    { handle: (name: string, handler: any) => handlers.set(name, handler), removeHandler() {} } as any, undefined, undefined, audio as any);
  const frame = { url: 'http://127.0.0.1:4242' };
  const sender = Object.assign(new EventEmitter(), { id: 7, mainFrame: frame, isDestroyed: () => false, send() {} });
  host.registerWindow(sender as any, frame.url); host.registerIpc();
  try {
    handlers.get(CH_NATIVE_AUDIO_FLOW_LEASE)!({ sender, senderFrame: frame }, sessionId, 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', true);
    sender.emit('did-start-navigation', {}, frame.url, false, true);
    assert.equal(reservations, 1); assert.equal(background, 1);
    cancelled.resolve(undefined); await cancelled.promise; await new Promise(resolve => setImmediate(resolve));
    assert.equal(reservations, 0); assert.equal(background, 0);
  } finally { host.dispose(); }
});

test('a failed latest switch restores the confirmed space after rapid opposing choices', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-space-failure-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const settings = new SettingsStore(root), path = settings.conversationSpaces.globalWorkspaces.chat;
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
  const settings = new SettingsStore(root), path = settings.conversationSpaces.globalWorkspaces.chat;
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
  const settings = new SettingsStore(root), path = settings.conversationSpaces.globalWorkspaces.chat;
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
  const settings = new SettingsStore(root), path = settings.conversationSpaces.globalWorkspaces.chat;
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
