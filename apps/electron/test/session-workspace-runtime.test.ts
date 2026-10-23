import assert from 'node:assert/strict';
import { EventEmitter, once } from 'node:events';
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test, type TestContext } from 'node:test';
import { BridgeClient } from '@lingxi/bridge-client';
import type { SetWorkingDirectoryRequest } from '@lingxi/bridge-client';

import { SessionRuntime } from '../src/main/bridge';
import { isRuntimeRemovedState } from '../src/renderer/bridge/bridgeRuntimeState';
import type { WorkspaceLaunchSettings } from '../src/main/bridgeTypes';
import { SessionRuntimeManager } from '../src/main/sessionRuntimeManager';

const sessionId = '11111111-2222-4333-8444-555555555555';
const secondId = '22222222-3333-4444-8555-666666666666';

function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (cause: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const nextTurn = () => new Promise<void>(resolve => setImmediate(resolve));

test('superseded audio initialization cannot create an unowned Bridge connection', async (t) => {
  const connect = BridgeClient.prototype.connect;
  let connections = 0;
  BridgeClient.prototype.connect = async function () { connections++; throw new Error('must not connect'); };
  t.after(() => { BridgeClient.prototype.connect = connect; });
  for (const interruption of ['restart', 'dispose'] as const) {
    const audioReady = deferred<void>();
    const runtime = new SessionRuntime({ sessionId, projectPath: '/project',
      launchConfig: () => ({ workspace: '/project', trusted: true, sessionId }),
      audioService: { getCapabilities: () => undefined, onEvent: () => () => {},
        initializeCapabilities: () => audioReady.promise } as any });
    const internal = runtime as any;
    const connecting = internal.connectBridgeClient('/unused', internal.generation, false);
    const rejected = assert.rejects(connecting, /superseded/);
    if (interruption === 'dispose') await runtime.dispose();
    else await internal.stopBridge();
    audioReady.resolve(undefined);
    await rejected;
    assert.equal(internal.client, null);
    await runtime.dispose();
  }
  assert.equal(connections, 0);
});

test('a superseded hello closes its own client without overwriting the replacement capability or state', async (t) => {
  const connect = BridgeClient.prototype.connect, close = BridgeClient.prototype.close;
  const hello = deferred<any>(), connected: BridgeClient[] = [], closed: BridgeClient[] = [];
  BridgeClient.prototype.connect = function () { connected.push(this); return hello.promise; };
  BridgeClient.prototype.close = function () { closed.push(this); };
  t.after(() => { BridgeClient.prototype.connect = connect; BridgeClient.prototype.close = close; });
  const runtime = new SessionRuntime({ sessionId, projectPath: '/project',
    launchConfig: () => ({ workspace: '/project', trusted: true, sessionId }) });
  const internal = runtime as any;
  const connecting = internal.connectBridgeClient('/unused', internal.generation, false);
  const rejected = assert.rejects(connecting, /superseded/);
  await nextTurn();
  assert.equal(connected.length, 1);
  const replacement = new EventEmitter();
  internal.generation++;
  internal.wireClient(replacement, internal.generation);
  internal.verifiedRealtimeAudio = true;
  internal.setState({ status: 'connected' });
  hello.resolve({ server_name: 'old', protocol_version: 'old', capabilities: { realtime_audio: false } });
  await rejected;
  assert.deepEqual(closed, connected);
  assert.equal(internal.client, replacement);
  assert.equal(runtime.realtimeAudioSupported, true);
  assert.equal(runtime.connectionState.status, 'connected');
  internal.client = null;
  await runtime.dispose();
});

class WorkspaceClient extends EventEmitter {
  calls: unknown[] = [];
  closes = 0;
  cwd = '/project';
  directories: string[] = [];
  close() { this.closes++; }
  sendPrompt(...args: unknown[]) { this.calls.push(['prompt', ...args]); }
  sendCommand(command: unknown) { this.calls.push(['command', command]); }
  async getWorkspaceState() { return { cwd: this.cwd, additional_directories: [...this.directories] }; }
  async setWorkingDirectory(request: SetWorkingDirectoryRequest) {
    this.calls.push(['cd', request]); this.cwd = request.path;
    return { status: 'ok' as const, cwd: this.cwd, changed: true, transcript_relocated: true };
  }
  async addWorkingDirectory(path: string) {
    this.calls.push(['add-dir', path]); this.directories.push(path);
    return this.getWorkspaceState();
  }
}

function connectedRuntime(t: TestContext) {
  const runtime = new SessionRuntime({ sessionId, projectPath: '/project', launchConfig: () => ({ workspace: '/project', trusted: true, sessionId }) });
  const client = new WorkspaceClient();
  Object.assign(runtime as any, { client, activeWorkspace: '/project', activeWorkspaceTrusted: true });
  (runtime as any).setState({ status: 'connected' });
  t.after(() => runtime.dispose());
  return { runtime, client };
}

function selectedResumeRuntime(t: TestContext, timeoutMs = 1500) {
  const runtime = new SessionRuntime({ sessionId, projectPath: '/project', sessionResumeTimeoutMs: timeoutMs,
    launchConfig: () => ({ workspace: '/project', trusted: true, sessionId }) });
  const acknowledgement = deferred<void>(), restored: string[] = [];
  const client = Object.assign(new WorkspaceClient(), {
    resumeSessionFromTranscript: (id: string, path: string) => {
      client.calls.push(['resume-selected', id, path]);
      return acknowledgement.promise;
    },
  });
  const internal = runtime as any;
  Object.assign(internal, { client, activeWorkspace: '/project', activeWorkspaceTrusted: true, generation: 1,
    ensureModelProviderCredential: async () => {},
    restoreModel: async () => { restored.push('model'); },
    restorePermissionMode: async () => { restored.push('permission'); },
    restoreFastMode: async () => { restored.push('fast'); },
  });
  internal.wireClient(client, 1);
  internal.setState({ status: 'connected' });
  t.after(() => runtime.dispose());
  return { runtime, client, acknowledgement, restored };
}

function resumeProof(client: WorkspaceClient, id = sessionId) {
  client.emit('event', { type: 'session_resumed', session_id: id, messages: [] });
  client.emit('event', { type: 'model_changed', model: 'anthropic/restored-model' });
}

test('a failed owned restore cannot be reused as ready and retries require full engine proof', async t => {
  for (const firstOperation of ['open', 'restart'] as const) {
    await t.test(firstOperation, async t => {
      const manager = new SessionRuntimeManager({ launchConfig: ref => ({ workspace: ref.projectPath, trusted: true, sessionId: ref.sessionId }) });
      t.after(() => manager.dispose());
      const ref = { projectPath: '/project', sessionId }, runtime = await manager.ensure(ref, false), internal = runtime as any;
      let requests = 0;
      const acknowledgement = deferred<void>();
      const client = Object.assign(new WorkspaceClient(), {
        resumeSessionFromTranscript: async () => {
          if (++requests === 1) throw new Error('selected transcript RPC transient failure');
          await acknowledgement.promise;
        },
      });
      Object.assign(internal, { activeWorkspace: '/project', activeWorkspaceTrusted: true, ownedTranscriptPath: '/saved/session.jsonl',
        sessionHasHistory: firstOperation === 'restart', ensureModelProviderCredential: async () => {},
        restoreModel: async () => {}, restorePermissionMode: async () => {}, restoreFastMode: async () => {} });
      internal.wireClient(client, internal.generation);
      runtime.start = async () => { internal.setState({ status: 'connected' }); };
      runtime.restart = async () => { internal.setState({ status: 'connected' }); };
      if (firstOperation === 'restart') internal.setState({ status: 'connected' });
      await assert.rejects(firstOperation === 'open' ? manager.openSession(ref) : manager.restart(ref), /RPC transient failure/);
      assert.equal(runtime.connectionState.status, 'connected');
      assert.equal(runtime.needsSessionResume, true);
      assert.throws(() => runtime.sendPrompt('must not overwrite unrestored history'), /Resume this chat/);
      await assert.rejects(runtime.dispatchCommand({ type: 'list_models' }), /Resume this chat/);
      assert.throws(() => runtime.beginWorkspaceChange(), /Resume this chat/);
      await assert.rejects(runtime.manageCron({ action: 'list' }), /Resume this chat/);
      await assert.rejects(runtime.runScheduledTurn('run', { automation: { model: 'test/model' } } as any), /must be resumed/);
      // Reconnecting the same process also cannot certify the failed restore.
      internal.liveConnectionRecovered = true;
      let ready = false;
      const retry = manager.openSession(ref).then(() => { ready = true; });
      await nextTurn();
      assert.equal(requests, 2);
      assert.equal(ready, false);
      resumeProof(client); await nextTurn();
      assert.equal(ready, false, 'matching UUID/model alone do not acknowledge the selected transcript RPC');
      acknowledgement.resolve(undefined); await retry;
      assert.equal(runtime.needsSessionResume, false);
      assert.equal(ready, true);
      await runtime.dispatchCommand({ type: 'list_models' });
    });
  }
});

test('opening during a restart waits for its real resume without issuing a second restore', async t => {
  const manager = new SessionRuntimeManager({ launchConfig: ref => ({ workspace: ref.projectPath, trusted: true, sessionId: ref.sessionId }) });
  t.after(() => manager.dispose());
  const ref = { projectPath: '/project', sessionId }, runtime = await manager.ensure(ref, false), internal = runtime as any;
  let requests = 0;
  const acknowledgement = deferred<void>();
  const client = Object.assign(new WorkspaceClient(), { resumeSessionFromTranscript: () => { requests++; return acknowledgement.promise; } });
  Object.assign(internal, { activeWorkspace: '/project', activeWorkspaceTrusted: true, ownedTranscriptPath: '/saved/session.jsonl',
    sessionHasHistory: true, ensureModelProviderCredential: async () => {},
    restoreModel: async () => {}, restorePermissionMode: async () => {}, restoreFastMode: async () => {} });
  internal.wireClient(client, internal.generation); internal.setState({ status: 'connected' });
  runtime.restart = async guard => { guard?.(); };
  const restarting = manager.restart(ref);
  await nextTurn(); assert.equal(requests, 1);
  let ready = false;
  const opening = manager.openSession(ref).then(() => { ready = true; });
  await nextTurn(); assert.equal(ready, false);
  resumeProof(client); await nextTurn(); assert.equal(ready, false);
  acknowledgement.resolve(undefined); await restarting; await opening;
  assert.equal(requests, 1);
  assert.equal(ready, true);
  await assert.rejects(manager.restart(ref, () => { throw new Error('restart refused'); }), /restart refused/);
  assert.equal(await manager.openSession(ref), runtime, 'a refused restart retains the healthy restored owner');
});

test('a restart arriving during a cold open joins its owned restore without a second resume or process', async t => {
  const manager = new SessionRuntimeManager({ launchConfig: ref => ({ workspace: ref.projectPath, trusted: true, sessionId: ref.sessionId }) });
  t.after(() => manager.dispose());
  const ref = { projectPath: '/project', sessionId }, runtime = await manager.ensure(ref, false), internal = runtime as any;
  const start = deferred<void>(), acknowledgement = deferred<void>();
  let requests = 0, restarts = 0;
  const client = Object.assign(new WorkspaceClient(), { resumeSessionFromTranscript: () => {
    requests++;
    if (requests === 1) return acknowledgement.promise;
    queueMicrotask(() => resumeProof(client));
    return Promise.resolve();
  } });
  Object.assign(internal, { activeWorkspace: '/project', activeWorkspaceTrusted: true, ownedTranscriptPath: '/saved/session.jsonl',
    sessionHasHistory: true, ensureModelProviderCredential: async () => {},
    restoreModel: async () => {}, restorePermissionMode: async () => {}, restoreFastMode: async () => {} });
  internal.wireClient(client, internal.generation);
  runtime.start = async () => { await start.promise; internal.setState({ status: 'connected' }); };
  runtime.restart = async guard => { guard?.(); restarts++; };
  const opening = manager.openSession(ref), restarting = manager.restart(ref);
  start.resolve(undefined); await nextTurn();
  assert.equal(requests, 1); assert.equal(restarts, 0);
  resumeProof(client); acknowledgement.resolve(undefined);
  await opening; await restarting;
  assert.equal(requests, 1); assert.equal(restarts, 0);
});

function resumeInteractions(t: TestContext, timeoutMs = 1500) {
  const f = selectedResumeRuntime(t, timeoutMs), internal = f.runtime as any;
  const scopes = new Map<number, ReturnType<typeof deferred<{ request_id: number; background_owned: boolean }>>>();
  const sent: Array<{ channel: string; payload: any }> = [], decisions: unknown[] = [];
  Object.assign(f.client, {
    requestPermissionScope: (id: number) => {
      const scope = deferred<{ request_id: number; background_owned: boolean }>(); scopes.set(id, scope); return scope.promise;
    },
    approvePermission: (id: number, response: unknown) => decisions.push(['approve', id, response]),
    denyPermission: (id: number) => decisions.push(['deny', id]),
    approveComputerAccess: (id: number, response: unknown) => decisions.push(['computer', id, response]),
    denyComputerAccess: (id: number) => decisions.push(['deny-computer', id]),
    answerAskUserQuestion: (id: number, answers: unknown) => decisions.push(['answer', id, answers]),
    cancelAskUserQuestion: (id: number) => decisions.push(['cancel-question', id]),
  });
  internal.broadcast = (channel: string, payload: unknown) => sent.push({ channel, payload });
  const request = (id: number) => ({ request_id: id, kind: { type: 'tool_use_confirm' as const,
    tool_name: 'Bash', tool_input_json: '{"command":"echo deferred"}', default_allow: false } });
  const receive = (id: number) => internal.receivePermissionRequest(f.client, 1, request(id)) as Promise<void>;
  const scope = async (id: number, background_owned: boolean) => {
    assert.ok(scopes.has(id), 'the real permission path must query SDK ownership');
    scopes.get(id)!.resolve({ request_id: id, background_owned }); await nextTurn();
  };
  const computer = (id: number) => f.client.emit('computerAccess', { request_id: id, reason: 'resume deferred automation',
    apps: [{ label: 'Terminal' }], tier: 'full', clipboard_read: false, clipboard_write: false, system_key_combos: false });
  const question = (id: number) => f.client.emit('event', { type: 'ask_user_question', request: { request_id: id,
    questions: [{ question: 'Resume mode?', header: 'Mode', options: [{ label: 'Safe', description: 'Keep safeguards' }], multi_select: false }] } });
  return { ...f, internal, scopes, sent, decisions, receive, request, scope, computer, question };
}

test('selected resume foreground permissions are broadcast and answerable without inventing a prompt turn', async (t) => {
  for (const allow of [true, false]) {
    const f = resumeInteractions(t), pending = f.runtime.resumeOwnedSession('/saved/selected.jsonl');
    void pending.catch(() => undefined);
    assert.equal(f.runtime.turnActive, false);
    const receiving = f.receive(7);
    assert.equal(f.internal.permissionScopeChecks.size, 1);
    assert.equal(f.sent.some(entry => entry.channel === 'lingxi:permission'), false);
    await f.scope(7, false); await receiving;
    const forwarded = f.sent.find(entry => entry.channel === 'lingxi:permission')?.payload;
    assert.equal(forwarded?.request_id, 7);
    assert.equal(forwarded?.backgroundOwned, false);
    assert.equal(f.runtime.pendingInteractions, 1);
    assert.equal(f.runtime.turnActive, false);
    if (allow) f.runtime.approvePermission(7); else f.runtime.denyPermission(7);
    assert.deepEqual(f.decisions, allow ? [['approve', 7, { type: 'allow_once' }]] : [['deny', 7]]);
    assert.equal(f.runtime.pendingInteractions, 0);
    resumeProof(f.client); f.acknowledgement.resolve(undefined); await pending;
    assert.equal(f.runtime.turnActive, false);
  }
});

test('resume approvals can cross multiple ordinary deadlines and complete after the user answers', async (t) => {
  const f = resumeInteractions(t, 15);
  let complete = false;
  const pending = f.runtime.resumeOwnedSession('/saved/selected.jsonl').then(() => { complete = true; });
  void pending.catch(() => undefined);
  const receiving = f.receive(7); await f.scope(7, false); await receiving;
  resumeProof(f.client); f.acknowledgement.resolve(undefined);
  await new Promise(resolve => setTimeout(resolve, 65));
  assert.equal(complete, false, 'waiting for a human decision must not consume the ordinary resume deadline');
  assert.equal(f.internal.pendingPermissionIds.has(7), true);
  assert.deepEqual(f.restored, []);
  f.runtime.approvePermission(7);
  await pending;
  assert.equal(complete, true);
  assert.deepEqual(f.restored, ['model', 'permission', 'fast']);
});

test('resume tool, question and computer approval events remain owned after proof until RPC acknowledgement and tool completion', async (t) => {
  const f = resumeInteractions(t);
  let complete = false;
  const pending = f.runtime.resumeOwnedSession('/saved/selected.jsonl').then(() => { complete = true; });
  void pending.catch(() => undefined);
  f.client.emit('event', { type: 'tool_use_started', id: 'deferred-shell', tool: 'Bash', input_json: '{}',
    header: { verb: 'shell', label: 'Running shell command', title: 'Running shell command' } });
  resumeProof(f.client); await nextTurn();
  assert.equal(complete, false);
  assert.deepEqual(f.restored, []);
  f.client.emit('permission', f.request(7)); await f.scope(7, false);
  f.computer(8); f.question(9);
  assert.equal(f.runtime.pendingInteractions, 3);
  assert.equal(f.runtime.turnActive, false);
  assert.equal(f.sent.some(entry => entry.payload?.type === 'tool_use_started' && entry.payload.id === 'deferred-shell'), true);
  assert.equal(f.sent.some(entry => entry.payload?.type === 'ask_user_question' && entry.payload.request.request_id === 9), true);
  assert.equal(f.sent.some(entry => entry.channel === 'lingxi:computerAccess' && entry.payload.request_id === 8), true);
  f.runtime.approvePermission(7);
  const computerAnswer = { granted_apps: ['Terminal'], clipboard_read: false, clipboard_write: false, system_key_combos: false };
  f.runtime.approveComputerAccess(8, computerAnswer);
  f.runtime.answerAskUserQuestion(9, { 'Resume mode?': 'Safe' });
  assert.deepEqual(f.decisions, [
    ['approve', 7, { type: 'allow_once' }], ['computer', 8, computerAnswer], ['answer', 9, { 'Resume mode?': 'Safe' }],
  ]);
  f.acknowledgement.resolve(undefined); await nextTurn();
  assert.equal(complete, false, 'RPC acknowledgement cannot release a tool which has not finished');
  assert.deepEqual(f.restored, []);
  f.client.emit('event', { type: 'tool_use_result', id: 'deferred-shell', tool: 'Bash', result_json: '"ok"',
    is_error: false, display: { body_lines: 1 } });
  await pending;
  assert.equal(f.sent.some(entry => entry.payload?.type === 'tool_use_result' && entry.payload.id === 'deferred-shell'), true);
  assert.deepEqual(f.restored, ['model', 'permission', 'fast']);
  assert.equal(f.runtime.turnActive, false);
});

test('a deferred foreground permission scope cannot attach after resume rejection, timeout, connection or owner replacement', async (t) => {
  for (const interruption of ['rejection', 'timeout', 'connection', 'generation', 'client'] as const) {
    const f = resumeInteractions(t);
    const pending = f.runtime.resumeOwnedSession('/saved/selected.jsonl');
    const rejected = assert.rejects(pending, /rejected|timed out|disconnected|interrupted/);
    const receiving = f.receive(7);
    if (interruption === 'rejection') f.acknowledgement.reject(new Error('selected copy rejected'));
    else if (interruption === 'timeout') f.acknowledgement.reject(new Error('selected transcript RPC timed out'));
    else if (interruption === 'connection') f.client.emit('close', 1006, 'lost connection');
    else if (interruption === 'generation' || interruption === 'client') {
      if (interruption === 'generation') f.internal.generation++;
      else f.internal.client = new WorkspaceClient();
      f.internal.rejectPendingSessionResume(new Error('session resume was interrupted'));
    }
    await rejected;
    await f.scope(7, false); await receiving;
    assert.equal(f.internal.pendingPermissionIds.size, 0, interruption);
    assert.equal(f.internal.permissionScopeChecks.size, 0, interruption);
    assert.equal(f.sent.some(entry => entry.channel === 'lingxi:permission'), false, interruption);
    assert.deepEqual(f.decisions, []);
    assert.deepEqual(f.restored, []);
  }
});

test('an old deferred permission cannot join a new resume on the same client and generation', async (t) => {
  const f = resumeInteractions(t);
  const first = f.runtime.resumeOwnedSession('/saved/copy-A.jsonl');
  const firstRejected = assert.rejects(first, /copy A rejected/);
  const receiving = f.receive(7), firstOwner = f.internal.pendingSessionResume;
  f.acknowledgement.reject(new Error('copy A rejected')); await firstRejected;
  const secondAck = deferred<void>();
  f.client.resumeSessionFromTranscript = () => secondAck.promise;
  const second = f.runtime.resumeOwnedSession('/saved/copy-B.jsonl');
  assert.notEqual(f.internal.pendingSessionResume, firstOwner);
  await f.scope(7, false); await receiving;
  assert.equal(f.internal.pendingPermissionIds.size, 0);
  assert.equal(f.sent.some(entry => entry.channel === 'lingxi:permission' && entry.payload.request_id === 7), false);
  const currentReceiving = f.receive(8); await f.scope(8, false); await currentReceiving;
  f.runtime.denyPermission(8);
  assert.deepEqual(f.decisions, [['deny', 8]]);
  resumeProof(f.client); secondAck.resolve(undefined); await second;
});

test('replaced client tool, question and computer events cannot join a new resume at the same generation', async (t) => {
  const f = resumeInteractions(t), first = f.runtime.resumeOwnedSession('/saved/copy-A.jsonl');
  const firstRejected = assert.rejects(first, /copy A rejected/);
  f.acknowledgement.reject(new Error('copy A rejected')); await firstRejected;
  const secondAck = deferred<void>();
  const replacement = Object.assign(new WorkspaceClient(), { resumeSessionFromTranscript: () => secondAck.promise });
  f.internal.client = replacement;
  f.internal.wireClient(replacement, 1);
  const second = f.runtime.resumeOwnedSession('/saved/copy-B.jsonl');
  void second.catch(() => undefined);
  const before = f.sent.length;
  f.computer(8); f.question(9);
  f.client.emit('event', { type: 'tool_use_started', id: 'old-connection', tool: 'Bash', input_json: '{}',
    header: { verb: 'shell', label: 'Running shell command', title: 'Running shell command' } });
  f.client.emit('close', 1006, 'retired connection');
  f.client.emit('error', new Error('retired connection failure'));
  assert.equal(f.runtime.connectionState.status, 'connected');
  assert.equal(f.runtime.pendingInteractions, 0);
  assert.equal(f.sent.slice(before).some(entry => entry.channel === 'lingxi:computerAccess'
    || ['ask_user_question', 'tool_use_started'].includes(entry.payload?.type)), false);
  resumeProof(replacement); secondAck.resolve(undefined); await second;
});

test('resume rejection and timeout retire their foreground dialogs while preserving background worker permissions', async (t) => {
  for (const interruption of ['rejection', 'timeout'] as const) {
    const f = resumeInteractions(t);
    const pending = f.runtime.resumeOwnedSession('/saved/selected.jsonl');
    const rejected = assert.rejects(pending, /rejected|timed out/);
    const foreground = f.receive(7); await f.scope(7, false); await foreground;
    const background = f.receive(10); await f.scope(10, true); await background;
    f.computer(8); f.question(9);
    assert.equal(f.runtime.pendingInteractions, 4);
    if (interruption === 'rejection') f.acknowledgement.reject(new Error('selected copy rejected'));
    else f.acknowledgement.reject(new Error('selected transcript RPC timed out'));
    await rejected;
    assert.equal(f.runtime.pendingInteractions, 1);
    assert.equal(f.internal.pendingPermissionIds.has(7), false);
    assert.equal(f.internal.pendingPermissionIds.get(10)?.backgroundOwned, true);
    assert.equal(f.internal.pendingComputerAccessIds.size, 0);
    assert.equal(f.internal.pendingAskUserQuestionIds.size, 0);
    assert.throws(() => f.runtime.approvePermission(7), /not pending/);
    assert.throws(() => f.runtime.denyComputerAccess(8), /not pending/);
    assert.throws(() => f.runtime.cancelAskUserQuestion(9), /not pending/);
    f.runtime.denyPermission(10);
    assert.deepEqual(f.decisions, [['deny', 10]]);
    const forwarded = f.sent.length;
    f.computer(18); f.question(19);
    assert.equal(f.internal.pendingComputerAccessIds.size, 0);
    assert.equal(f.internal.pendingAskUserQuestionIds.size, 0);
    assert.equal(f.sent.slice(forwarded).some(entry => entry.channel === 'lingxi:computerAccess'
      || entry.payload?.type === 'ask_user_question'), false);
  }
});

test('workspace lease blocks prompts and public commands while allowing fenced private controls', async (t) => {
  const { runtime, client } = connectedRuntime(t);
  const release = runtime.beginWorkspaceChange();
  assert.equal(runtime.hasActiveWork, true);
  assert.throws(() => runtime.sendPrompt('racing prompt'), /directory is being changed/);
  await assert.rejects(runtime.dispatchCommand({ type: 'list_models' }), /directory is being changed/);
  assert.deepEqual(client.calls, []);
  assert.deepEqual(await runtime.getWorkspaceState(), { cwd: '/project', additional_directories: [] });
  await runtime.setWorkingDirectory({ path: '/next', trust_accepted: true, trusted_directory: '/next' });
  await runtime.addWorkingDirectory('/extra');
  assert.deepEqual(client.calls, [
    ['cd', { path: '/next', trust_accepted: true, trusted_directory: '/next' }], ['add-dir', '/extra'],
  ]);
  release();
  assert.equal(runtime.hasActiveWork, false);
  await runtime.dispatchCommand({ type: 'list_models' });
  assert.deepEqual(client.calls.at(-1), ['command', { type: 'list_models' }]);
});

test('private workspace mutations require a lease and releasing one is idempotent', (t) => {
  const { runtime } = connectedRuntime(t);
  assert.throws(() => runtime.setWorkingDirectory({ path: '/next' }), /not fenced/);
  assert.throws(() => runtime.addWorkingDirectory('/extra'), /not fenced/);
  const first = runtime.beginWorkspaceChange();
  assert.throws(() => runtime.beginWorkspaceChange(), /active work/);
  first(); first();
  const second = runtime.beginWorkspaceChange();
  first();
  assert.equal(runtime.hasActiveWork, true, 'an old release must not unlock a newer workspace operation');
  second();
});

test('workspace lease refuses foreground work, pending interactions, cron, side chats and startup', (t) => {
  const { runtime } = connectedRuntime(t);
  const internal = runtime as any;
  const cases: Array<{ enter(): void; leave(): void }> = [
    { enter: () => { internal.activeTurn = true; }, leave: () => { internal.activeTurn = false; } },
    { enter: () => internal.gitActivity.accept({ type: 'coordinator_status', active_workers: 1 }), leave: () => internal.gitActivity.reset() },
    { enter: () => internal.pendingPermissionIds.set(1, {}), leave: () => internal.pendingPermissionIds.clear() },
    { enter: () => internal.pendingComputerAccessIds.add(1), leave: () => internal.pendingComputerAccessIds.clear() },
    { enter: () => internal.pendingAskUserQuestionIds.add(1), leave: () => internal.pendingAskUserQuestionIds.clear() },
    { enter: () => internal.pendingPromptHydrations.set(Symbol.for('test'), 1), leave: () => internal.pendingPromptHydrations.clear() },
    { enter: () => internal.pendingCron.set('test', {}), leave: () => internal.pendingCron.clear() },
    { enter: () => { internal.activeCronExecutions = 1; }, leave: () => { internal.activeCronExecutions = 0; } },
    { enter: () => internal.liveSideChats.add('side'), leave: () => internal.liveSideChats.clear() },
    { enter: () => internal.pendingSideCreates.add('side'), leave: () => internal.pendingSideCreates.clear() },
    { enter: () => { internal.startPromise = Promise.resolve(); }, leave: () => { internal.startPromise = null; } },
    { enter: () => { internal.archiving = true; }, leave: () => { internal.archiving = false; } },
  ];
  for (const state of cases) {
    state.enter();
    try { assert.throws(() => runtime.beginWorkspaceChange(), /active work/); }
    finally { state.leave(); }
  }
  const release = runtime.beginWorkspaceChange(); release();
});

test('session grants survive a same-UUID runtime remount and cannot be mutated by their caller', async (t) => {
  const manager = new SessionRuntimeManager({ launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }) });
  t.after(() => manager.dispose());
  const grants = ['/extra'];
  const promptDirectories = ['/project', '/extra'];
  manager.setSessionDirectoryGrants(sessionId, grants);
  manager.setSessionPromptDirectories(sessionId, promptDirectories);
  grants.push('/not-granted');
  promptDirectories.push('/not-authorized');
  const original = await manager.ensure({ projectPath: '/project', sessionId }, false);
  const before = await (original as any).opts.launchConfig();
  assert.deepEqual(before.additionalDirectories, ['/extra']);
  assert.deepEqual(before.promptDirectories, ['/project', '/extra']);
  await manager.closeSession({ projectPath: '/project', sessionId });
  const remounted = await manager.ensure({ projectPath: '/next', sessionId }, false);
  const config = await (remounted as any).opts.launchConfig();
  assert.notEqual(remounted, original);
  assert.equal(config.workspace, '/next');
  assert.deepEqual(config.additionalDirectories, ['/extra']);
  assert.deepEqual(config.promptDirectories, ['/project', '/extra']);
});

