import { test } from 'node:test';
import assert from 'node:assert/strict';
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { EventEmitter } from 'node:events';

import { BridgeManager, SessionRuntime, SessionRuntimeManager } from '../src/main/bridge';
import { DiagnosticBuffer } from '../src/main/host-utils';

function temporaryDirectory(): string {
  return mkdtempSync(join(tmpdir(), 'lingxi-electron-bridge-test-'));
}

function deferred<T = void>(): { promise: Promise<T>; resolve(value?: T): void; reject(error: unknown): void } {
  let resolvePromise!: (value: T) => void;
  let rejectPromise!: (error: unknown) => void;
  const promise = new Promise<T>((resolve, reject) => {
    resolvePromise = resolve;
    rejectPromise = reject;
  });
  return { promise, resolve: (value?: T) => resolvePromise(value as T), reject: rejectPromise };
}

function fakeWebContents(sent: Array<{ channel: string; payload: unknown }> = []): EventEmitter & {
  isDestroyed(): boolean;
  send(channel: string, payload: unknown): void;
} {
  const webContents = new EventEmitter() as EventEmitter & {
    isDestroyed(): boolean;
    send(channel: string, payload: unknown): void;
  };
  webContents.isDestroyed = () => false;
  webContents.send = (channel, payload) => sent.push({ channel, payload });
  return webContents;
}

test('SessionRuntimeManager owns one destroyed listener for all session runtimes', async () => {
  const manager = new SessionRuntimeManager({
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
  });
  const webContents = fakeWebContents();
  const first = { projectPath: '/workspace', sessionId: '11111111-2222-4333-8444-555555555555' };
  const second = { projectPath: '/workspace', sessionId: '22222222-3333-4444-8555-666666666666' };

  manager.registerWindow(webContents as any, 'app://desktop/index.html');
  await manager.ensure(first, false);
  await manager.ensure(second, false);

  assert.equal(webContents.listenerCount('destroyed'), 1);
  webContents.emit('destroyed');
  assert.equal(webContents.listenerCount('destroyed'), 0);
  assert.equal((manager.get(first.sessionId) as any).targets.size, 0);
  assert.equal((manager.get(second.sessionId) as any).targets.size, 0);

  await manager.dispose();
});

test('disposing SessionRuntimeManager removes its centralized destroyed listener', async () => {
  const manager = new SessionRuntimeManager({
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
  });
  const webContents = fakeWebContents();

  manager.registerWindow(webContents as any, 'app://desktop/index.html');
  assert.equal(webContents.listenerCount('destroyed'), 1);
  await manager.dispose();
  assert.equal(webContents.listenerCount('destroyed'), 0);
});

test('disposing a session runtime sends an explicit removal state to the renderer', async () => {
  const sent: Array<{ channel: string; payload: unknown }> = [];
  const manager = new SessionRuntimeManager({
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
  });
  const webContents = fakeWebContents(sent);
  const ref = { projectPath: '/workspace', sessionId: '33333333-4444-4555-8666-777777777777' };

  manager.registerWindow(webContents as any, 'app://desktop/index.html');
  await manager.ensure(ref, false);
  await manager.closeSession(ref);

  assert.deepEqual(sent.at(-1), {
    channel: 'lingxi:connectionStateChanged',
    payload: {
      sessionId: ref.sessionId,
      event: { status: 'disconnected', reason: 'session runtime disposed' },
    },
  });

  await manager.dispose();
});

test('newSession keeps the generated runtime/session id and does not send a second new_session command', async () => {
  const commands: unknown[] = [];
  let launchRef: { projectPath: string; sessionId: string } | undefined;
  const originalStart = SessionRuntime.prototype.start;
  const manager = new SessionRuntimeManager({
    launchConfig: (ref) => {
      launchRef = ref;
      return { workspace: '/workspace', sessionId: ref.sessionId, trusted: true };
    },
  });
  SessionRuntime.prototype.start = async function () {
    const launch = await (this as any).opts.launchConfig();
    assert.equal(launch.sessionId, this.sessionId);
  };
  try {
    const created = await manager.newSession('/workspace');
    assert.deepEqual(created, launchRef);
    assert.equal(commands.length, 0);
  } finally {
    SessionRuntime.prototype.start = originalStart;
    await manager.dispose();
  }
});

test('newSession reuses one unsent draft per project until the first prompt crosses the bridge', async () => {
  const projectPath = '/workspace-draft';
  const commits: string[] = [];
  let prompts = 0;
  const originalStart = SessionRuntime.prototype.start;
  SessionRuntime.prototype.start = async function () {
    (this as any).activeWorkspace = projectPath;
    (this as any).activeWorkspaceTrusted = true;
    (this as any).state = { status: 'connected' };
  };
  const manager = new SessionRuntimeManager({
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
    onFirstPromptSent: (ref) => { commits.push(ref.sessionId); },
  });
  try {
    const first = await manager.newSession(projectPath);
    const second = await manager.newSession(projectPath);
    assert.equal(second.sessionId, first.sessionId);

    const runtime = manager.require(first) as SessionRuntime & { client: { sendPrompt: () => void } | null };
    (runtime as any).client = { sendPrompt: () => { prompts += 1; } };
    runtime.sendPrompt('hello');
    runtime.sendPrompt('hello again');

    assert.equal(prompts, 2);
    assert.deepEqual(commits, [first.sessionId]);

    const third = await manager.newSession(projectPath);
    const fourth = await manager.newSession(projectPath);
    assert.notEqual(third.sessionId, first.sessionId);
    assert.equal(fourth.sessionId, third.sessionId);
  } finally {
    SessionRuntime.prototype.start = originalStart;
    await manager.dispose();
  }
});

