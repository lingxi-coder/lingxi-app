import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test, type TestContext } from 'node:test';

import { validateClientEvent, type CronAutomationDto, type CronJobDto } from '@lingxi/bridge-client';
import type { SessionRuntime } from '../src/main/bridge.js';
import type { ProjectSessionCatalog } from '../src/main/session-catalog.js';
import type { SessionRuntimeManager } from '../src/main/sessionRuntimeManager.js';
import type { SettingsStore } from '../src/main/settings.js';
import type { SessionRef } from '../src/shared/settings.js';
import { ScheduledTaskService } from '../src/main/scheduled.js';
import { scheduledRunIdentity } from '../src/main/scheduled-run-identity.js';

const targetSessionId = '11111111-2222-4333-8444-555555555555';

function task(patch: Partial<CronAutomationDto> = {}): CronJobDto {
  return {
    id: 'task-1', cron: '0 9 * * *', prompt: '# Morning brief\nSummarize the project.',
    recurring: true, durable: true, permanent: false, created_at: 1,
    automation: {
      version: 2, status: 'active', model: 'openai/test-model', reasoning: { type: 'automatic' },
      runMode: 'new_session', notificationPolicy: 'all', ...patch,
    },
  };
}

function claimWireId(occurrenceId: string, claimGeneration: number): string {
  return `lingxi-cron-claim-v1:${JSON.stringify([occurrenceId, claimGeneration])}`;
}

function claimedTask(runId: string, claimGeneration: number, patch: Partial<CronAutomationDto> = {}): CronJobDto {
  return task({
    runs: [{
      id: runId, taskId: 'task-1', claimGeneration, scheduledAt: 1, status: 'running',
      model: 'openai/test-model', reasoning: { type: 'automatic' },
    }],
    ...patch,
  });
}

function deferred<T = void>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  const promise = new Promise<T>((complete) => { resolve = complete; });
  return { promise, resolve };
}

function harness(t: TestContext) {
  const projects = new Set(['/project-a', '/project-b']);
  const background: SessionRef[] = [];
  const bindings: Array<{ projectPath: string; runId: string; sessionId: string }> = [];
  const turns: Array<{ ref: SessionRef; runId: string; task: CronJobDto }> = [];
  const notifications: Array<{ title: string; body: string; ref?: SessionRef }> = [];
  const reports: unknown[] = [];
  let execute = async (_ref: SessionRef, _runId: string, _task: CronJobDto) => 'Completed summary';
  let openRuntime = async (ref: SessionRef) => runtime(ref);
  let beforeBinding = async () => undefined;
  let listCatalog = async (_path: string) => ({ sessions: [{ uuid: targetSessionId }] });
  const runtime = (ref: SessionRef) => ({
    projectPath: ref.projectPath,
    markCronRunStarted: async (runId: string, sessionId: string) => {
      bindings.push({ projectPath: ref.projectPath, runId, sessionId });
    },
    runScheduledTurn: async (runId: string, saved: CronJobDto, beforeStart?: () => Promise<void>) => {
      await beforeBinding();
      await beforeStart?.();
      turns.push({ ref, runId, task: saved });
      return execute(ref, runId, saved);
    },
  }) as unknown as SessionRuntime;
  const settings = {
    scheduledWorkspace: '/global',
    getPublic: () => ({ projects: [...projects], model: 'openai/default' }),
    hasProject: (path: string) => projects.has(path),
    isTrustedWorkspace: (path: string) => projects.has(path),
    isSessionArchived: () => false,
  } as unknown as SettingsStore;
  const manager = {
    withBackgroundSession: async <T>(ref: SessionRef, _empty: boolean, _model: string | undefined, operation: (target: SessionRuntime) => Promise<T>) => {
      background.push(ref);
      return operation(await openRuntime(ref));
    },
  } as unknown as SessionRuntimeManager;
  const catalog = {
    list: (path: string) => listCatalog(path),
    find: async (path: string, sessionId: string) => (await listCatalog(path)).sessions.find((row) => row.uuid === sessionId),
  } as unknown as ProjectSessionCatalog;
  const service = new ScheduledTaskService(settings, manager, catalog, (title, body, ref) => {
    notifications.push({ title, body, ref });
    return true;
  }, (error) => reports.push(error));
  t.after(() => service.dispose());
  return {
    service, background, bindings, turns, notifications, reports,
    source: runtime({ projectPath: '/project-a', sessionId: targetSessionId }),
    sourceFor: (projectPath: string) => runtime({ projectPath, sessionId: targetSessionId }),
    setExecute: (callback: typeof execute) => { execute = callback; },
    setOpenRuntime: (callback: typeof openRuntime) => { openRuntime = callback; },
    setBeforeBinding: (callback: typeof beforeBinding) => { beforeBinding = callback; },
    setListCatalog: (callback: typeof listCatalog) => { listCatalog = callback; },
    runtime,
  };
}