test('a same-UUID remount overlays new tool environment while retaining its initial root and configuration owner', async (t) => {
  const settings: Record<string, WorkspaceLaunchSettings> = {
    '/project': { environment: { RETAINED: 'first-only', OVERRIDDEN: 'old', HOME: '/home-A', LINGXI_CONFIG_DIR: '/config-A' },
      configHome: '/owner-A', globalConfigPath: '/owner-A/global.toml' },
    '/next': { environment: { OVERRIDDEN: 'new', ADDED: 'next-only', HOME: '/home-B', LINGXI_CONFIG_DIR: '/config-B' },
      configHome: '/owner-B', globalConfigPath: '/owner-B/global.toml' },
  };
  const lookups: string[] = [];
  const manager = new SessionRuntimeManager({
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
    workspaceSettings: async (projectPath) => { lookups.push(projectPath); return settings[projectPath]!; },
  });
  t.after(() => manager.dispose());
  manager.setSessionProjectRoot(sessionId, '/initial-project');
  const original = await manager.ensure({ projectPath: '/project', sessionId }, false);
  const before = await (original as any).opts.launchConfig();
  assert.equal(before.initialProjectRoot, '/initial-project');
  assert.deepEqual(before.settingsEnvironment, settings['/project']!.environment);
  assert.equal(before.configHome, '/owner-A');
  assert.equal(before.globalConfigPath, '/owner-A/global.toml');
  await manager.closeSession({ projectPath: '/project', sessionId });
  const remounted = await manager.ensure({ projectPath: '/next', sessionId }, false);
  const after = await (remounted as any).opts.launchConfig();
  assert.equal(after.workspace, '/next');
  assert.equal(after.initialProjectRoot, '/initial-project');
  assert.deepEqual(after.settingsEnvironment, { RETAINED: 'first-only', OVERRIDDEN: 'new', ADDED: 'next-only', HOME: '/home-B', LINGXI_CONFIG_DIR: '/config-B' });
  assert.equal(after.configHome, '/owner-A');
  assert.equal(after.globalConfigPath, '/owner-A/global.toml');
  assert.deepEqual(lookups, ['/project', '/next']);
});

