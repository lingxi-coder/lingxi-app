import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { existsSync, mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

import type { ClientCommand } from '@lingxi/bridge-client';
import type { SessionRuntime } from '../src/main/bridge.js';
import { HostController } from '../src/main/host.js';
import { DiagnosticBuffer } from '../src/main/host-utils.js';
import { SessionRuntimeManager } from '../src/main/sessionRuntimeManager.js';
import { SettingsStore } from '../src/main/settings.js';
import { GitService } from '../src/main/git.js';

const sessionId = '11111111-2222-4333-8444-555555555555';
const secondSessionId = '22222222-3333-4444-8555-666666666666';
const activeWorkError = /active work|active turns?|background|pending interactions/i;

function deferred<T = void>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  const promise = new Promise<T>((complete) => { resolve = complete; });
  return { promise, resolve };
}

class FakeTransport extends EventEmitter {
  closes = 0;
  onCommand?: (command: ClientCommand) => void;

  sendCommand(command: ClientCommand): void { this.onCommand?.(command); }
  close(): void { this.closes += 1; }

  workerStarts(): void {
    this.emit('event', { type: 'coordinator_status', active_workers: 1 });
  }

  workersComplete(): void {
    this.emit('event', { type: 'coordinator_status', active_workers: 0 });
  }

  foregroundTurnEnds(): void {
    this.emit('event', { type: 'turn_started', turn_id: 1 });
    this.emit('event', { type: 'turn_ended', turn_id: 1 });
  }
}

async function openFakeRuntime(manager: SessionRuntimeManager, projectPath: string, id = sessionId) {
  const ref = { projectPath, sessionId: id };
  const runtime = await manager.ensure(ref, false);
  const transport = new FakeTransport();
  const internal = runtime as any;
  internal.activeWorkspace = projectPath;
  internal.activeWorkspaceTrusted = true;
  internal.client = transport;
  internal.wireClient(transport, internal.generation);
  internal.setState({ status: 'connected' });
  return { ref, runtime, transport };
}

function createManager(): SessionRuntimeManager {
  return new SessionRuntimeManager({
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
  });
}

async function hostFixture() {
  const directory = mkdtempSync(join(tmpdir(), 'lingxi-background-lifecycle-'));
  const projectDirectory = join(directory, 'project');
  const settingsDirectory = join(directory, 'settings');
  mkdirSync(projectDirectory);
  mkdirSync(settingsDirectory);
  const projectPath = realpathSync.native(projectDirectory);
  const settings = new SettingsStore(settingsDirectory);
  settings.addProject(projectPath);
  settings.activateProject(projectPath);
  const manager = createManager();
  const opened = await openFakeRuntime(manager, projectPath);
  settings.setActiveSession(opened.ref);
  const catalog = {
    find: async () => undefined,
    list: async () => ({ sessions: [] }),
    invalidate: () => undefined,
  };
  const host = new HostController(settings, manager, new DiagnosticBuffer(), catalog as any, {
    handle: () => undefined,
    removeHandler: () => undefined,
  } as any);
  (host as any).bootstrap = () => ({ settings: settings.getPublic() });
  const teardown: string[] = [];
  const replacements: string[] = [];
  manager.newSession = async (project) => {
    replacements.push(project);
    return { projectPath: project, sessionId: secondSessionId };
  };
  (host as any).terminals = {
    closeScope: async () => { teardown.push('session terminals'); },
    closeProject: async () => { teardown.push('project terminals'); },
  };
  return {
    ...opened, projectPath, settingsDirectory, settings, manager, host, catalog, teardown, replacements,
    cleanup: async () => {
      await manager.dispose();
      rmSync(directory, { recursive: true, force: true });
    },
  };
}

function assertRuntimePreserved(manager: SessionRuntimeManager, runtime: SessionRuntime, transport: FakeTransport): void {
  assert.strictEqual(manager.get(runtime.sessionId), runtime);
  assert.equal(runtime.connectionState.status, 'connected');
  assert.equal(transport.closes, 0, 'a rejected lifecycle operation must keep the engine channel open');
}

test('coordinator workers keep a project active after its foreground turn ends', async () => {
  const manager = createManager();
  const { runtime, transport } = await openFakeRuntime(manager, '/background-project');
  try {
    transport.workerStarts();
    transport.foregroundTurnEnds();
    assert.equal(runtime.turnActive, false);
    assert.equal(runtime.hasActiveAgents, true);
    assert.equal(manager.hasActiveWork('/background-project'), true);
    assert.equal(manager.hasActiveWork('/unrelated-project'), false);
    transport.workersComplete();
    assert.equal(manager.hasActiveWork('/background-project'), false);
  } finally { await manager.dispose(); }
});