test('actual Rust producer claims replay once and preserve their opaque wire IDs through execution and binding', async (t) => {
  const fixture: unknown = JSON.parse(readFileSync(new URL('./fixtures/cron-claim-replay.json', import.meta.url), 'utf8'));
  assert.ok(Array.isArray(fixture));
  assert.equal(fixture.length, 2);
  const firstEvent = validateClientEvent(fixture[0]);
  const secondEvent = validateClientEvent(fixture[1]);
  assert.ok(firstEvent.type === 'cron_run_requested');
  assert.ok(secondEvent.type === 'cron_run_requested');
  const firstRecord = firstEvent.task.automation!.runs![0]!;
  const secondRecord = secondEvent.task.automation!.runs![0]!;
  assert.equal(firstRecord.id, 'retried-occurrence');
  assert.equal(secondRecord.id, firstRecord.id);
  assert.equal(firstRecord.claimGeneration, 1);
  assert.equal(secondRecord.claimGeneration, 2);
  assert.notEqual(firstRecord.id, firstEvent.run_id, 'SDK metadata keeps the canonical occurrence ID');
  assert.equal(firstRecord.startedAt, null);
  assert.equal(firstRecord.sessionId, null);
  assert.equal(secondRecord.startedAt, null);
  assert.equal(secondRecord.sessionId, null);
  const h = harness(t);
  const busy = new Error('busy: fixture first claim must retry');
  h.setExecute(async (_ref, wireId) => {
    if (wireId === firstEvent.run_id) throw busy;
    assert.equal(wireId, secondEvent.run_id);
    return 'Rust fixture retry completed';
  });
  const first = h.service.run(h.source, firstEvent.run_id, firstEvent.task);
  assert.strictEqual(h.service.run(h.source, firstEvent.run_id, structuredClone(firstEvent.task)), first);
  await assert.rejects(first, (error) => error === busy);
  assert.strictEqual(h.service.run(h.source, firstEvent.run_id, firstEvent.task), first);
  const retry = h.service.run(h.source, secondEvent.run_id, secondEvent.task);
  assert.notStrictEqual(retry, first);
  assert.strictEqual(h.service.run(h.source, secondEvent.run_id, structuredClone(secondEvent.task)), retry);
  const result = await retry;
  assert.equal(result.summary, 'Rust fixture retry completed');
  assert.strictEqual(h.service.run(h.source, secondEvent.run_id, secondEvent.task), retry);
  assert.deepEqual(h.turns.map((turn) => turn.runId), [firstEvent.run_id, secondEvent.run_id]);
  assert.deepEqual(h.bindings.map((binding) => binding.runId), [firstEvent.run_id, secondEvent.run_id]);
  assert.equal(h.background.length, 2);
  assert.notEqual(h.background[0]!.sessionId, result.sessionId);
  assert.equal(h.background[1]!.sessionId, result.sessionId);
  assert.equal(h.notifications.length, 0, 'the producer fixture keeps its none notification policy');
});