test('a workspace lease prevents runtime disposal and cache eviction until it is released', async (t) => {
  const manager = new SessionRuntimeManager({ maxCachedRuntimes: 1,
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }) });
  t.after(() => manager.dispose());
  const ref = { projectPath: '/project', sessionId };
  const runtime = await manager.ensure(ref, false);
  const release = runtime.beginWorkspaceChange();
  await assert.rejects(manager.closeSession(ref), /active work/);
  const newer = await manager.ensure({ projectPath: '/next', sessionId: secondId }, false);
  (manager as any).activate(newer);
  assert.equal(manager.get(sessionId), runtime);
  assert.equal(manager.size, 2, 'a workspace transaction may temporarily exceed the cache bound');
  release();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(manager.get(sessionId), undefined);
  assert.equal(manager.get(secondId), newer);
});

test('selected transcript restoration waits for both its RPC acknowledgement and matching resume/model proof', async (t) => {
  for (const first of ['acknowledgement', 'proof']) {
    const f = selectedResumeRuntime(t), path = '/saved/selected-copy.jsonl';
    let settled = false;
    const pending = f.runtime.resumeOwnedSession(path).then(() => { settled = true; });
    assert.deepEqual(f.client.calls, [['resume-selected', sessionId, path]]);
    if (first === 'acknowledgement') {
      f.acknowledgement.resolve(undefined); await nextTurn();
      assert.equal(settled, false); assert.deepEqual(f.restored, []);
      f.client.emit('event', { type: 'model_changed', model: 'anthropic/restored-model' }); await nextTurn();
      assert.equal(settled, false); assert.deepEqual(f.restored, [], 'a model event alone does not prove that the owned UUID resumed');
      f.client.emit('event', { type: 'session_resumed', session_id: sessionId, messages: [] });
    } else {
      f.client.emit('event', { type: 'session_resumed', session_id: sessionId, messages: [] }); await nextTurn();
      assert.equal(settled, false); assert.deepEqual(f.restored, [], 'session_resumed alone does not prove the restored model');
      f.client.emit('event', { type: 'model_changed', model: 'anthropic/restored-model' }); await nextTurn();
      assert.equal(settled, false); assert.deepEqual(f.restored, [], 'matching events must not outrun a rejected or missing RPC acknowledgement');
      f.acknowledgement.resolve(undefined);
    }
    await pending;
    assert.equal(settled, true);
    assert.deepEqual(f.restored, ['model', 'permission', 'fast']);
  }
});

