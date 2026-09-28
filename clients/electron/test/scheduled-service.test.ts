import assert from 'node:assert/strict';
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test, type TestContext } from 'node:test';
import type { CronAutomationDto, CronJobDto, CronRequestDto } from '@lingxi/bridge-client';
import { ScheduledTaskService } from '../src/main/scheduled';
import type { SessionRuntime } from '../src/main/bridge.js';
import type { SessionRuntimeManager } from '../src/main/sessionRuntimeManager.js';
import type { SettingsStore } from '../src/main/settings';
import type { ProjectSessionCatalog } from '../src/main/session-catalog';
import type { SessionRef } from '../src/shared/settings';

const targetId = '11111111-2222-4333-8444-555555555555';
function task(patch: Partial<CronAutomationDto> = {}): CronJobDto {
  return { id: 'task-1', cron: '0 9 * * *', prompt: '# Morning brief\n\nSummarize the project.', recurring: true, durable: true, permanent: false, created_at: 1,
    automation: { version: 2, status: 'active', model: 'openai/test-model', reasoning: { type: 'level', id: 'high' }, runMode: 'new_session', notificationPolicy: 'all', ...patch } };
}
function deferred<T = void>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}
async function harness(t: TestContext) {
  const directory = await mkdtemp(join(tmpdir(), 'lingxi-scheduled-service-'));
  const project = join(directory, 'project');
  const global = join(directory, 'managed-scheduled-workspace');
  const projects = new Set([project]);
  const archived = new Set<string>();
  const catalogRows = new Map<string, { uuid: string; title: string }[]>([[project, [{ uuid: targetId, title: 'Chosen chat' }]], [global, []]]);
  const ensured: { ref: SessionRef; empty: boolean; model?: string }[] = [];
  const background: { ref: SessionRef; empty: boolean; model?: string }[] = [];
  const turns: { runId: string; task: CronJobDto; ref: SessionRef }[] = [];
  const requests: CronRequestDto[] = [];
  const bindings: {runId: string; sessionId: string}[] = [];
  const leases: { ref: SessionRef; released: boolean }[] = [];
  const notifications: { title: string; body: string; ref?: SessionRef }[] = [];
  const errors: unknown[] = [];
  let execute = async (_runId: string, _task: CronJobDto) => 'A completed summary';
  let ensureFailure: Error | undefined;
  const runtime = (ref: SessionRef) => ({ projectPath: ref.projectPath,
    markCronRunStarted: async (runId: string, sessionId: string) => { bindings.push({ runId, sessionId }); },
    manageCron: async (request: CronRequestDto) => { requests.push(request); return []; },
    scheduledModelCatalog: async () => ({ details: [], current: 'openai/project-model' }),
    runScheduledTurn: async (runId: string, saved: CronJobDto, beforeStart?: () => Promise<void>) => { await beforeStart?.(); turns.push({ runId, task: saved, ref }); return execute(runId, saved); },
  }) as unknown as SessionRuntime;
  const settings = {
    scheduledWorkspace: global,
    settingsPath: join(directory, 'settings.v1.json'),
    getPublic: () => ({ projects: [...projects], model: 'openai/default' }),
    hasProject: (path: string) => projects.has(path),
    isTrustedWorkspace: (path: string) => path === global || projects.has(path),
    isSessionArchived: (ref: SessionRef) => archived.has(`${ref.projectPath}:${ref.sessionId}`),
  } as unknown as SettingsStore;
  const manager = {
    retainBackgroundSession: (ref: SessionRef) => { const lease = { ref, released: false }; leases.push(lease); return () => { lease.released = true; }; },
    ensure: async (ref: SessionRef, empty: boolean, model?: string) => { ensured.push({ ref, empty, model }); if (ensureFailure) throw ensureFailure; return runtime(ref); },
    withBackgroundSession: async <T>(ref: SessionRef, empty: boolean, model: string | undefined, operation: (runtime: SessionRuntime) => Promise<T>) => {
      background.push({ ref, empty, model }); return operation(runtime(ref));
    },
    activate: () => assert.fail('Scheduled tasks must not activate a foreground session'),
    newSession: () => assert.fail('Scheduled tasks must not create foreground drafts'),
  } as unknown as SessionRuntimeManager;
  const catalog = { list: async (path: string) => ({ sessions: catalogRows.get(path) ?? [] }) } as unknown as ProjectSessionCatalog;
  const createService = () => new ScheduledTaskService(settings, manager, catalog, (title, body, ref) => notifications.push({ title, body, ref }), (error) => errors.push(error));
  const service = createService();
  t.after(async () => { service.dispose(); await rm(directory, { recursive: true, force: true }); });
  return { directory, project, global, projects, archived, catalogRows, ensured, background, turns, requests, leases, notifications, errors, bindings, service, createService,
    source: runtime({ projectPath: project, sessionId: targetId }),
    setExecute: (callback: typeof execute) => { execute = callback; },
    failEnsure: (error: Error) => { ensureFailure = error; },
  };
}