test('concurrent new-chat replay shares one promise, session, execution and notification', async (t) => {
  const h = harness(t);
  const entered = deferred();
  const finish = deferred<string>();
  h.setExecute(async () => { entered.resolve(); return finish.promise; });
  const saved = task();
  const first = h.service.run(h.source, 'same-occurrence', saved);
  const replay = h.service.run(h.source, 'same-occurrence', { ...saved, prompt: 'Later redelivery payload' });
  assert.strictEqual(replay, first, 'admission must share the exact in-flight promise');
  await entered.promise;
  assert.equal(h.background.length, 1);
  assert.equal(h.turns.length, 1);
  assert.strictEqual(h.turns[0]!.task, saved, 'the first admitted task snapshot wins');
  finish.resolve('First execution summary');
  const [result, duplicate] = await Promise.all([first, replay]);
  assert.strictEqual(duplicate, result);
  assert.strictEqual(h.service.run(h.source, 'same-occurrence', task()), first);
  assert.deepEqual(h.bindings, [{ projectPath: h.source.projectPath, runId: 'same-occurrence', sessionId: result.sessionId }]);
  assert.equal(h.notifications.length, 1);
  assert.equal(h.notifications[0]!.ref?.sessionId, result.sessionId);
  assert.equal(h.background.length, 1, 'terminal replay must not create another chat');
  assert.deepEqual(h.reports, []);
});

for (const message of ['model request failed', 'busy: selected chat is active', 'paused: model is unavailable', 'cancelled: User cancelled this run']) {
  test(`terminal failure replay preserves the original error without executing again: ${message}`, async (t) => {
    const h = harness(t);
    const failure = new Error(message);
    h.setExecute(async () => { throw failure; });
    const first = h.service.run(h.source, 'failed-occurrence', task());
    await assert.rejects(first, (error) => error === failure);
    h.setExecute(async () => assert.fail('the same claim generation must not execute again'));
    const replay = h.service.run(h.source, 'failed-occurrence', task());
    assert.strictEqual(replay, first);
    await assert.rejects(replay, (error) => error === failure);
    assert.equal(h.background.length, 1);
    assert.equal(h.turns.length, 1);
    assert.equal(h.notifications.length, /^(busy|cancelled):/.test(message) ? 0 : 1);
  });
}

test('a busy occurrence retries once when Rust advances its claim generation', async (t) => {
  const h = harness(t);
  const busy = new Error('busy: selected chat is active');
  h.setExecute(async () => { throw busy; });
  const firstClaim = claimedTask('retry-occurrence', 1);
  const firstWireId = claimWireId('retry-occurrence', 1);
  const secondWireId = claimWireId('retry-occurrence', 2);
  const first = h.service.run(h.source, firstWireId, firstClaim);
  await assert.rejects(first, (error) => error === busy);
  assert.strictEqual(h.service.run(h.source, firstWireId, firstClaim), first);
  h.setExecute(async () => 'Retry completed');
  const retryClaim = claimedTask('retry-occurrence', 2);
  const retry = h.service.run(h.source, secondWireId, retryClaim);
  assert.notStrictEqual(retry, first);
  assert.strictEqual(h.service.run(h.source, secondWireId, retryClaim), retry);
  const result = await retry;
  assert.equal(result.summary, 'Retry completed');
  assert.equal(h.turns.length, 2);
  assert.deepEqual(h.turns.map((turn) => turn.runId), [firstWireId, secondWireId]);
  assert.deepEqual(h.bindings.map((binding) => binding.runId), [firstWireId, secondWireId]);
  assert.equal(h.background.length, 2);
  assert.equal(result.sessionId, h.background[1]!.sessionId);
  assert.notEqual(h.background[0]!.sessionId, result.sessionId);
  assert.equal(h.notifications.length, 1, 'the busy claim must not notify');
  assert.strictEqual(h.service.run(h.source, firstWireId, firstClaim), first);
  await assert.rejects(first, (error) => error === busy);
});