test('selected transcript rejection, RPC timeout, missing model proof and wrong session proof never report success', async (t) => {
  for (const failure of ['rejection', 'rpc-timeout', 'missing-model', 'wrong-session']) {
    const f = selectedResumeRuntime(t, failure === 'missing-model' ? 30 : 1500);
    const pending = f.runtime.resumeOwnedSession('/saved/selected-copy.jsonl');
    const rejected = assert.rejects(pending, /selected copy rejected|timed out|different session id/);
    // The production resume timer is unref'ed; keep the test alive until its
    // actual timeout fires when only a session_resumed event is received.
    const keepAlive = setTimeout(() => {}, 1000);
    try {
      if (failure === 'rejection') f.acknowledgement.reject(new Error('selected copy rejected'));
      else if (failure === 'rpc-timeout') {
        resumeProof(f.client); await nextTurn();
        f.acknowledgement.reject(new Error('selected transcript RPC timed out'));
      } else {
        f.acknowledgement.resolve(undefined);
        f.client.emit('event', { type: 'session_resumed', session_id: failure === 'wrong-session' ? secondId : sessionId, messages: [] });
      }
      await rejected;
      assert.deepEqual(f.restored, []);
      assert.equal((f.runtime as any).pendingSessionResume, null);
      assert.equal((f.runtime as any).ownedTranscriptPath, undefined);
    } finally { clearTimeout(keepAlive); }
  }
});