test('newSession deduplicates concurrent draft creation and closeSession clears the draft slot', async () => {
  const projectPath = '/workspace-race';
  const gate = deferred<void>();
  const originalStart = SessionRuntime.prototype.start;
  let starts = 0;
  SessionRuntime.prototype.start = async function () {
    starts += 1;
    (this as any).activeWorkspace = projectPath;
    (this as any).activeWorkspaceTrusted = true;
    (this as any).state = { status: 'connected' };
    await gate.promise;
  };
  const manager = new SessionRuntimeManager({
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
  });
  try {
    const first = manager.newSession(projectPath);
    const second = manager.newSession(projectPath);
    gate.resolve();
    const [created, duplicate] = await Promise.all([first, second]);
    assert.equal(starts, 1);
    assert.equal(duplicate.sessionId, created.sessionId);

    await manager.closeSession(created);
    const next = await manager.newSession(projectPath);
    assert.notEqual(next.sessionId, created.sessionId);
  } finally {
    SessionRuntime.prototype.start = originalStart;
    await manager.dispose();
  }
});

test('first-prompt commit failures keep the draft recoverable and retry without making sendPrompt fail', async () => {
  const projectPath = '/workspace-commit-error';
  const diagnostics = new DiagnosticBuffer();
  let commitAttempts = 0;
  const originalStart = SessionRuntime.prototype.start;
  SessionRuntime.prototype.start = async function () {
    (this as any).activeWorkspace = projectPath;
    (this as any).activeWorkspaceTrusted = true;
    (this as any).state = { status: 'connected' };
  };
  const manager = new SessionRuntimeManager({
    diagnostics,
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
    onFirstPromptSent: () => {
      commitAttempts += 1;
      if (commitAttempts === 1) throw new Error('settings disk is read-only');
    },
  });
  try {
    const first = await manager.newSession(projectPath);
    const runtime = manager.require(first) as SessionRuntime & { client: { sendPrompt: () => void } | null };
    (runtime as any).client = { sendPrompt: () => undefined };

    assert.doesNotThrow(() => runtime.sendPrompt('hello'));
    assert.match(diagnostics.snapshot().at(-1)?.message ?? '', /failed to commit draft session/);

    const recoverable = await manager.newSession(projectPath);
    assert.equal(recoverable.sessionId, first.sessionId);

    assert.doesNotThrow(() => runtime.sendPrompt('retry commit'));
    assert.equal(commitAttempts, 2);
    const next = await manager.newSession(projectPath);
    assert.notEqual(next.sessionId, first.sessionId);
  } finally {
    SessionRuntime.prototype.start = originalStart;
    await manager.dispose();
  }
});

test('owned resume waits for a matching engine event and never exposes renderer lifecycle commands', async () => {
  const sessionId = '11111111-2222-4333-8444-555555555555';
  const commands: unknown[] = [];
  const client = new EventEmitter() as EventEmitter & { sendCommand(command: unknown): void };
  client.sendCommand = (command) => { commands.push(command); };
  const runtime = new SessionRuntime({
    sessionId,
    projectPath: '/workspace',
    sessionResumeTimeoutMs: 100,
    launchConfig: () => ({ workspace: '/workspace', sessionId, trusted: true }),
  });
  (runtime as any).activeWorkspace = '/workspace';
  (runtime as any).activeWorkspaceTrusted = true;
  (runtime as any).state = { status: 'connected' };
  (runtime as any).generation = 1;
  (runtime as any).client = client;
  (runtime as any).wireClient(client, 1);

  let settled = false;
  const resume = runtime.resumeOwnedSession().then(() => { settled = true; });
  await Promise.resolve();
  assert.equal(settled, false);
  assert.deepEqual(commands, [{ type: 'resume_session', session_id: sessionId, cwd: '/workspace' }]);

  client.emit('event', { type: 'session_resumed', session_id: sessionId, messages: [] });
  await resume;
  assert.equal(settled, true);
});

test('session runtime replay resets at resume and reconstructs later transcript events', () => {
  const sessionId = '12121212-3434-4567-8899-aaaaaaaaaaaa';
  const client = new EventEmitter();
  const runtime = new SessionRuntime({
    sessionId,
    projectPath: '/workspace',
    launchConfig: () => ({ workspace: '/workspace', sessionId, trusted: true }),
  });
  (runtime as any).generation = 1;
  (runtime as any).wireClient(client, 1);

  client.emit('event', { type: 'session_started', session_id: sessionId });
  client.emit('event', { type: 'system_notice', level: 'info', message: 'old base' });
  client.emit('event', { type: 'session_resumed', session_id: sessionId, messages: [] });
  client.emit('event', { type: 'turn_started', turn_id: 7 });
  client.emit('event', { type: 'text_delta', text: 'hello' });

  const first = runtime.replaySnapshot();
  assert.deepEqual(first.map((entry) => entry.event.type), ['session_resumed', 'turn_started', 'text_delta']);
  assert.deepEqual(first.map((entry) => entry.sequence), [3, 4, 5]);
  (first[2]!.event as { text: string }).text = 'mutated';
  assert.equal((runtime.replaySnapshot()[2]!.event as { text: string }).text, 'hello');
});