test('in-flight claims of different generations keep independent promises and session bindings', async (t) => {
  const h = harness(t);
  const entered = deferred();
  const finish = deferred<string>();
  h.setExecute(async (_ref, runId, saved) => {
    const generation = saved.automation!.runs![0]!.claimGeneration;
    if (generation === 1) { entered.resolve(); return finish.promise; }
    return 'Second claim completed';
  });
  const firstClaim = claimedTask('generation-race', 1);
  const secondClaim = claimedTask('generation-race', 2);
  const firstWireId = claimWireId('generation-race', 1);
  const secondWireId = claimWireId('generation-race', 2);
  const first = h.service.run(h.source, firstWireId, firstClaim);
  await entered.promise;
  const second = h.service.run(h.source, secondWireId, secondClaim);
  assert.notStrictEqual(second, first);
  const newer = await second;
  assert.strictEqual(h.service.run(h.source, firstWireId, firstClaim), first);
  assert.strictEqual(h.service.run(h.source, secondWireId, secondClaim), second);
  finish.resolve('First claim completed later');
  const older = await first;
  assert.notEqual(older.sessionId, newer.sessionId);
  assert.equal(h.turns.length, 2);
  assert.deepEqual(h.bindings.map((binding) => binding.sessionId), [older.sessionId, newer.sessionId]);
  assert.deepEqual(h.bindings.map((binding) => binding.runId), [firstWireId, secondWireId]);
  assert.equal(h.notifications.length, 1, 'different claim wire IDs share one canonical occurrence notification');
  assert.strictEqual(h.service.run(h.source, firstWireId, firstClaim), first);
  assert.strictEqual(h.service.run(h.source, secondWireId, secondClaim), second);
});

for (const rawId of [
  '["canonical-occurrence",1]',
  'lingxi-cron-claim-v1:not-json',
  'lingxi-cron-claim-v1:["canonical-occurrence",-1]',
  'lingxi-cron-claim-v1:["canonical-occurrence",1,"extra"]',
]) {
  test(`unrelated or invalid claim-looking IDs remain unchanged: ${rawId}`, async (t) => {
    const h = harness(t);
    const saved = claimedTask('canonical-occurrence', 1, { notificationPolicy: 'none' });
    const first = h.service.run(h.source, rawId, saved);
    await first;
    assert.strictEqual(h.service.run(h.source, rawId, saved), first);
    const canonical = h.service.run(h.source, 'canonical-occurrence', saved);
    assert.notStrictEqual(canonical, first);
    await canonical;
    assert.deepEqual(h.bindings.map((binding) => binding.runId), [rawId, 'canonical-occurrence']);
    assert.equal(h.turns.length, 2);
  });
}

test('claim metadata must match both canonical ID and generation before decoding', async (t) => {
  const h = harness(t);
  const wireId = claimWireId('canonical-occurrence', 1);
  const mismatchedGeneration = claimedTask('canonical-occurrence', 2, { notificationPolicy: 'none' });
  const raw = h.service.run(h.source, wireId, mismatchedGeneration);
  await raw;
  assert.strictEqual(h.service.run(h.source, wireId, mismatchedGeneration), raw);
  const mismatchedOccurrence = claimedTask('another-occurrence', 1, { notificationPolicy: 'none' });
  assert.strictEqual(h.service.run(h.source, wireId, mismatchedOccurrence), raw, 'matching generation alone cannot decode another occurrence');
  const matching = claimedTask('canonical-occurrence', 1, { notificationPolicy: 'none' });
  const decoded = h.service.run(h.source, wireId, matching);
  assert.notStrictEqual(decoded, raw);
  await decoded;
  const legacyCanonical = h.service.run(h.source, 'canonical-occurrence', matching);
  assert.strictEqual(legacyCanonical, decoded, 'matching metadata identifies the canonical occurrence');
  assert.equal(h.turns.length, 2);
  assert.deepEqual(h.bindings.map((binding) => binding.runId), [wireId, wireId]);
});