test('a selected transcript survives a real runtime restart and manager eviction without falling back to UUID lookup', async (t) => {
  const selectedPath = '/saved/repository/selected-copy.jsonl';
  const selectedCalls: string[] = [], legacyCalls: string[] = [];
  const clients: WorkspaceClient[] = [];
  const originalStart = SessionRuntime.prototype.start;
  const manager = new SessionRuntimeManager({ maxCachedRuntimes: 1, sessionResumeTimeoutMs: 100,
    launchConfig: ref => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }) });
  const connect = (runtime: SessionRuntime) => {
    const internal = runtime as any, client = new WorkspaceClient();
    clients.push(client);
    const sendCommand = client.sendCommand.bind(client);
    client.sendCommand = (command: any) => {
      sendCommand(command);
      if (command.type === 'resume_session') {
        legacyCalls.push(command.session_id);
        queueMicrotask(() => resumeProof(client, runtime.sessionId));
      }
    };
    Object.assign(client, { resumeSessionFromTranscript: (id: string, path: string) => {
      assert.equal(id, sessionId); selectedCalls.push(path);
      queueMicrotask(() => resumeProof(client, id));
      return Promise.resolve();
    } });
    const generation = (internal.generation ?? 0) + 1;
    Object.assign(internal, { client, generation, activeWorkspace: runtime.projectPath, activeWorkspaceTrusted: true,
      ensureModelProviderCredential: async () => {}, startInternal: async () => connect(runtime) });
    internal.wireClient(client, generation);
    internal.setState({ status: 'connected' });
  };
  SessionRuntime.prototype.start = async function () { await (this as any).opts.launchConfig(); connect(this); };
  t.after(async () => { SessionRuntime.prototype.start = originalStart; await manager.dispose(); });
  const ref = { projectPath: '/project', sessionId };
  const original = await manager.openSession(ref, false, undefined, true, selectedPath);
  assert.deepEqual(selectedCalls, [selectedPath]);
  const firstClient = clients[0]!;
  await manager.restart(ref);
  assert.equal(firstClient.closes, 1, 'restart must actually tear down the old transport');
  assert.deepEqual(selectedCalls, [selectedPath, selectedPath]);
  await manager.openSession({ projectPath: '/other', sessionId: secondId }, true);
  await nextTurn();
  assert.equal(manager.get(sessionId), undefined, 'activating another runtime must evict the idle selected one');
  const reopened = await manager.openSession(ref);
  assert.notEqual(reopened, original);
  assert.deepEqual(selectedCalls, [selectedPath, selectedPath, selectedPath]);
  assert.deepEqual(legacyCalls, []);
});