test('scheduled scopes isolate no-project workspace and reject arbitrary paths before acquiring a runtime', async (t) => {
  const h = await harness(t);
  assert.equal(h.service.resolveScope('global'), h.global);
  assert.notEqual(h.global, h.project);
  assert.notEqual(h.global, process.cwd());
  assert.deepEqual(h.service.scopes().map((scope) => scope.id), ['global', h.project]);
  await assert.rejects(h.service.manage(join(h.directory, 'unauthorized'), { action: 'list' }), /unavailable/);
  assert.equal(h.ensured.length, 0);
  await h.service.manage('global', { action: 'list' });
  assert.equal(h.ensured[0].ref.projectPath, h.global);
  assert.equal(h.requests[0].action, 'list');
  assert.equal(h.projects.has(h.global), false, 'managed workspace is not added as a user project');
});

test('new-session runs use unique background sessions and forward the fixed model/effort snapshot', async (t) => {
  const h = await harness(t);
  const saved = task();
  const first = await h.service.run(h.source, 'run-a', saved);
  const second = await h.service.run(h.source, 'run-b', saved);
  assert.notEqual(first.sessionId, second.sessionId);
  assert.deepEqual(h.background.map((item) => [item.ref.projectPath, item.empty, item.model]), [[h.project, true, saved.automation!.model], [h.project, true, saved.automation!.model]]);
  assert.deepEqual(h.bindings, [{runId: 'run-a', sessionId: first.sessionId}, {runId: 'run-b', sessionId: second.sessionId}]);
  assert.equal(h.turns[0].task, saved, 'the per-run snapshot is delivered unchanged');
  assert.deepEqual(h.turns[0].task.automation?.reasoning, { type: 'level', id: 'high' });
  assert.deepEqual(h.turns.map((turn) => turn.runId), ['run-a', 'run-b']);
  assert.equal(h.notifications.length, 2);
  assert.equal(h.notifications[0].ref?.sessionId, first.sessionId);
});

test('selected-session runs serialize and revalidate a queued target after archive', async (t) => {
  const h = await harness(t);
  const entered = deferred(); const finish = deferred<string>();
  h.setExecute(async () => { entered.resolve(); return finish.promise; });
  const saved = task({ runMode: 'selected_session', targetSessionId: targetId });
  const first = h.service.run(h.source, 'first', saved);
  await entered.promise;
  const second = h.service.run(h.source, 'second', saved);
  const rejected = assert.rejects(second, /archived/);
  await new Promise<void>((resolve) => setImmediate(resolve));
  assert.equal(h.turns.length, 1, 'second run must not overlap the first');
  h.archived.add(`${h.project}:${targetId}`);
  finish.resolve('Finished first run');
  await first; await rejected;
  assert.equal(h.turns.length, 1, 'archived target must not receive the queued turn');
  assert.equal(h.background[0].empty, false);
});

test('selected-session creation rejects malformed, missing and archived targets', async (t) => {
  const h = await harness(t);
  for (const id of ['../invalid', '22222222-3333-4444-8555-666666666666']) {
    await assert.rejects(h.service.manage(h.project, { action: 'create', automation: task({ runMode: 'selected_session', targetSessionId: id }).automation }), /target chat|unavailable/);
  }
  h.archived.add(`${h.project}:${targetId}`);
  await assert.rejects(h.service.manage(h.project, { action: 'create', automation: task({ runMode: 'selected_session', targetSessionId: targetId }).automation }), /archived/);
  assert.equal(h.ensured.length, 0);
});

test('dedicated task chat reuses persisted ownership and never recreates a missing owner', async (t) => {
  const h = await harness(t);
  const first = await h.service.run(h.source, 'first', task({ runMode: 'task_session' }));
  h.catalogRows.get(h.project)!.push({ uuid: first.sessionId, title: 'Dedicated task' });
  const saved = task({ runMode: 'task_session', ownedSessionId: first.sessionId });
  const second = await h.service.run(h.source, 'second', saved);
  assert.equal(second.sessionId, first.sessionId);
  assert.equal(h.background[0].empty, true);
  assert.equal(h.background[1].empty, false);
  h.catalogRows.set(h.project, []);
  await assert.rejects(h.service.run(h.source, 'third', saved), /unavailable/);
  assert.equal(h.background.length, 2);
});

test('notification policy reports each success/failure once and suppresses disabled notifications', async (t) => {
  const h = await harness(t);
  for (const policy of ['all', 'failed', 'none'] as const) {
    h.notifications.length = 0;
    h.setExecute(async () => 'summary');
    await h.service.run(h.source, `${policy}-success`, task({ notificationPolicy: policy }));
    assert.equal(h.notifications.length, policy === 'all' ? 1 : 0);
    h.notifications.length = 0;
    h.setExecute(async () => { throw new Error('model request failed'); });
    await assert.rejects(h.service.run(h.source, `${policy}-failed`, task({ notificationPolicy: policy })), /model request failed/);
    assert.equal(h.notifications.length, policy === 'none' ? 0 : 1);
  }
});