test('a claim wire ID without run metadata stays stable as a legacy opaque ID', async (t) => {
  const h = harness(t);
  const wireId = claimWireId('missing-metadata', 1);
  const first = h.service.run(h.source, wireId, task({ notificationPolicy: 'none' }));
  await first;
  assert.strictEqual(h.service.run(h.source, wireId, task({ notificationPolicy: 'none' })), first);
  const canonical = h.service.run(h.source, 'missing-metadata', task({ notificationPolicy: 'none' }));
  assert.notStrictEqual(canonical, first);
  await canonical;
  assert.deepEqual(h.bindings.map((binding) => binding.runId), [wireId, 'missing-metadata']);
});

for (const cancellationMarked of [false, true]) {
test(cancellationMarked
  ? 'same-host cancellation recovery replay preserves the in-flight and terminal result without another admission'
  : 'same-host admitted replay preserves the in-flight and terminal result', async (t) => {
  const h = harness(t);
  const entered = deferred();
  const finish = deferred<string>();
  h.setExecute(async () => { entered.resolve(); return finish.promise; });
  const wireId = claimWireId('admitted-occurrence', 1);
  const saved = claimedTask('admitted-occurrence', 1);
  const first = h.service.run(h.source, wireId, saved);
  await entered.promise;
  const admitted = claimedTask('admitted-occurrence', 1, {
    runs: saved.automation!.runs!.map((run) => cancellationMarked
      ? { ...run, error: 'lingxi-host-cancel-requested-v1' }
      : { ...run, startedAt: 123, sessionId: h.background[0]!.sessionId }),
  });
  assert.strictEqual(h.service.run(h.source, wireId, admitted), first);
  finish.resolve('First host completed');
  const result = await first;
  assert.strictEqual(h.service.run(h.source, wireId, admitted), first);
  assert.equal(result.summary, 'First host completed');
  assert.equal(h.background.length, 1);
  assert.equal(h.turns.length, 1);
  assert.equal(h.notifications.length, 1);
});
}

for (const marker of [{ startedAt: 123 }, { sessionId: targetSessionId }, { error: 'lingxi-host-cancel-requested-v1' }]) {
  test(`new-host admitted replay rejects without opening another runtime: ${Object.keys(marker)[0]}`, async (t) => {
    const h = harness(t);
    const saved = claimedTask('admitted-occurrence', 1);
    const admitted = claimedTask('admitted-occurrence', 1, {
      runs: saved.automation!.runs!.map((run) => ({ ...run, ...marker })),
    });
    h.setExecute(async () => assert.fail('an admitted claim must never execute in another host'));
    const wireId = claimWireId('admitted-occurrence', 1);
    const first = h.service.run(h.source, wireId, admitted);
    await assert.rejects(first, /^Error: interrupted:.*already started.*completion is unavailable/);
    assert.strictEqual(h.service.run(h.source, wireId, admitted), first);
    await assert.rejects(first, /^Error: interrupted:/);
    assert.equal(h.background.length, 0);
    assert.equal(h.bindings.length, 0);
    assert.equal(h.turns.length, 0);
    assert.equal(h.notifications.length, 1, 'replay reports the interrupted occurrence once');
  });
}

test('cancellation recovery markers require an exact verified claim identity', () => {
  const wireId = claimWireId('cancelled-occurrence', 1);
  const marked = claimedTask('cancelled-occurrence', 1);
  marked.automation!.runs![0]!.error = 'lingxi-host-cancel-requested-v1';
  assert.deepEqual(scheduledRunIdentity(wireId, marked), {
    occurrenceId: 'cancelled-occurrence', claimGeneration: 1, hostAdmitted: true,
  });
  for (const error of [undefined, 'lingxi-host-cancel-requested-v1 ', 'lingxi-host-cancel-requested-v2', 'cancelled']) {
    const unrelated = structuredClone(marked);
    unrelated.automation!.runs![0]!.error = error;
    assert.equal(scheduledRunIdentity(wireId, unrelated).hostAdmitted, false);
  }
  for (const [rawId, saved] of [
    ['cancelled-occurrence', marked],
    ['lingxi-cron-claim-v1:not-json', marked],
    ['lingxi-cron-claim-v1:["cancelled-occurrence",-1]', marked],
    ['lingxi-cron-claim-v1:["cancelled-occurrence",1,"extra"]', marked],
    [wireId, claimedTask('another-occurrence', 1, { runs: [{ ...marked.automation!.runs![0]!, id: 'another-occurrence' }] })],
    [wireId, claimedTask('cancelled-occurrence', 2, { runs: [{ ...marked.automation!.runs![0]!, claimGeneration: 2 }] })],
    [wireId, task()],
  ] as const) {
    assert.equal(scheduledRunIdentity(rawId, saved).hostAdmitted, false);
    assert.equal(scheduledRunIdentity(rawId, saved).occurrenceId, rawId);
  }
});