test('the actual bridge spawn receives the exact remembered transcript path as one startup argument', { skip: process.platform === 'win32' }, async (t) => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-workspace-spawn-'));
  const serverBin = join(root, 'capture-arguments');
  writeFileSync(serverBin, '#!/usr/bin/env node\nprocess.stdout.write(JSON.stringify(process.argv.slice(2)));\n');
  chmodSync(serverBin, 0o700);
  const selectedPath = join(root, '中文 copy B', 'selected transcript.jsonl');
  const runtime = new SessionRuntime({ sessionId, projectPath: root, serverBin,
    resumeTranscriptPath: join(root, 'old-copy.jsonl'), launchConfig: () => ({ workspace: root, trusted: true }) });
  t.after(async () => { await runtime.dispose(); rmSync(root, { recursive: true, force: true }); });
  runtime.rememberTranscriptSelection(selectedPath);
  const child = (runtime as any).spawnServer({ workspace: root, trusted: true }, join(root, 'bridge'));
  let output = '', errors = '';
  child.stdout.on('data', (chunk: Buffer) => { output += chunk.toString(); });
  child.stderr.on('data', (chunk: Buffer) => { errors += chunk.toString(); });
  const [code] = await once(child, 'close');
  assert.equal(code, 0, errors);
  const args = JSON.parse(output) as string[];
  const selectedFlag = args.indexOf('--session-transcript-path');
  assert.ok(selectedFlag >= 0);
  assert.equal(args[selectedFlag + 1], selectedPath);
  assert.equal(args.filter(value => value === '--session-transcript-path').length, 1);
  assert.equal(args[args.indexOf('--session-id') + 1], sessionId);
});