test('failed historical resume removes only the newly-created runtime', async () => {
  const sessionId = '22222222-3333-4444-8555-666666666666';
  const originalStart = SessionRuntime.prototype.start;
  const originalResume = SessionRuntime.prototype.resumeOwnedSession;
  SessionRuntime.prototype.start = async function () {
    (this as any).state = { status: 'connected' };
  };
  SessionRuntime.prototype.resumeOwnedSession = async function () {
    throw new Error('transcript is corrupt');
  };
  const manager = new SessionRuntimeManager({
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
  });
  try {
    await assert.rejects(manager.openSession({ projectPath: '/workspace', sessionId }), /transcript is corrupt/);
    assert.equal(manager.get(sessionId), undefined);
  } finally {
    SessionRuntime.prototype.start = originalStart;
    SessionRuntime.prototype.resumeOwnedSession = originalResume;
    await manager.dispose();
  }
});

test('opening another session leaves the running session alive', async () => {
  const runningRef = { projectPath: '/workspace-a', sessionId: 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee' };
  const nextRef = { projectPath: '/workspace-b', sessionId: 'bbbbbbbb-cccc-4ddd-8eee-ffffffffffff' };
  const originalStart = SessionRuntime.prototype.start;
  const originalResume = SessionRuntime.prototype.resumeOwnedSession;
  SessionRuntime.prototype.start = async function () {
    (this as any).state = { status: 'connected' };
  };
  SessionRuntime.prototype.resumeOwnedSession = async function () {};
  const manager = new SessionRuntimeManager({
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
  });
  try {
    const running = await manager.ensure(runningRef, true);
    (running as any).activeTurn = true;

    const opened = await manager.openSession(nextRef);

    assert.equal(manager.size, 2);
    assert.equal(manager.get(runningRef.sessionId), running);
    assert.equal(running.turnActive, true);
    assert.equal(manager.get(nextRef.sessionId), opened);
    assert.equal(opened.connectionState.status, 'connected');
  } finally {
    SessionRuntime.prototype.start = originalStart;
    SessionRuntime.prototype.resumeOwnedSession = originalResume;
    await manager.dispose();
  }
});

test('openSession deduplicates concurrent opens by UUID and rejects a different project', async () => {
  const ref = { projectPath: '/workspace-a', sessionId: 'cccccccc-dddd-4eee-8fff-000000000000' };
  const gate = deferred<void>();
  const originalStart = SessionRuntime.prototype.start;
  const originalResume = SessionRuntime.prototype.resumeOwnedSession;
  let starts = 0;
  let resumes = 0;
  SessionRuntime.prototype.start = async function () {
    starts += 1;
    (this as any).state = { status: 'connected' };
    await gate.promise;
  };
  SessionRuntime.prototype.resumeOwnedSession = async function () {
    resumes += 1;
  };
  const manager = new SessionRuntimeManager({
    launchConfig: (candidate) => ({ workspace: candidate.projectPath, sessionId: candidate.sessionId, trusted: true }),
  });
  try {
    const first = manager.openSession(ref);
    const second = manager.openSession(ref);
    assert.strictEqual(first, second);
    assert.throws(
      () => manager.openSession({ ...ref, projectPath: '/workspace-b' }),
      /owned by a different project/,
    );
    assert.equal(starts, 1);
    assert.equal(resumes, 0);
    gate.resolve();
    const [firstRuntime, secondRuntime] = await Promise.all([first, second]);
    assert.strictEqual(firstRuntime, secondRuntime);
    assert.equal(resumes, 1);
  } finally {
    SessionRuntime.prototype.start = originalStart;
    SessionRuntime.prototype.resumeOwnedSession = originalResume;
    await manager.dispose();
  }
});

test('opening a validated empty session keeps its UUID runtime without sending resume_session', async () => {
  const sessionId = 'eeeeeeee-ffff-4000-8111-222222222222';
  const originalStart = SessionRuntime.prototype.start;
  const originalResume = SessionRuntime.prototype.resumeOwnedSession;
  let resumes = 0;
  SessionRuntime.prototype.start = async function () {
    (this as any).state = { status: 'connected' };
  };
  SessionRuntime.prototype.resumeOwnedSession = async function () { resumes += 1; };
  const manager = new SessionRuntimeManager({
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
  });
  try {
    const runtime = await manager.openSession({ projectPath: '/workspace', sessionId }, true);
    assert.equal(runtime.sessionId, sessionId);
    assert.equal(resumes, 0);
  } finally {
    SessionRuntime.prototype.start = originalStart;
    SessionRuntime.prototype.resumeOwnedSession = originalResume;
    await manager.dispose();
  }
});

test('closeProject removes runtimes from routing before asynchronous disposal', async () => {
  const projectPath = '/workspace-closing';
  const ref = { projectPath, sessionId: 'dddddddd-eeee-4fff-8000-111111111111' };
  const gate = deferred<void>();
  const originalDispose = SessionRuntime.prototype.dispose;
  SessionRuntime.prototype.dispose = async function () {
    await gate.promise;
  };
  const manager = new SessionRuntimeManager({
    launchConfig: (candidate) => ({ workspace: candidate.projectPath, sessionId: candidate.sessionId, trusted: true }),
  });
  try {
    const runtime = await manager.ensure(ref, false);
    (runtime as any).activeTurn = true;
    await assert.rejects(manager.closeProject(projectPath), /cancel active turns/);
    assert.strictEqual(manager.get(ref.sessionId), runtime);
    (runtime as any).activeTurn = false;
    const closing = manager.closeProject(projectPath);
    assert.equal(manager.get(ref.sessionId), undefined);
    assert.equal(manager.isProjectClosing(projectPath), true);
    assert.throws(() => manager.require(ref), /not open/);
    await assert.rejects(manager.newSession(projectPath), /project is closing/);
    assert.throws(() => manager.openSession({ projectPath, sessionId: 'eeeeeeee-ffff-4000-8111-222222222222' }), /project is closing/);
    assert.equal(runtime.projectPath, projectPath);
    gate.resolve();
    await closing;
    assert.equal(manager.isProjectClosing(projectPath), false);
  } finally {
    SessionRuntime.prototype.dispose = originalDispose;
    await manager.dispose();
  }
});

test('restart exposes an explicit restarting state and structured connection diagnostics', async () => {
  const diagnostics = new DiagnosticBuffer();
  const manager = new BridgeManager({
    diagnostics,
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const states: string[] = [];
  (manager as any).registerIpc = () => undefined;
  (manager as any).broadcast = (_channel: string, payload: { status?: string }) => {
    if (payload?.status) states.push(payload.status);
  };
  (manager as any).stopBridge = async () => undefined;
  (manager as any).startInternal = async () => {
    (manager as any).setState({ status: 'connected' });
  };

  await manager.restart();

  assert.deepEqual(states, ['restarting', 'connected']);
  const events = diagnostics.snapshot().map((entry) => JSON.parse(entry.message));
  assert.deepEqual(
    events.filter((entry) => entry.event === 'connection_state').map((entry) => entry.state.status),
    ['restarting', 'connected'],
  );
});

test('stop disconnects the active project and returns the bridge to idle', async () => {
  const manager = new BridgeManager({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const states: string[] = [];
  (manager as any).registerIpc = () => undefined;
  (manager as any).broadcast = (_channel: string, payload: { status?: string }) => {
    if (payload?.status) states.push(payload.status);
  };
  (manager as any).stopBridge = async () => undefined;

  await manager.stop();

  assert.deepEqual(states, ['idle']);
  assert.deepEqual(manager.connectionState, { status: 'idle' });
});

test('restart surfaces launch failures instead of remaining stuck in restarting', async () => {
  const diagnostics = new DiagnosticBuffer();
  const manager = new BridgeManager({
    diagnostics,
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const states: string[] = [];
  (manager as any).registerIpc = () => undefined;
  (manager as any).broadcast = (_channel: string, payload: { status?: string }) => {
    if (payload?.status) states.push(payload.status);
  };
  (manager as any).stopBridge = async () => undefined;
  (manager as any).startInternal = async () => {
    throw new Error('macOS login keychain is locked or access is denied (deepseek)');
  };

  await assert.rejects(manager.restart(), /login keychain is locked/);

  assert.deepEqual(states, ['restarting', 'error']);
  assert.deepEqual(manager.connectionState, {
    status: 'error',
    message: 'macOS login keychain is locked or access is denied (deepseek)',
  });
});

test('lockfile polling ignores invalid candidates until a valid private lockfile appears', async () => {
  const diagnostics = new DiagnosticBuffer();
  const workspace = temporaryDirectory();
  const launchDir = temporaryDirectory();
  const lockfilePath = join(launchDir, 'bridge.lock');
  const manager = new BridgeManager({
    diagnostics,
    lockfileTimeoutMs: 500,
    launchConfig: () => ({ workspace, trusted: true }),
  });
  (manager as any).generation = 1;
  (manager as any).child = { pid: process.pid };

  writeFileSync(lockfilePath, '{"pid":', { mode: 0o600 });
  chmodSync(lockfilePath, 0o600);
  setTimeout(() => {
    writeFileSync(lockfilePath, JSON.stringify({
      pid: process.pid,
      workspaceFolders: [workspace],
      ideName: 'LingXi-Bridge',
      transport: 'ws',
      runningInWindows: false,
      authToken: '0123456789abcdef0123456789abcdef',
    }), { mode: 0o600 });
    chmodSync(lockfilePath, 0o600);
  }, 75);

  try {
    const resolved = await (manager as any).waitForLockfile(launchDir, { workspace, trusted: true }, 1);
    assert.equal(resolved, lockfilePath);
    const ignored = diagnostics.snapshot().map((entry) => JSON.parse(entry.message))
      .find((entry) => entry.event === 'lockfile_ignored');
    assert.equal(ignored?.file, lockfilePath);
  } finally {
    rmSync(workspace, { recursive: true, force: true });
    rmSync(launchDir, { recursive: true, force: true });
  }
});

test('privileged bridge access re-checks current workspace trust before use', () => {
  let trusted = true;
  const client = { sendCommand: () => undefined };
  const manager = new BridgeManager({
    launchConfig: () => ({ workspace: '/workspace', trusted }),
  });
  (manager as any).client = client;
  (manager as any).activeWorkspace = '/workspace';
  (manager as any).activeWorkspaceTrusted = true;
  (manager as any).state = { status: 'connected' };

  assert.equal((manager as any).requireClient(), client);
  trusted = false;
  assert.throws(() => (manager as any).requireClient(), /workspace trust is required/);
});

test('computer access requests broadcast to renderers and are tracked as pending', async () => {
  const manager = new BridgeManager({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const broadcasts: Array<{ channel: string; payload: unknown }> = [];
  (manager as any).broadcast = (channel: string, payload: unknown) => broadcasts.push({ channel, payload });
  const handlers = new Map<string, (...args: unknown[]) => void>();
  const fakeClient = {
    on: (event: string, handler: (...args: unknown[]) => void) => { handlers.set(event, handler); return fakeClient; },
  };

  (manager as any).wireClient(fakeClient, 0);
  (manager as any).activeTurn = true;
  const request = {
    request_id: 5,
    reason: 'automate chat',
    apps: [{ label: 'Slack' }],
    tier: 'full',
    clipboard_read: false,
    clipboard_write: false,
    system_key_combos: false,
  };
  handlers.get('computerAccess')!(request);

  assert.deepEqual(broadcasts, [{ channel: 'lingxi:computerAccess', payload: request }]);
  assert.ok((manager as any).pendingComputerAccessIds.has(5));

  // stopBridge (restart/disconnect) clears pending computer access ids, just
  // like it clears pending permission ids.
  (manager as any).child = null;
  await (manager as any).stopBridge();
  assert.equal((manager as any).pendingComputerAccessIds.size, 0);
});

test('AskUserQuestion events are tracked and cleared across disconnect', async () => {
  const manager = new BridgeManager({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const broadcasts: Array<{ channel: string; payload: unknown }> = [];
  (manager as any).broadcast = (channel: string, payload: unknown) => broadcasts.push({ channel, payload });
  const handlers = new Map<string, (...args: unknown[]) => void>();
  const fakeClient = {
    on: (event: string, handler: (...args: unknown[]) => void) => { handlers.set(event, handler); return fakeClient; },
  };
  const event = {
    type: 'ask_user_question',
    request: {
      request_id: 7,
      questions: [{
        question: 'Choose a mode',
        header: 'Mode',
        options: [{ label: 'Safe', description: 'Keep safeguards enabled' }],
        multi_select: false,
      }],
    },
  };

  (manager as any).wireClient(fakeClient, 0);
  (manager as any).activeTurn = true;
  handlers.get('event')!(event);

  assert.deepEqual(broadcasts, [{ channel: 'lingxi:event', payload: event }]);
  assert.ok((manager as any).pendingAskUserQuestionIds.has(7));
  assert.deepEqual((manager as any).pendingAskUserQuestionRequests.get(7)?.request, event.request);
  assert.deepEqual(manager.pendingAskUserQuestions, [event.request]);

  (manager as any).child = null;
  await (manager as any).stopBridge();
  assert.equal((manager as any).pendingAskUserQuestionIds.size, 0);
  assert.equal((manager as any).pendingAskUserQuestionRequests.size, 0);
});

test('AskUserQuestion resolved events clear replay state before a renderer reload', () => {
  const manager = new BridgeManager({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const handlers = new Map<string, (...args: unknown[]) => void>();
  const fakeClient = {
    on: (event: string, handler: (...args: unknown[]) => void) => { handlers.set(event, handler); return fakeClient; },
  };

  (manager as any).wireClient(fakeClient, 0);
  (manager as any).activeTurn = true;
  handlers.get('event')!({
    type: 'ask_user_question',
    request: {
      request_id: 11,
      questions: [{
        question: 'Choose a mode',
        header: 'Mode',
        options: [{ label: 'Safe', description: 'Keep safeguards enabled' }],
        multi_select: false,
      }],
      timeout_secs: 60,
    },
  });
  handlers.get('event')!({
    type: 'ask_user_question_resolved',
    request_id: 11,
  });

  assert.equal((manager as any).pendingAskUserQuestionIds.has(11), false);
  assert.equal((manager as any).pendingAskUserQuestionRequests.has(11), false);
});

test('AskUserQuestion broker resolution clears replay state without a renderer answer', async () => {
  const manager = new BridgeManager({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const handlers = new Map<string, (...args: unknown[]) => void>();
  const fakeClient = {
    on: (event: string, handler: (...args: unknown[]) => void) => { handlers.set(event, handler); return fakeClient; },
  };

  (manager as any).broadcast = () => undefined;
  (manager as any).wireClient(fakeClient, 0);
  (manager as any).activeTurn = true;
  handlers.get('event')!({
    type: 'ask_user_question',
    request: {
      request_id: 12,
      timeout_secs: 0,
      questions: [{
        question: 'Choose a mode',
        header: 'Mode',
        options: [
          { label: 'Safe', description: 'Keep safeguards enabled' },
          { label: 'Fast', description: 'Move quicker' },
        ],
        multi_select: false,
      }],
    },
  });
  handlers.get('event')!({
    type: 'ask_user_question_resolved',
    request_id: 12,
  });

  assert.equal((manager as any).pendingAskUserQuestionIds.has(12), false);
  assert.equal((manager as any).pendingAskUserQuestionRequests.has(12), false);
});

test('registerWindow replays pending AskUserQuestion requests to a reloaded renderer', () => {
  const manager = new BridgeManager({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const handlers = new Map<string, (...args: unknown[]) => void>();
  const fakeClient = {
    on: (event: string, handler: (...args: unknown[]) => void) => { handlers.set(event, handler); return fakeClient; },
  };
  const sent: Array<{ channel: string; payload: unknown }> = [];
  const webContents = {
    send: (channel: string, payload: unknown) => sent.push({ channel, payload }),
    once: (_event: string, _handler: () => void) => undefined,
    isDestroyed: () => false,
  };
  const event = {
    type: 'ask_user_question',
    request: {
      request_id: 13,
      questions: [{
        question: 'Choose a mode',
        header: 'Mode',
        options: [{ label: 'Safe', description: 'Keep safeguards enabled' }],
        multi_select: false,
      }],
    },
  };

  (manager as any).wireClient(fakeClient, 0);
  (manager as any).activeTurn = true;
  handlers.get('event')!(event);

  manager.registerWindow(webContents as any, 'app://desktop/index.html');

  assert.deepEqual(sent, [{ channel: 'lingxi:event', payload: event }]);
});

test('old bridge generations cannot repopulate turn or permission state', () => {
  const manager = new BridgeManager({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const broadcasts: unknown[] = [];
  (manager as any).broadcast = (_channel: string, payload: unknown) => broadcasts.push(payload);
  (manager as any).generation = 2;
  const handlers = new Map<string, (...args: unknown[]) => void>();
  const staleClient = {
    on: (event: string, handler: (...args: unknown[]) => void) => { handlers.set(event, handler); return staleClient; },
  };

  (manager as any).wireClient(staleClient, 1);
  handlers.get('event')!({ type: 'turn_started', turn_id: 9 });
  handlers.get('permission')!({ request_id: 9, kind: { type: 'exit_plan_mode' } });

  assert.equal(manager.turnActive, false);
  assert.equal((manager as any).pendingPermissionIds.size, 0);
  assert.deepEqual(broadcasts, []);
});

test('turn terminal owns release and rejects late interactive or tool events', () => {
  const manager = new BridgeManager({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const broadcasts: Array<{ channel: string; payload: any }> = [];
  (manager as any).broadcast = (channel: string, payload: unknown) => broadcasts.push({ channel, payload });
  const handlers = new Map<string, (...args: unknown[]) => void>();
  const fakeClient = {
    on: (event: string, handler: (...args: unknown[]) => void) => { handlers.set(event, handler); return fakeClient; },
  };

  (manager as any).wireClient(fakeClient, 0);
  handlers.get('event')!({ type: 'turn_started', turn_id: 7 });
  handlers.get('permission')!({ request_id: 4, kind: { type: 'exit_plan_mode' } });
  handlers.get('event')!({ type: 'error', kind: { type: 'internal' }, message: 'non-terminal command error' });
  assert.equal(manager.turnActive, true, 'generic errors cannot release a running turn');
  assert.ok((manager as any).pendingPermissionIds.has(4));

  handlers.get('event')!({
    type: 'turn_ended',
    outcome: { type: 'cancelled' },
    cost: { total_usd: 0, input_tokens: 0, output_tokens: 0, api_calls: 0, session_duration_secs: 0, formatted: '' },
  });
  assert.equal(manager.turnActive, false);
  assert.equal((manager as any).pendingPermissionIds.size, 0);

  handlers.get('permission')!({ request_id: 5, kind: { type: 'exit_plan_mode' } });
  handlers.get('event')!({ type: 'tool_heartbeat', id: 'late', tool: 'WebSearch', elapsed_ms: 99_000 });
  handlers.get('event')!({
    type: 'ask_user_question',
    request: { request_id: 6, questions: [] },
  });

  assert.equal((manager as any).pendingPermissionIds.size, 0);
  assert.equal((manager as any).pendingAskUserQuestionIds.size, 0);
  assert.equal(
    broadcasts.some((entry) => entry.payload?.type === 'tool_heartbeat' && entry.payload?.id === 'late'),
    false,
  );
});

test('cancelling turn rejects late interactions but keeps turn events flowing', () => {
  const manager = new BridgeManager({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const broadcasts: Array<{ channel: string; payload: any }> = [];
  (manager as any).broadcast = (channel: string, payload: unknown) => broadcasts.push({ channel, payload });
  const handlers = new Map<string, (...args: unknown[]) => void>();
  const fakeClient = {
    on: (event: string, handler: (...args: unknown[]) => void) => { handlers.set(event, handler); return fakeClient; },
  };

  (manager as any).wireClient(fakeClient, 0);
  handlers.get('event')!({ type: 'turn_started', turn_id: 8 });
  (manager as any).cancellingTurn = true;
  handlers.get('permission')!({ request_id: 8, kind: { type: 'exit_plan_mode' } });
  handlers.get('computerAccess')!({ request_id: 9, reason: 'late', apps: [] });
  handlers.get('event')!({
    type: 'ask_user_question',
    request: { request_id: 10, questions: [] },
  });
  handlers.get('event')!({ type: 'tool_heartbeat', id: 'owned', tool: 'Bash', elapsed_ms: 17_000 });

  assert.equal(manager.turnActive, true, 'cancellation does not release the turn slot');
  assert.equal((manager as any).pendingPermissionIds.size, 0);
  assert.equal((manager as any).pendingComputerAccessIds.size, 0);
  assert.equal((manager as any).pendingAskUserQuestionIds.size, 0);
  assert.equal(
    broadcasts.some((entry) => entry.payload?.type === 'tool_heartbeat' && entry.payload?.id === 'owned'),
    true,
    'Block-tool heartbeats remain visible while stopping',
  );
});

test('prompt submission owns the pre-turn_started cancellation window', () => {
  const calls: Array<{ type: string; value?: unknown }> = [];
  const manager = new BridgeManager({
    accessState: () => ({ workspace: '/workspace', trusted: true }),
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const handlers = new Map<string, (...args: unknown[]) => void>();
  const fakeClient = {
    sendPrompt: (text: string, opts?: { images?: unknown[] }) => calls.push({ type: 'prompt', value: { text, images: opts?.images ?? [] } }),
    cancel: (turnId?: number) => calls.push({ type: 'cancel', value: turnId }),
    on: (event: string, handler: (...args: unknown[]) => void) => { handlers.set(event, handler); return fakeClient; },
  };
  (manager as any).client = fakeClient;
  (manager as any).activeWorkspace = '/workspace';
  (manager as any).wireClient(fakeClient, 0);

  (manager as any).sendPrompt('hello');
  assert.equal(manager.turnActive, true);
  (manager as any).cancelTurn(undefined);
  assert.equal((manager as any).cancellingTurn, true);

  handlers.get('event')!({ type: 'turn_started', turn_id: 91 });
  handlers.get('permission')!({ request_id: 91, kind: { type: 'exit_plan_mode' } });

  assert.equal((manager as any).cancellingTurn, true, 'turn_started must not undo an accepted cancel');
  assert.equal((manager as any).pendingPermissionIds.size, 0);
  assert.deepEqual(calls, [
    { type: 'prompt', value: { text: 'hello', images: [] } },
    { type: 'cancel', value: undefined },
  ]);
});

test('prompt submission forwards validated image attachments to the bridge client', () => {
  const calls: Array<{ text: string; images?: unknown[] }> = [];
  const manager = new BridgeManager({
    accessState: () => ({ workspace: '/workspace', trusted: true }),
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const fakeClient = {
    sendPrompt: (text: string, opts?: { images?: unknown[] }) => calls.push({ text, images: opts?.images }),
  };
  (manager as any).client = fakeClient;
  (manager as any).activeWorkspace = '/workspace';
  (manager as any).sendPrompt('describe this', [{
    media_type: 'image/png',
    base64: Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00]).toString('base64'),
  }]);
  assert.deepEqual(calls, [{
    text: 'describe this',
    images: [{
      media_type: 'image/png',
      base64: Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00]).toString('base64'),
    }],
  }]);
});

test('provider credential status is sourced from the engine secure store', async () => {
  const commands: Array<Record<string, unknown>> = [];
  const manager = new BridgeManager({
    providerIds: ['deepseek'],
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  (manager as any).client = { sendCommand: (command: Record<string, unknown>) => commands.push(command) };
  (manager as any).state = { status: 'connected' };
  (manager as any).broadcast = () => undefined;

  const pending = manager.listProviderCredentials(['deepseek']);
  const command = commands[0]!;
  assert.equal(command['type'], 'list_provider_credentials');
  assert.deepEqual(command['provider_ids'], ['deepseek']);

  (manager as any).handleProviderCredentialStatus({
    type: 'provider_credential_status',
    operation_id: command['operation_id'],
    configured_provider_ids: ['deepseek'],
    storage_encrypted: true,
  });

  await pending;
  assert.deepEqual(manager.persistedCredentialProviderIds, ['deepseek']);
  assert.deepEqual(manager.activeCredentialProviderIds, ['deepseek']);
  assert.equal(manager.providerCredentialStorageEncrypted, true);
});

test('provider credential writes cross only the authenticated bridge command path', async () => {
  const commands: Array<Record<string, unknown>> = [];
  const manager = new BridgeManager({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  (manager as any).client = { sendCommand: (command: Record<string, unknown>) => commands.push(command) };

  const pending = manager.setProviderCredential('deepseek', 'sk-test-secret');
  const command = commands[0]!;
  assert.equal(command['type'], 'set_provider_credential');
  assert.equal(command['provider_id'], 'deepseek');
  assert.equal(command['credential'], 'sk-test-secret');

  (manager as any).handleProviderCredentialStatus({
    type: 'provider_credential_status',
    operation_id: command['operation_id'],
    configured_provider_ids: ['deepseek'],
    storage_encrypted: true,
  });
  await pending;
});

test('partial credential read failures preserve unavailable providers and publish successful status', async () => {
  const commands: Array<Record<string, unknown>> = [];
  const broadcasts: unknown[] = [];
  const manager = new BridgeManager({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  (manager as any).client = { sendCommand: (command: Record<string, unknown>) => commands.push(command) };
  (manager as any).state = { status: 'connected' };
  (manager as any).persistedCredentialProviders.add('openrouter');
  (manager as any).broadcast = (_channel: string, payload: unknown) => broadcasts.push(payload);

  const pending = manager.listProviderCredentials(['deepseek', 'openrouter']);
  const command = commands[0]!;
  (manager as any).handleProviderCredentialStatus({
    type: 'provider_credential_status',
    operation_id: command['operation_id'],
    configured_provider_ids: ['deepseek'],
    unavailable_provider_ids: ['openrouter'],
    storage_encrypted: true,
    error: 'openrouter: macOS login keychain is locked',
  });

  await assert.rejects(pending, /login keychain is locked/);
  assert.deepEqual([...manager.persistedCredentialProviderIds].sort(), ['deepseek', 'openrouter']);
  assert.equal(broadcasts.length, 1, 'successful partial status must reach the renderer');
});

test('clean child exit clears the SIGKILL timer so the process group is not signalled twice', async () => {
  const manager = new BridgeManager({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
    stopTimeoutMs: 40,
  });
  const child = new EventEmitter() as EventEmitter & {
    exitCode: number | null;
    signalCode: NodeJS.Signals | null;
    pid: number;
  };
  child.exitCode = null;
  child.signalCode = null;
  child.pid = 4242;

  const signals: NodeJS.Signals[] = [];
  (manager as any).child = child;
  (manager as any).signalChildTree = (_child: unknown, signal: NodeJS.Signals) => {
    signals.push(signal);
  };

  const stopping = (manager as any).stopBridge();
  setTimeout(() => child.emit('exit', 0, null), 10);
  await stopping;
  await new Promise((resolve) => setTimeout(resolve, 80));

  assert.deepEqual(signals, ['SIGINT']);
});

// ── Bypass Permissions acceptance gate (security #34) ──────────────────────

function trustedManager(confirm?: () => Promise<boolean>): {
  manager: BridgeManager;
  commands: Array<Record<string, unknown>>;
} {
  const commands: Array<Record<string, unknown>> = [];
  const manager = new BridgeManager({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
    accessState: () => ({ workspace: '/workspace', trusted: true }),
    ...(confirm ? { confirmBypassPermissions: confirm } : {}),
  });
  (manager as any).client = {
    sendCommand: (command: Record<string, unknown>) => commands.push(command),
  };
  (manager as any).activeWorkspace = '/workspace';
  return { manager, commands };
}

test('bypassPermissions is refused when the acceptance dialog is declined', async () => {
  let confirmCalls = 0;
  const { manager, commands } = trustedManager(async () => {
    confirmCalls += 1;
    return false; // user picked Cancel
  });
  await assert.rejects(
    (manager as any).dispatchCommand({ type: 'set_permission_mode', mode: 'bypassPermissions' }),
    /not accepted/,
  );
  assert.equal(confirmCalls, 1, 'the acceptance confirmer is consulted');
  assert.equal(commands.length, 0, 'a declined bypass never reaches the engine');
});

test('bypassPermissions reaches the engine only after acceptance', async () => {
  const { manager, commands } = trustedManager(async () => true); // user accepted
  await (manager as any).dispatchCommand({ type: 'set_permission_mode', mode: 'bypassPermissions' });
  assert.equal(commands.length, 1);
  assert.equal(commands[0]!['type'], 'set_permission_mode');
  assert.equal(commands[0]!['mode'], 'bypassPermissions');
});

test('bypassPermissions is refused when no confirmer is wired (never one-click)', async () => {
  const { manager, commands } = trustedManager(); // no confirmBypassPermissions
  await assert.rejects(
    (manager as any).dispatchCommand({ type: 'set_permission_mode', mode: 'bypassPermissions' }),
    /not accepted/,
  );
  assert.equal(commands.length, 0);
});

test('non-bypass permission modes are not gated by the acceptance dialog', async () => {
  let confirmCalls = 0;
  const { manager, commands } = trustedManager(async () => {
    confirmCalls += 1;
    return true;
  });
  await (manager as any).dispatchCommand({ type: 'set_permission_mode', mode: 'acceptEdits' });
  assert.equal(confirmCalls, 0, 'only bypassPermissions consults the confirmer');
  assert.equal(commands.length, 1);
  assert.equal(commands[0]!['mode'], 'acceptEdits');
});
