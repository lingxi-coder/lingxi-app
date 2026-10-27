import assert from 'node:assert/strict';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, dirname, join } from 'node:path';
import { test, type TestContext } from 'node:test';
import type { CronAutomationDto, CronJobDto, CronRequestDto } from '@lingxi/bridge-client';
import { ScheduledTaskService, type RoutineBot, type RoutineBots, type RoutineCatalog } from '../src/main/scheduled';
import type { SessionRuntime } from '../src/main/bridge.js';
import type { SessionRuntimeManager } from '../src/main/sessionRuntimeManager.js';
import type { SettingsStore } from '../src/main/settings';
import type { ProjectSessionCatalog } from '../src/main/session-catalog';
import type { SessionRef } from '../src/shared/settings';

const targetId = '11111111-2222-4333-8444-555555555555';
function claimWireId(occurrenceId: string, generation = 1): string {
  return `lingxi-cron-claim-v1:${JSON.stringify([occurrenceId, generation])}`;
}
function task(patch: Partial<CronAutomationDto> = {}): CronJobDto {
  return { id: 'task-1', cron: '0 9 * * *', prompt: '# Morning brief\n\nSummarize the project.', recurring: true, durable: true, permanent: false, created_at: 1,
    automation: { version: 2, status: 'active', model: 'openai/test-model', reasoning: { type: 'level', id: 'high' }, runMode: 'new_session', notificationPolicy: 'all', ...patch } };
}
function taskForClaim(saved: CronJobDto, occurrenceId: string, generation = 1): CronJobDto {
  const automation = saved.automation!;
  return {
    ...saved,
    automation: {
      ...automation,
      runs: [{ id: occurrenceId, taskId: saved.id, claimGeneration: generation, scheduledAt: 1,
        status: 'running' as const, model: automation.model, reasoning: automation.reasoning }],
    },
  };
}
function runClaim(service: ScheduledTaskService, source: SessionRuntime, occurrenceId: string, saved: CronJobDto, generation = 1) {
  return service.run(source, claimWireId(occurrenceId, generation), taskForClaim(saved, occurrenceId, generation));
}
async function writeCatalog(workspace: string, tasks: unknown[]): Promise<void> {
  await mkdir(join(workspace, '.lingxi'), { recursive: true });
  await writeFile(join(workspace, '.lingxi', 'scheduled_tasks.json'), JSON.stringify({ tasks }));
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
  let managed: CronJobDto[] = [];
  const runtime = (ref: SessionRef) => ({ projectPath: ref.projectPath,
    markCronRunStarted: async (runId: string, sessionId: string) => { bindings.push({ runId, sessionId }); },
    manageCron: async (request: CronRequestDto) => { requests.push(request); return managed; },
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
  const catalog = {
    list: async (path: string) => ({ sessions: (catalogRows.get(path) ?? []).slice(0, 200) }),
    find: async (path: string, sessionId: string) => catalogRows.get(path)?.find((row) => row.uuid === sessionId),
  } as unknown as ProjectSessionCatalog;
  const createService = () => new ScheduledTaskService(settings, manager, catalog, (title, body, ref) => notifications.push({ title, body, ref }), (error) => errors.push(error));
  const service = createService();
  t.after(async () => { service.dispose(); await rm(directory, { recursive: true, force: true }); });
  return { directory, project, global, projects, archived, catalogRows, ensured, background, turns, requests, leases, notifications, errors, bindings, service, createService,
    source: runtime({ projectPath: project, sessionId: targetId }),
    runtime,
    setManaged: (jobs: CronJobDto[]) => { managed = jobs; },
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
  await writeCatalog(h.global, [task()]);
  await h.service.manage('global', { action: 'list' });
  assert.equal(h.ensured[0].ref.projectPath, h.global);
  assert.equal(h.requests[0].action, 'list');
  assert.equal(h.projects.has(h.global), false, 'managed workspace is not added as a user project');
});

test('listing a scope with no persisted catalog answers without starting an engine', async (t) => {
  const h = await harness(t);
  assert.deepEqual(await h.service.manage(h.project, { action: 'list' }), []);
  assert.deepEqual(await h.service.manage(h.project, { action: 'history', id: 'task-1' }), []);
  assert.deepEqual(await h.service.manage('global', { action: 'list' }), []);
  assert.equal(h.ensured.length, 0, 'an empty scope must not lease a controller runtime');
  assert.equal(h.leases.length, 0);
  assert.deepEqual(h.errors, []);
});

test('an unreadable catalog body is not reported as an empty scope', async (t) => {
  const h = await harness(t);
  await mkdir(join(h.project, '.lingxi'), { recursive: true });
  await writeFile(join(h.project, '.lingxi', 'scheduled_tasks.json'), JSON.stringify({ tasks: {} }));
  await h.service.manage(h.project, { action: 'list' });
  assert.equal(h.ensured.length, 1, 'a body without a tasks array must reach the engine, which reports the corruption');
  assert.equal(h.requests[0].action, 'list');
});

test('an emptied catalog retires the cached jobs of that scope', async (t) => {
  const h = await harness(t);
  await writeFile(join(h.directory, 'scheduled-scopes.json'), JSON.stringify([{ path: h.project, label: 'project', jobs: [task()] }]));
  const service = h.createService();
  t.after(() => service.dispose());
  assert.deepEqual(await service.manage(h.project, { action: 'list' }), []);
  h.projects.delete(h.project);
  assert.equal(service.scopes().some((scope) => scope.unavailable), false, 'an emptied scope must not be advertised as unavailable with stale jobs');
});

test('new-session runs use unique background sessions and forward the fixed model/effort snapshot', async (t) => {
  const h = await harness(t);
  const saved = task();
  const firstTask = taskForClaim(saved, 'run-a');
  const secondTask = taskForClaim(saved, 'run-b');
  const first = await h.service.run(h.source, claimWireId('run-a'), firstTask);
  const second = await h.service.run(h.source, claimWireId('run-b'), secondTask);
  assert.notEqual(first.sessionId, second.sessionId);
  assert.deepEqual(h.background.map((item) => [item.ref.projectPath, item.empty, item.model]), [[h.project, true, saved.automation!.model], [h.project, true, saved.automation!.model]]);
  assert.deepEqual(h.bindings, [{runId: claimWireId('run-a'), sessionId: first.sessionId}, {runId: claimWireId('run-b'), sessionId: second.sessionId}]);
  assert.equal(h.turns[0].task, firstTask, 'the per-run claim snapshot is delivered unchanged');
  assert.deepEqual(h.turns[0].task.automation?.reasoning, { type: 'level', id: 'high' });
  assert.deepEqual(h.turns.map((turn) => turn.runId), [claimWireId('run-a'), claimWireId('run-b')]);
  assert.equal(h.notifications.length, 2);
  assert.equal(h.notifications[0].ref?.sessionId, first.sessionId);
});

test('selected-session runs serialize and revalidate a queued target after archive', async (t) => {
  const h = await harness(t);
  const entered = deferred(); const finish = deferred<string>();
  h.setExecute(async () => { entered.resolve(); return finish.promise; });
  const saved = task({ runMode: 'selected_session', targetSessionId: targetId });
  const first = runClaim(h.service, h.source, 'first', saved);
  await entered.promise;
  const second = runClaim(h.service, h.source, 'second', saved);
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
  const first = await runClaim(h.service, h.source, 'first', task({ runMode: 'task_session' }));
  h.catalogRows.get(h.project)!.push({ uuid: first.sessionId, title: 'Dedicated task' });
  const saved = task({ runMode: 'task_session', ownedSessionId: first.sessionId });
  const second = await runClaim(h.service, h.source, 'second', saved);
  assert.equal(second.sessionId, first.sessionId);
  assert.equal(h.background[0].empty, true);
  assert.equal(h.background[1].empty, false);
  h.catalogRows.set(h.project, []);
  await assert.rejects(runClaim(h.service, h.source, 'third', saved), /unavailable/);
  assert.equal(h.background.length, 2);
});

test('notification policy reports each success/failure once and suppresses disabled notifications', async (t) => {
  const h = await harness(t);
  for (const policy of ['all', 'failed', 'none'] as const) {
    h.notifications.length = 0;
    h.setExecute(async () => 'summary');
    await runClaim(h.service, h.source, `${policy}-success`, task({ notificationPolicy: policy }));
    assert.equal(h.notifications.length, policy === 'all' ? 1 : 0);
    h.notifications.length = 0;
    h.setExecute(async () => { throw new Error('model request failed'); });
    await assert.rejects(runClaim(h.service, h.source, `${policy}-failed`, task({ notificationPolicy: policy })), /model request failed/);
    assert.equal(h.notifications.length, policy === 'none' ? 0 : 1);
  }
});

test('configuration failures notify according to failure policy before execution starts', async (t) => {
  const h = await harness(t);
  h.archived.add(`${h.project}:${targetId}`);
  await assert.rejects(runClaim(h.service, h.source, 'archived', task({ runMode: 'selected_session', targetSessionId: targetId, notificationPolicy: 'failed' })), /archived/);
  assert.equal(h.background.length, 0);
  assert.equal(h.notifications.length, 1);
  h.projects.delete(h.project);
  await assert.rejects(runClaim(h.service, h.source, 'removed', task({ notificationPolicy: 'none' })), /unavailable/);
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
  await writeCatalog(h.project, [task()]);
  h.failEnsure(new Error('engine failed to start'));
  await assert.rejects(h.service.manage(h.project, { action: 'list' }), /failed to start/);
  assert.equal(h.leases[0].released, true);
  h.service.dispose();
  await assert.rejects(h.service.manage(h.project, { action: 'list' }), /closed/);
});

test('notification delivery suppresses duplicate run IDs across service restarts', async (t) => {
  const h = await harness(t);
  await runClaim(h.service, h.source, 'same-occurrence', task());
  await runClaim(h.service, h.source, 'same-occurrence', task());
  assert.equal(h.notifications.length, 1);
  h.service.dispose();
  const reopened = h.createService();
  t.after(() => reopened.dispose());
  await runClaim(reopened, h.source, 'same-occurrence', task());
  assert.equal(h.notifications.length, 1, 'delivered notification reservation survives restart');
  await runClaim(reopened, h.source, 'next-occurrence', task());
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

test('selected and task-owned targets beyond the sidebar limit remain valid authoritative sessions', async (t) => {
  const h = await harness(t);
  h.catalogRows.set(h.project, Array.from({ length: 200 }, (_, index) => ({
    uuid: `00000000-0000-4${String(index).padStart(3, '0')}-8000-${String(index).padStart(12, '0')}`,
    title: `Newer chat ${index}`,
  })).concat([{ uuid: targetId, title: 'Older persistent target' }]));
  assert.ok(!(await h.service.context(h.project)).sessions.some((session) => session.uuid === targetId));
  await h.service.manage(h.project, { action: 'create', automation: task({ runMode: 'selected_session', targetSessionId: targetId }).automation });
  const selected = await runClaim(h.service, h.source, 'old-selected', task({ runMode: 'selected_session', targetSessionId: targetId, notificationPolicy: 'none' }));
  const owned = await runClaim(h.service, h.source, 'old-task-owner', task({ runMode: 'task_session', ownedSessionId: targetId, notificationPolicy: 'none' }));
  assert.equal(selected.sessionId, targetId);
  assert.equal(owned.sessionId, targetId);
  assert.equal(h.turns.length, 2);
  assert.deepEqual(h.background.map(({ empty }) => empty), [false, false]);
});

// ---- The workspaces of Bots ----

const botTarget = '66666666-7777-4888-8999-000000000000';

/** The Bots the scheduler is told of: one that exists, as `BotService` answers for its folder. */
function botsOf(h: Awaited<ReturnType<typeof harness>>, options: { model?: string } = {}) {
  const root = join(h.directory, 'bots');
  const workspace = join(root, 'Bot0000000000000000001', 'workspace');
  const rows: { uuid: string; title: string }[] = [{ uuid: botTarget, title: 'Bot chat' }];
  const catalogCalls: string[] = [];
  const catalog: RoutineCatalog = {
    list: async (path) => { catalogCalls.push(`list ${path}`); return { sessions: rows } as never; },
    find: async (path, sessionId) => { catalogCalls.push(`find ${path} ${sessionId}`); return rows.find((row) => row.uuid === sessionId) as never; },
  };
  const bot: RoutineBot = { botId: 'Bot0000000000000000001', name: 'Research', workspace, ...(options.model ? { model: options.model } : {}), catalog: () => catalog };
  const present = new Set([workspace]);
  const deleting = new Set<string>();
  const api: RoutineBots = {
    routineBots: () => present.has(workspace) && !deleting.has(workspace) ? [bot] : [],
    routineBotFor: (path) => path === workspace && present.has(workspace) && !deleting.has(workspace) ? bot : undefined,
    isBotWorkspace: (path) => basename(path) === 'workspace' && dirname(dirname(path)) === root,
  };
  h.service.attachBots(api);
  return { bot, workspace, rows, catalogCalls, deleting, remove: () => { present.delete(workspace); }, api };
}

async function until(condition: () => Promise<boolean>): Promise<void> {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    if (await condition()) return;
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
  assert.fail('the condition did not come true');
}

test('the workspace of a Bot is a scope named for the Bot, and is not a project', async (t) => {
  const h = await harness(t);
  const b = botsOf(h);
  assert.deepEqual(h.service.scopes(), [
    { id: 'global', label: 'No project' },
    { id: h.project, projectPath: h.project, label: 'project' },
    { id: b.workspace, label: 'Research', botId: b.bot.botId },
  ]);
  assert.ok(!('projectPath' in h.service.scopes()[2]!), 'the Bot\'s folder is not offered as a project');
  assert.equal(h.service.resolveScope(b.workspace), b.workspace);
  assert.equal(h.projects.has(b.workspace), false);
  await assert.rejects(async () => h.service.resolveScope(join(dirname(b.workspace), 'home')), /Project is unavailable/);
  await assert.rejects(async () => h.service.resolveScope(join(h.directory, 'bots', 'Bot0000000000000000002', 'workspace')), /This Bot is unavailable/, 'the folder of a Bot that is not there');
});

test('a Bot that is being deleted, or is gone, is not a scope and starts no controller', async (t) => {
  const h = await harness(t);
  const b = botsOf(h);
  await writeCatalog(b.workspace, [task()]);
  b.deleting.add(b.workspace);
  assert.deepEqual(h.service.scopes().map((scope) => scope.id), ['global', h.project]);
  assert.throws(() => h.service.resolveScope(b.workspace), /This Bot is unavailable/);
  await assert.rejects(h.service.manage(b.workspace, { action: 'list' }), /This Bot is unavailable/);
  await assert.rejects(h.service.manage(b.workspace, { action: 'create', automation: task().automation }), /This Bot is unavailable/);
  await assert.rejects(h.service.context(b.workspace), /This Bot is unavailable/);
  assert.equal(h.ensured.length, 0, 'no engine was started for it');
  assert.equal(h.leases.length, 0, 'and no lease taken');
  b.deleting.delete(b.workspace);
  b.remove();
  assert.deepEqual(h.service.scopes().map((scope) => scope.id), ['global', h.project]);
  await assert.rejects(h.service.manage(b.workspace, { action: 'list' }), /This Bot is unavailable/);
});

test('a Bot that begins to be deleted while the scheduler is reading its catalog gets no controller and no lease', async (t) => {
  const h = await harness(t);
  const b = botsOf(h);
  // The scope was listed and its catalog read; only then did the deletion start. The scheduler gets to the controller last.
  b.deleting.add(b.workspace);
  await assert.rejects((h.service as any).controller(b.workspace), /This Bot is unavailable/);
  assert.equal(h.leases.length, 0);
  assert.equal(h.ensured.length, 0);
});

test('a Bot\'s routines are managed in its workspace, by a controller that starts on the Bot\'s model and not the owner\'s', async (t) => {
  const h = await harness(t);
  const b = botsOf(h, { model: 'bot/model' });
  await writeCatalog(b.workspace, [task()]);
  await writeCatalog(h.project, [task()]);
  await h.service.manage(b.workspace, { action: 'list' });
  await h.service.manage(h.project, { action: 'list' });
  assert.equal(h.ensured[0]!.ref.projectPath, b.workspace);
  assert.equal(h.ensured[0]!.empty, true);
  assert.equal(h.ensured[0]!.model, 'bot/model');
  assert.equal(h.ensured[1]!.ref.projectPath, h.project);
  assert.equal(h.ensured[1]!.model, 'openai/default', 'a project still starts on the owner\'s model');
  assert.ok(h.service.isControllerSession(b.workspace, h.ensured[0]!.ref.sessionId));
  assert.deepEqual(h.errors, []);
});

test('reading the routines of a Bot that has none starts no engine, so opening a Bot\'s page costs nothing', async (t) => {
  const h = await harness(t);
  const b = botsOf(h);
  assert.deepEqual(await h.service.manage(b.workspace, { action: 'list' }), []);
  assert.deepEqual(await h.service.manage(b.workspace, { action: 'history', id: 'task-1' }), []);
  assert.equal(h.ensured.length, 0);
  assert.equal(h.leases.length, 0);
  assert.deepEqual(h.errors, []);
});

test('a Bot without a model of its own starts its controller on no model, which the Bot\'s launch then settles', async (t) => {
  const h = await harness(t);
  const b = botsOf(h);
  await writeCatalog(b.workspace, [task()]);
  await h.service.manage(b.workspace, { action: 'list' });
  assert.equal(h.ensured[0]!.model, undefined, 'the owner\'s model must not be passed on: it would win over what the Bot is set to');
});

test('discovery finds the routines a chat of the Bot scheduled, and lets go of the Bot\'s controller when the Bot goes', async (t) => {
  const h = await harness(t);
  const b = botsOf(h);
  await writeCatalog(b.workspace, [{ automation: { status: 'active' } }]);
  await h.service.sync();
  assert.equal(h.leases.length, 1);
  assert.equal(h.leases[0]!.ref.projectPath, b.workspace);
  assert.equal(h.leases[0]!.released, false);
  b.deleting.add(b.workspace);
  await h.service.sync();
  assert.equal(h.leases[0]!.released, true);
  assert.deepEqual(h.errors, []);
});

test('a routine of a Bot runs in the Bot\'s workspace although it is not a project', async (t) => {
  const h = await harness(t);
  const b = botsOf(h);
  const source = h.runtime({ projectPath: b.workspace, sessionId: targetId });
  const result = await runClaim(h.service, source, 'bot-run', task({ model: 'bot/model' }));
  assert.deepEqual(h.background.map((item) => [item.ref.projectPath, item.empty, item.model]), [[b.workspace, true, 'bot/model']]);
  assert.deepEqual(h.notifications.map((item) => item.ref), [{ projectPath: b.workspace, sessionId: result.sessionId }], 'the notification names the Bot\'s workspace, which the host opens as the Bot\'s chat');
  assert.deepEqual(h.bindings.map((binding) => binding.sessionId), [result.sessionId]);
});

test('a routine of a Bot that is being deleted, or is gone, does not run', async (t) => {
  const h = await harness(t);
  const b = botsOf(h);
  const source = h.runtime({ projectPath: b.workspace, sessionId: targetId });
  b.deleting.add(b.workspace);
  await assert.rejects(runClaim(h.service, source, 'deleting', task()), /paused: The Bot is unavailable\./);
  b.deleting.delete(b.workspace);
  b.remove();
  await assert.rejects(runClaim(h.service, source, 'gone', task()), /paused: The Bot is unavailable\./);
  assert.equal(h.background.length, 0);
  assert.equal(h.notifications.at(-1)?.body, 'The Bot is unavailable.');
  // Another folder that is neither a project nor a Bot's is still the project that is unavailable.
  await assert.rejects(runClaim(h.service, h.runtime({ projectPath: join(h.directory, 'elsewhere'), sessionId: targetId }), 'stranger', task()), /paused: The project is unavailable\./);
});

test('a routine queued behind another one is checked again once it is its turn, and the Bot may be gone by then', async (t) => {
  const h = await harness(t);
  const b = botsOf(h);
  const source = h.runtime({ projectPath: b.workspace, sessionId: targetId });
  const entered = deferred(); const finish = deferred<string>();
  h.setExecute(async () => { entered.resolve(); return finish.promise; });
  const saved = task({ runMode: 'selected_session', targetSessionId: botTarget });
  const first = runClaim(h.service, source, 'first', saved);
  await entered.promise;
  const second = runClaim(h.service, source, 'second', saved);
  const rejected = assert.rejects(second, /The Bot is unavailable/);
  await new Promise<void>((resolve) => setImmediate(resolve));
  b.deleting.add(b.workspace);
  finish.resolve('Finished first run');
  await first; await rejected;
  assert.equal(h.turns.length, 1, 'the second run never started');
});

test('the chats a routine can target are the Bot\'s own, not the owner\'s', async (t) => {
  const h = await harness(t);
  const b = botsOf(h);
  const context = await h.service.context(b.workspace);
  assert.deepEqual(context.sessions, [{ uuid: botTarget, title: 'Bot chat' }]);
  assert.deepEqual(b.catalogCalls, [`list ${b.workspace}`]);
  assert.equal(h.ensured[0]!.ref.projectPath, b.workspace);
  await h.service.manage(b.workspace, { action: 'create', automation: task({ runMode: 'selected_session', targetSessionId: botTarget }).automation });
  assert.ok(b.catalogCalls.includes(`find ${b.workspace} ${botTarget}`), 'a target is looked up in the Bot\'s catalog');
  await assert.rejects(h.service.manage(b.workspace, { action: 'create', automation: task({ runMode: 'selected_session', targetSessionId: targetId }).automation }), /target chat is unavailable/, 'a chat of the owner\'s is not the Bot\'s');
  // And a project\'s routine still looks in the owner\'s catalog.
  await h.service.manage(h.project, { action: 'create', automation: task({ runMode: 'selected_session', targetSessionId: targetId }).automation });
  assert.ok(!b.catalogCalls.some((call) => call.includes(h.project)));
});

test('the tasks of a Bot are never kept for an unavailable project, whether the Bot is there or not', async (t) => {
  const h = await harness(t);
  const b = botsOf(h);
  h.setManaged([task()]);
  await h.service.manage(b.workspace, { action: 'create', automation: task().automation });
  await h.service.manage(h.project, { action: 'create', automation: task().automation });
  const cacheFile = join(h.directory, 'scheduled-scopes.json');
  const cached = JSON.parse(await readFile(cacheFile, 'utf8')) as { path: string }[];
  assert.deepEqual(cached.map((row) => row.path), [h.project], 'only the project is remembered');
  h.projects.delete(h.project);
  b.deleting.add(b.workspace);
  assert.deepEqual(h.service.scopes().filter((scope) => scope.unavailable).map((scope) => scope.id), [h.project], 'the Bot is not listed as an unavailable project');
  b.remove();
  assert.deepEqual(h.service.scopes().filter((scope) => scope.unavailable).map((scope) => scope.id), [h.project]);
});

test('an entry kept for the folder of a Bot is never offered as an unavailable project, and is dropped when the Bot is released', async (t) => {
  const h = await harness(t);
  const b = botsOf(h);
  const cacheFile = join(h.directory, 'scheduled-scopes.json');
  await writeFile(cacheFile, JSON.stringify([{ path: b.workspace, label: 'workspace', jobs: [task()] }, { path: h.project, label: 'project', jobs: [task()] }]));
  const service = h.createService();
  t.after(() => service.dispose());
  service.attachBots(b.api);
  h.projects.delete(h.project);
  b.remove();
  assert.deepEqual(service.scopes().filter((scope) => scope.unavailable).map((scope) => scope.id), [h.project]);
  service.releaseScope(b.workspace);
  await until(async () => !(await readFile(cacheFile, 'utf8')).includes(b.workspace));
  assert.deepEqual((JSON.parse(await readFile(cacheFile, 'utf8')) as { path: string }[]).map((row) => row.path), [h.project], 'what was kept for the project is kept');
});

test('releasing a scope lets go of its controller at once, and the scheduler may start another when the scope is listed again', async (t) => {
  const h = await harness(t);
  const b = botsOf(h);
  h.setManaged([task()]);
  await writeCatalog(b.workspace, [task()]);
  await h.service.manage(b.workspace, { action: 'list' });
  assert.equal(h.leases.length, 1);
  assert.equal(h.leases[0]!.released, false, 'an active routine keeps the controller');
  h.service.releaseScope(b.workspace);
  assert.equal(h.leases[0]!.released, true);
  h.service.releaseScope(b.workspace);
  h.service.releaseScope(join(h.directory, 'never-seen'));
  assert.equal(h.leases.length, 1, 'releasing what is not held is harmless');
  await h.service.manage(b.workspace, { action: 'list' });
  assert.equal(h.leases.length, 2);
  assert.equal(h.leases[1]!.released, false);
});

test('without Bots attached the scheduler is what it was: projects only, and a Bot\'s folder is just an unknown path', async (t) => {
  const h = await harness(t);
  assert.deepEqual(h.service.scopes().map((scope) => scope.id), ['global', h.project]);
  assert.throws(() => h.service.resolveScope(join(h.directory, 'bots', 'Bot0000000000000000001', 'workspace')), /Project is unavailable/);
});