test('a cold selected open exposes its exact transcript before the first start and rejects a competing pending copy', async (t) => {
  const selectedPath = '/saved/copy-B.jsonl', starts: unknown[] = [];
  const admitted = deferred<void>(), finished = deferred<void>();
  const originalStart = SessionRuntime.prototype.start, originalResume = SessionRuntime.prototype.resumeOwnedSession;
  const manager = new SessionRuntimeManager({ launchConfig: ref => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }) });
  SessionRuntime.prototype.start = async function () {
    starts.push({ startup: (this as any).opts.resumeTranscriptPath, remembered: (this as any).ownedTranscriptPath });
    (this as any).setState({ status: 'connected' });
  };
  SessionRuntime.prototype.resumeOwnedSession = async function (path) {
    assert.equal(path, selectedPath); admitted.resolve(undefined); await finished.promise;
  };
  t.after(async () => {
    SessionRuntime.prototype.start = originalStart; SessionRuntime.prototype.resumeOwnedSession = originalResume;
    await manager.dispose();
  });
  const ref = { projectPath: '/project', sessionId };
  const opening = manager.openSession(ref, false, undefined, true, selectedPath);
  await admitted.promise;
  assert.deepEqual(starts, [{ startup: selectedPath, remembered: selectedPath }]);
  assert.throws(() => manager.openSession(ref, false, undefined, true, '/saved/copy-A.jsonl'), /another transcript selection/);
  assert.equal(manager.openSession(ref, false, undefined, true, selectedPath), opening);
  finished.resolve(undefined);
  await opening;
});