test('legacy IDs with pre-existing SDK session and start fields are not treated as host admission', async (t) => {
  const h = harness(t);
  const saved = claimedTask('legacy-occurrence', 1);
  const legacy = claimedTask('legacy-occurrence', 1, {
    runs: saved.automation!.runs!.map((run) => ({ ...run, startedAt: 123, sessionId: targetSessionId })),
  });
  const first = h.service.run(h.source, 'legacy-occurrence', legacy);
  const result = await first;
  assert.equal(result.summary, 'Completed summary');
  assert.strictEqual(h.service.run(h.source, 'legacy-occurrence', legacy), first);
  assert.equal(h.background.length, 1);
  assert.deepEqual(h.bindings.map((binding) => binding.runId), ['legacy-occurrence']);
});

test('the same run ID in different projects executes independently', async (t) => {
  const h = harness(t);
  const secondSource = h.sourceFor('/project-b');
  const first = h.service.run(h.source, 'shared-run-id', task());
  const second = h.service.run(secondSource, 'shared-run-id', task());
  assert.notStrictEqual(second, first);
  const [a, b] = await Promise.all([first, second]);
  assert.notEqual(a.sessionId, b.sessionId);
  assert.deepEqual(h.background.map((ref) => ref.projectPath).sort(), ['/project-a', '/project-b']);
  assert.equal(h.notifications.length, 2);
  assert.strictEqual(h.service.run(h.source, 'shared-run-id', task()), first);
  assert.strictEqual(h.service.run(secondSource, 'shared-run-id', task()), second);
});

test('disposed service rejects and replays first-time admission without opening a runtime', async (t) => {
  const h = harness(t);
  h.service.dispose();
  const first = h.service.run(h.source, 'after-shutdown', task());
  await assert.rejects(first, /^Error: cancelled:.*closed/);
  assert.strictEqual(h.service.run(h.source, 'after-shutdown', task()), first);
  await assert.rejects(first, /^Error: cancelled:/);
  assert.equal(h.background.length, 0);
  assert.equal(h.bindings.length, 0);
  assert.equal(h.turns.length, 0);
  assert.equal(h.notifications.length, 0);
});

test('shutdown prevents a selected-session run from starting after its queue drains', async (t) => {
  const h = harness(t);
  const entered = deferred();
  const finish = deferred<string>();
  h.setExecute(async () => { entered.resolve(); return finish.promise; });
  const saved = task({ runMode: 'selected_session', targetSessionId: targetSessionId, notificationPolicy: 'none' });
  const first = h.service.run(h.source, 'running', saved);
  await entered.promise;
  const queued = h.service.run(h.source, 'queued', saved);
  const rejected = assert.rejects(queued, /^Error: cancelled:/);
  await new Promise<void>((resolve) => setImmediate(resolve));
  h.service.dispose();
  finish.resolve('Already running work finished');
  await first;
  await rejected;
  assert.equal(h.background.length, 1);
  assert.equal(h.turns.length, 1);
  assert.strictEqual(h.service.run(h.source, 'queued', saved), queued);
});

test('shutdown during target validation does not acquire an execution runtime', async (t) => {
  const h = harness(t);
  const entered = deferred();
  const finish = deferred<{ sessions: { uuid: string }[] }>();
  h.setListCatalog(async () => { entered.resolve(); return finish.promise; });
  const run = h.service.run(h.source, 'validating', task({ runMode: 'selected_session', targetSessionId: targetSessionId }));
  const rejected = assert.rejects(run, /^Error: cancelled:/);
  await entered.promise;
  h.service.dispose();
  finish.resolve({ sessions: [{ uuid: targetSessionId }] });
  await rejected;
  assert.equal(h.background.length, 0);
  assert.equal(h.bindings.length, 0);
  assert.equal(h.notifications.length, 0);
});