test('archiving refuses a running coordinator worker and succeeds after workers complete', async () => {
  const manager = createManager();
  const { runtime, transport } = await openFakeRuntime(manager, '/background-project');
  try {
    transport.workerStarts();
    transport.foregroundTurnEnds();
    assert.throws(() => runtime.beginArchive(), activeWorkError);
    assertRuntimePreserved(manager, runtime, transport);
    transport.workersComplete();
    const release = runtime.beginArchive();
    release();
  } finally { await manager.dispose(); }
});

test('closing a session preserves its running worker and closes only after worker completion', async () => {
  const manager = createManager();
  const { ref, runtime, transport } = await openFakeRuntime(manager, '/background-project');
  try {
    transport.workerStarts();
    transport.foregroundTurnEnds();
    await assert.rejects(manager.closeSession(ref), activeWorkError);
    assertRuntimePreserved(manager, runtime, transport);
    transport.workersComplete();
    await manager.closeSession(ref);
    assert.equal(manager.get(ref.sessionId), undefined);
    assert.equal(transport.closes, 1);
  } finally { await manager.dispose(); }
});

test('closing a project preserves every runtime when one background worker is active', async () => {
  const manager = createManager();
  const busy = await openFakeRuntime(manager, '/background-project');
  const idle = await openFakeRuntime(manager, '/background-project', secondSessionId);
  try {
    busy.transport.workerStarts();
    busy.transport.foregroundTurnEnds();
    await assert.rejects(manager.closeProject('/background-project'), activeWorkError);
    assertRuntimePreserved(manager, busy.runtime, busy.transport);
    assertRuntimePreserved(manager, idle.runtime, idle.transport);
    busy.transport.workersComplete();
    await manager.closeProject('/background-project');
    assert.equal(manager.size, 0);
    assert.equal(busy.transport.closes, 1);
    assert.equal(idle.transport.closes, 1);
  } finally { await manager.dispose(); }
});

test('project removal rechecks workers after awaiting scheduled task cleanup', async () => {
  const fixture = await hostFixture();
  const entered = deferred();
  const resume = deferred<any[]>();
  (fixture.host as any).scheduled = {
    manage: async () => { entered.resolve(); return resume.promise; },
  };
  try {
    const removal = (fixture.host as any).removeProjectInternal(fixture.projectPath);
    const rejected = assert.rejects(removal, activeWorkError);
    await entered.promise;
    fixture.transport.workerStarts();
    resume.resolve([]);
    await rejected;
    assert.equal(fixture.settings.hasProject(fixture.projectPath), true);
    assert.equal(new SettingsStore(fixture.settingsDirectory).hasProject(fixture.projectPath), true);
    assert.deepEqual(fixture.teardown, [], 'terminals must survive a worker appearing during cleanup');
    assertRuntimePreserved(fixture.manager, fixture.runtime, fixture.transport);
  } finally { resume.resolve([]); await fixture.cleanup(); }
});

test('session clear rechecks workers after awaiting the previous conversation catalog', async () => {
  const fixture = await hostFixture();
  const entered = deferred();
  const resume = deferred<undefined>();
  fixture.catalog.find = async () => { entered.resolve(); return resume.promise; };
  try {
    const clearing = (fixture.host as any).clearActiveSession(sessionId, 'Previous conversation');
    const rejected = assert.rejects(clearing, activeWorkError);
    await entered.promise;
    fixture.transport.workerStarts();
    resume.resolve(undefined);
    await rejected;
    assert.deepEqual(fixture.settings.getPublic().activeSession, fixture.ref);
    assert.deepEqual(new SettingsStore(fixture.settingsDirectory).getPublic().activeSession, fixture.ref);
    assert.deepEqual(fixture.teardown, []);
    assert.deepEqual(fixture.replacements, []);
    assertRuntimePreserved(fixture.manager, fixture.runtime, fixture.transport);
  } finally { resume.resolve(undefined); await fixture.cleanup(); }
});

test('archive rechecks workers after cron cleanup before persisting its archive marker', async () => {
  const fixture = await hostFixture();
  const entered = deferred();
  const resume = deferred();
  fixture.transport.onCommand = (command) => {
    if (command.type !== 'cron_manage') throw new Error(`unexpected command ${command.type}`);
    assert.equal(command.request.action, 'list');
    entered.resolve();
    void resume.promise.then(() => fixture.transport.emit('event', {
      type: 'cron_result', request_id: command.request_id, jobs: [],
    }));
  };
  try {
    const archiving = (fixture.host as any).archiveSessionInternal(fixture.ref);
    const rejected = assert.rejects(archiving, /Chat was not archived.*(?:active work|active turn|background|pending interactions)/i);
    await entered.promise;
    fixture.transport.workerStarts();
    resume.resolve();
    await rejected;
    assert.equal(fixture.settings.isSessionArchived(fixture.ref), false);
    assert.equal(new SettingsStore(fixture.settingsDirectory).isSessionArchived(fixture.ref), false);
    assert.deepEqual(fixture.settings.getPublic().activeSession, fixture.ref);
    assert.deepEqual(new SettingsStore(fixture.settingsDirectory).getPublic().activeSession, fixture.ref);
    assert.deepEqual(fixture.teardown, []);
    assert.deepEqual(fixture.replacements, []);
    assertRuntimePreserved(fixture.manager, fixture.runtime, fixture.transport);
    fixture.transport.workersComplete();
    const release = fixture.runtime.beginArchive();
    release();
  } finally { resume.resolve(); await fixture.cleanup(); }
});