test('configuration failures notify according to failure policy before execution starts', async (t) => {
  const h = await harness(t);
  h.archived.add(`${h.project}:${targetId}`);
  await assert.rejects(h.service.run(h.source, 'archived', task({ runMode: 'selected_session', targetSessionId: targetId, notificationPolicy: 'failed' })), /archived/);
  assert.equal(h.background.length, 0);
  assert.equal(h.notifications.length, 1);
  h.projects.delete(h.project);
  await assert.rejects(h.service.run(h.source, 'removed', task({ notificationPolicy: 'none' })), /unavailable/);
  assert.equal(h.notifications.length, 1, 'disabled notifications stay silent on configuration failure');
});

test('context loads scoped model catalog and omits archived chats', async (t) => {
  const h = await harness(t);
  h.archived.add(`${h.project}:${targetId}`);
  const context = await h.service.context(h.project);
  assert.equal(context.currentModel, 'openai/project-model');
  assert.deepEqual(context.sessions, []);
  assert.equal(h.ensured[0].ref.projectPath, h.project);
});

test('discovery keeps active scopes alive and releases controllers for removed projects', async (t) => {
  const h = await harness(t);
  await mkdir(join(h.project, '.lingxi'), { recursive: true });
  await writeFile(join(h.project, '.lingxi', 'scheduled_tasks.json'), JSON.stringify({ tasks: [{ automation: { status: 'active' } }] }));
  await h.service.sync();
  assert.equal(h.leases.length, 1);
  assert.equal(h.leases[0].released, false);
  h.projects.delete(h.project);
  await h.service.sync();
  assert.equal(h.leases[0].released, true);
  assert.deepEqual(h.errors, []);
});

test('failed controller startup releases its lease and disposed services reject new controllers', async (t) => {
  const h = await harness(t);
  h.failEnsure(new Error('engine failed to start'));
  await assert.rejects(h.service.manage(h.project, { action: 'list' }), /failed to start/);
  assert.equal(h.leases[0].released, true);
  h.service.dispose();
  await assert.rejects(h.service.manage(h.project, { action: 'list' }), /closed/);
});

test('notification delivery suppresses duplicate run IDs across service restarts', async (t) => {
  const h = await harness(t);
  await h.service.run(h.source, 'same-occurrence', task());
  await h.service.run(h.source, 'same-occurrence', task());
  assert.equal(h.notifications.length, 1);
  h.service.dispose();
  const reopened = h.createService();
  t.after(() => reopened.dispose());
  await reopened.run(h.source, 'same-occurrence', task());
  assert.equal(h.notifications.length, 1, 'delivered notification reservation survives restart');
  await reopened.run(h.source, 'next-occurrence', task());
  assert.equal(h.notifications.length, 2);
});

test('history retention prunes oldest terminal records across scopes without pruning active runs', async (t) => {
  const h = await harness(t);
  const records = (count: number, offset: number) => Array.from({ length: count }, (_, index) => ({ id: `run-${offset + index}`, taskId: 'task-1', scheduledAt: offset + index, finishedAt: offset + index, status: 'succeeded' as const, model: 'openai/test-model', reasoning: { type: 'automatic' as const } }));
  for (const [path, runs] of [[h.project, records(251, 0)], [h.global, records(251, 251)]] as const) {
    await mkdir(join(path, '.lingxi'), { recursive: true });
    await writeFile(join(path, '.lingxi', 'scheduled_tasks.json'), JSON.stringify({ tasks: [{ ...task(), automation: { ...task().automation, runs: [...runs, { id: `running-${path}`, taskId: 'task-1', scheduledAt: 0, status: 'running', model: 'openai/test-model', reasoning: { type: 'automatic' } }] } }] }));
  }
  await h.service.sync();
  const pruning = h.requests.filter((request) => request.action === 'prune_history');
  assert.equal(pruning.length, 1);
  assert.deepEqual(pruning[0].automation?.runs?.map((run) => run.id), ['run-1', 'run-0']);
  assert.equal(pruning[0].automation?.runs?.some((run) => run.status === 'running'), false);
  assert.deepEqual(h.errors, []);
});

test('pausing an invalid selected target still reaches the lifecycle endpoint', async (t) => {
  const h = await harness(t);
  h.archived.add(`${h.project}:${targetId}`);
  await h.service.manage(h.project, { action: 'pause', id: 'task-1', automation: { ...task({ runMode: 'selected_session', targetSessionId: targetId }).automation!, statusReason: 'Target chat was archived.' } });
  const change = h.requests.find((request) => request.action === 'pause');
  assert.equal(change?.automation?.statusReason, 'Target chat was archived.');
});