test('shutdown during queued target revalidation does not acquire an execution runtime', async (t) => {
  const h = harness(t);
  const entered = deferred();
  const finish = deferred<{ sessions: { uuid: string }[] }>();
  let validations = 0;
  h.setListCatalog(async () => {
    validations += 1;
    if (validations === 2) { entered.resolve(); return finish.promise; }
    return { sessions: [{ uuid: targetSessionId }] };
  });
  const run = h.service.run(h.source, 'revalidating', task({ runMode: 'selected_session', targetSessionId: targetSessionId }));
  const rejected = assert.rejects(run, /^Error: cancelled:/);
  await entered.promise;
  h.service.dispose();
  finish.resolve({ sessions: [{ uuid: targetSessionId }] });
  await rejected;
  assert.equal(validations, 2);
  assert.equal(h.background.length, 0);
  assert.equal(h.turns.length, 0);
  assert.equal(h.bindings.length, 0);
  assert.equal(h.notifications.length, 0);
});

test('shutdown while the runtime opens prevents turn dispatch and run binding', async (t) => {
  const h = harness(t);
  const entered = deferred();
  const finish = deferred<SessionRuntime>();
  h.setOpenRuntime(async () => { entered.resolve(); return finish.promise; });
  const run = h.service.run(h.source, 'opening', task());
  const rejected = assert.rejects(run, /^Error: cancelled:/);
  await entered.promise;
  h.service.dispose();
  finish.resolve(h.runtime(h.background[0]!));
  await rejected;
  assert.equal(h.turns.length, 0);
  assert.equal(h.bindings.length, 0);
  assert.equal(h.notifications.length, 0);
});

test('shutdown while a runtime prepares its turn prevents the binding callback', async (t) => {
  const h = harness(t);
  const entered = deferred();
  const finish = deferred();
  h.setBeforeBinding(async () => { entered.resolve(); await finish.promise; });
  const run = h.service.run(h.source, 'preparing', task());
  const rejected = assert.rejects(run, /^Error: cancelled:/);
  await entered.promise;
  h.service.dispose();
  finish.resolve();
  await rejected;
  assert.equal(h.turns.length, 0);
  assert.equal(h.bindings.length, 0);
  assert.equal(h.notifications.length, 0);
});

test('terminal history is bounded while in-flight replay survives terminal cache pressure', async (t) => {
  const h = harness(t);
  const entered = deferred();
  const finish = deferred<string>();
  h.setExecute(async (_ref, runId) => {
    if (runId === 'in-flight') { entered.resolve(); return finish.promise; }
    return runId;
  });
  const silent = task({ notificationPolicy: 'none' });
  const pending = h.service.run(h.source, 'in-flight', silent);
  await entered.promise;
  const oldest = h.service.run(h.source, 'terminal-0', silent);
  await oldest;
  let newest = oldest;
  for (let index = 1; index <= 500; index += 1) {
    newest = h.service.run(h.source, `terminal-${index}`, silent);
    await newest;
  }
  assert.strictEqual(h.service.run(h.source, 'in-flight', silent), pending, 'terminal eviction must never evict active work');
  assert.strictEqual(h.service.run(h.source, 'terminal-500', silent), newest);
  const admittedAfterEviction = h.service.run(h.source, 'terminal-0', silent);
  assert.notStrictEqual(admittedAfterEviction, oldest, 'the oldest terminal entry must leave the bounded history');
  await admittedAfterEviction;
  assert.equal(h.turns.filter((turn) => turn.runId === 'in-flight').length, 1);
  finish.resolve('Completed long run');
  await pending;
  assert.strictEqual(h.service.run(h.source, 'in-flight', silent), pending);
});