for (const taskType of ['local_bash', 'local_workflow', 'local_fusion']) {
  test(`${taskType} blocks archive, close, restart and real Git discard after the foreground ends`, async () => {
    const fixture = await hostFixture();
    const git = new GitService({ isBusy: () => fixture.manager.hasActiveWork(fixture.projectPath) });
    const output = join(fixture.projectPath, 'in-progress-output.txt');
    try {
      await git.request(fixture.ref, { kind: 'init' });
      writeFileSync(output, 'work in progress');
      fixture.transport.emit('event', { type: 'task_row', task: {
        task_id: 'background-task', task_type: taskType, description: 'Writes project output', status: { type: 'running' },
      } });
      fixture.transport.foregroundTurnEnds();
      assert.equal(fixture.runtime.turnActive, false);
      assert.equal(fixture.manager.hasActiveWork(fixture.projectPath), true);
      assert.throws(() => fixture.runtime.beginArchive(), activeWorkError);
      await assert.rejects(fixture.manager.closeSession(fixture.ref), activeWorkError);
      await assert.rejects(fixture.manager.restart(fixture.ref), activeWorkError);
      const status = (await git.request(fixture.ref, { kind: 'status' })).status!;
      assert.equal(status.busy, true);
      await assert.rejects(git.request(fixture.ref, {
        kind: 'discard', paths: ['in-progress-output.txt'], untracked: true, token: status.token,
      }), /agent/i);
      assert.equal(existsSync(output), true);
      assertRuntimePreserved(fixture.manager, fixture.runtime, fixture.transport);
      fixture.transport.emit('event', { type: 'task_status_changed', task_id: 'background-task', status: { type: 'completed' } });
      assert.equal(fixture.manager.hasActiveWork(fixture.projectPath), false);
      const release = fixture.runtime.beginArchive(); release();
      const idle = (await git.request(fixture.ref, { kind: 'status' })).status!;
      await git.request(fixture.ref, { kind: 'discard', paths: ['in-progress-output.txt'], untracked: true, token: idle.token });
      assert.equal(existsSync(output), false);
      await fixture.manager.closeSession(fixture.ref);
      assert.equal(fixture.transport.closes, 1);
    } finally { git.dispose(); await fixture.cleanup(); }
  });
}

for (const source of ['task', 'coordinator'] as const) test(`${source} process events pin an inactive runtime and completion triggers cache eviction`, async () => {
  const manager = new SessionRuntimeManager({ maxCachedRuntimes: 1,
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }) });
  const busy = await openFakeRuntime(manager, '/background-project');
  const releaseOther = manager.retainBackgroundSession({ projectPath: '/other-project', sessionId: secondSessionId });
  try {
    const events: string[] = [];
    const broadcast = (busy.runtime as any).broadcastClientEvent.bind(busy.runtime);
    (busy.runtime as any).broadcastClientEvent = (event: { type: string }) => { events.push(event.type); broadcast(event); };
    if (source === 'task') busy.transport.emit('event', { type: 'task_lifecycle', event_json: JSON.stringify({
      type: 'system', subtype: 'task_started', task_id: 'background-shell', task_type: 'local_bash',
    }) });
    else busy.transport.workerStarts();
    const other = await openFakeRuntime(manager, '/other-project', secondSessionId);
    await manager.openSession(other.ref, true);
    releaseOther();
    assert.equal(manager.size, 2, 'a task keeps the inactive process above the idle-cache limit');
    assertRuntimePreserved(manager, busy.runtime, busy.transport);
    if (source === 'task') busy.transport.emit('event', { type: 'task_lifecycle', event_json: JSON.stringify({
      type: 'system', subtype: 'task_updated', task_id: 'background-shell', patch: { status: 'completed' },
    }) });
    else busy.transport.workersComplete();
    assert.equal(events.filter((type) => type === (source === 'task' ? 'task_lifecycle' : 'coordinator_status')).length, 2,
      'process events reach the renderer without a foreground owner');
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(manager.get(busy.ref.sessionId), undefined, 'terminal task activity schedules cache trimming');
    assert.strictEqual(manager.get(other.ref.sessionId), other.runtime);
    assert.equal(busy.transport.closes, 1);
  } finally { releaseOther(); await manager.dispose(); }
});