test('a failed selected open never replaces the last successfully remembered startup transcript', async (t) => {
  const originalStart = SessionRuntime.prototype.start, originalResume = SessionRuntime.prototype.resumeOwnedSession;
  t.after(() => { SessionRuntime.prototype.start = originalStart; SessionRuntime.prototype.resumeOwnedSession = originalResume; });
  for (const previousPath of [undefined, '/saved/verified-copy-A.jsonl']) {
    const starts: Array<string | undefined> = [];
    let rejectResume = false;
    const manager = new SessionRuntimeManager({ launchConfig: ref => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }) });
    t.after(() => manager.dispose());
    SessionRuntime.prototype.start = async function () {
      starts.push((this as any).opts.resumeTranscriptPath);
      (this as any).setState({ status: 'connected' });
    };
    SessionRuntime.prototype.resumeOwnedSession = async function () {
      if (rejectResume) throw new Error('selected transcript rejected');
    };
    const ref = { projectPath: '/project', sessionId };
    if (previousPath) { await manager.openSession(ref, false, undefined, true, previousPath); await manager.closeSession(ref); }
    rejectResume = true;
    await assert.rejects(manager.openSession(ref, false, undefined, true, '/saved/unverified-copy-B.jsonl'), /selected transcript rejected/);
    assert.equal(manager.get(sessionId), undefined);
    rejectResume = false;
    await manager.openSession(ref);
    assert.equal(starts.at(-1), previousPath, 'the next ordinary open must not inherit a path from a rejected selection');
  }
});


test('same-UUID remount restores private controls after selected resume without replaying shared defaults', async (t) => {
  const f = selectedResumeRuntime(t), internal = f.runtime as any;
  internal.remountControls = 'ephemeral-controls';
  Object.assign(f.client, { restoreWorkspaceControls: async (snapshot: string) => {
    assert.equal(snapshot, 'ephemeral-controls'); f.restored.push('private-controls');
  } });
  const pending = f.runtime.resumeOwnedSession('/saved/selected.jsonl');
  resumeProof(f.client); f.acknowledgement.resolve(undefined); await pending;
  assert.deepEqual(f.restored, ['model', 'private-controls']);
  f.runtime.completeWorkspaceRemount();
  assert.equal(internal.remountControls, undefined, 'later control changes cannot be overwritten by the captured state');
});

test('private control restore failure rejects remount instead of exposing defaults as a successful resume', async (t) => {
  const f = selectedResumeRuntime(t), internal = f.runtime as any;
  internal.remountControls = 'ephemeral-controls';
  Object.assign(f.client, { restoreWorkspaceControls: async () => { throw new Error('private restore rejected'); } });
  const pending = f.runtime.resumeOwnedSession('/saved/selected.jsonl');
  const rejected = assert.rejects(pending, /private restore rejected/);
  resumeProof(f.client); f.acknowledgement.resolve(undefined); await rejected;
  assert.deepEqual(f.restored, ['model']);
});


test('private controls finish before a replacement runtime publishes connected', async (t) => {
  const connect = BridgeClient.prototype.connect, send = BridgeClient.prototype.sendCommand;
  const restore = BridgeClient.prototype.restoreWorkspaceControls;
  const pending = deferred<void>(), calls: string[] = [];
  BridgeClient.prototype.connect = async () => ({ server_name: 'test', protocol_version: 'test', capabilities: {} } as any);
  BridgeClient.prototype.sendCommand = () => {};
  BridgeClient.prototype.restoreWorkspaceControls = async snapshot => {
    assert.equal(snapshot, 'ephemeral'); calls.push('restore'); await pending.promise;
  };
  t.after(() => { BridgeClient.prototype.connect = connect; BridgeClient.prototype.sendCommand = send;
    BridgeClient.prototype.restoreWorkspaceControls = restore; });
  const runtime = new SessionRuntime({ sessionId, projectPath: '/project', remountControls: 'ephemeral',
    launchConfig: () => ({ workspace: '/project', trusted: true, sessionId }) });
  const internal = runtime as any;
  internal.activeWorkspaceTrusted = true;
  internal.activeWorkspace = '/project';
  internal.restorePermissionMode = internal.restoreFastMode = async () => { throw new Error('must not replay defaults'); };
  internal.refreshProviderCredentials = async () => {};
  const connecting = internal.connectBridgeClient('/unused', internal.generation);
  await nextTurn();
  assert.deepEqual(calls, ['restore']);
  assert.equal(runtime.connectionState.status, 'connecting');
  pending.resolve(undefined); await connecting;
  assert.equal(runtime.connectionState.status, 'connected');
  await runtime.dispose();
});


test('same-UUID replacement disposes the old owner without tombstoning replacement renderer events', async () => {
  const sent: Array<{ channel: string; payload: any }> = [];
  const manager = new SessionRuntimeManager({ launchConfig: ref => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }) });
  const ref = { projectPath: '/A', sessionId: '33333333-4444-4555-8666-777777777777' };
  const window = Object.assign(new EventEmitter(), { isDestroyed: () => false,
    send: (channel: string, payload: any) => sent.push({ channel, payload }) });
  manager.registerWindow(window as any, 'app://desktop/index.html');
  const old = await manager.ensure(ref, false);
  await manager.closeSession(ref, { replacing: true });
  const replacementState = sent.at(-1)?.payload.event;
  assert.deepEqual(replacementState, { status: 'disconnected', reason: 'session runtime replaced' });
  assert.equal(isRuntimeRemovedState(replacementState), false);
  const replacement = await manager.ensure({ ...ref, projectPath: '/B' }, false);
  assert.notEqual(old, replacement);
  (replacement as any).setState({ status: 'connected' });
  assert.equal(sent.at(-1)?.payload.event.status, 'connected');
  await manager.closeSession({ ...ref, projectPath: '/B' });
  assert.equal(isRuntimeRemovedState(sent.at(-1)?.payload.event), true);
  await manager.dispose();
});
