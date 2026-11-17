import assert from 'node:assert/strict';
import { digest, FIXTURE_DIGEST, SHARED_CHECKSUM } from './team-value-fixtures.mjs';
import { reportedUsage } from './team-value-host-receipts.mjs';

export const POLICY = Object.freeze({ slots: 144, intervalMs: 600_000, deadlineMs: 300_000, pollMs: 30_000, windowMs: 86_400_000 });
const terminal = new Set(['passed', 'failed', 'missed']);
const bounded = value => Number.isSafeInteger(value) && value >= 0;
const nonempty = value => typeof value === 'string' && value.length > 0;
export function validatePreflight(p, requiredRemaining = POLICY.slots) {
  assert.ok(p && ['live', 'fixture', 'mock'].includes(p.mode), 'Explicit provenance mode required');
  assert.ok(['team', 'service'].includes(p.scope?.kind) && nonempty(p.scope.id), 'Verified scope required');
  assert.ok(nonempty(p.package?.revision) && /^[a-f0-9]{64}$/u.test(p.package?.sha256), 'Frozen published package evidence required');
  assert.ok(nonempty(p.provider?.id) && nonempty(p.provider?.model) && nonempty(p.provider?.evidenceRef), 'Provider source evidence required');
  assert.equal(p.isolatedQa, true, 'An isolated, already configured QA scope is required');
  for (const field of ['team', 'member', 'bot']) {
    assert.equal(p.quotas?.limits?.[field], 200, 'QA daily quota must be 200/200/200');
    assert.ok(p.quotas?.remaining?.[field] >= requiredRemaining, 'Insufficient QA quota');
  }
  for (const key of ['context_window_tokens', 'output_tokens_per_request', 'physical_dispatches']) assert.ok(bounded(p.limits?.[key]) && p.limits[key] > 0, 'Existing per-task input context/output/call limits required');
  assert.ok(nonempty(p.scope.evidenceRef) && nonempty(p.package.evidenceRef), 'Actual scope and package inspection references required');
  return p;
}

export function createRun({ runId, startAt, preflight }) {
  assert.match(runId, /^[A-Za-z0-9_-]{8,40}$/u);
  assert.ok(bounded(startAt));
  validatePreflight(preflight);
  return { schemaVersion: 1, runId, startAt, endAt: startAt + POLICY.windowMs, fixtureDigest: FIXTURE_DIGEST,
    checksum: SHARED_CHECKSUM, policy: { ...POLICY }, preflight,
    slots: Array.from({ length: POLICY.slots }, (_, index) => ({ index, probeId: `${runId}_${String(index).padStart(3, '0')}`,
      scheduledAt: startAt + index * POLICY.intervalMs, status: 'pending' })) };
}

// The checkpoint contains no messages, answer body, credentials, or provider request payloads.
export function validateRun(run) {
  assert.equal(run.schemaVersion, 1);
  assert.ok(bounded(run.startAt));
  assert.match(run.runId, /^[A-Za-z0-9_-]{8,40}$/u);
  assert.equal(run.fixtureDigest, FIXTURE_DIGEST, 'Fixture drift');
  assert.equal(run.checksum, SHARED_CHECKSUM, 'Shared source drift');
  assert.deepEqual(run.policy, POLICY, 'Policy/deadline cannot change on resume');
  assert.equal(run.endAt, run.startAt + POLICY.windowMs);
  assert.equal(run.slots.length, POLICY.slots);
  validatePreflight(run.preflight);
  run.slots.forEach((slot, index) => {
    assert.equal(slot.index, index);
    assert.equal(slot.probeId, `${run.runId}_${String(index).padStart(3, '0')}`);
    assert.equal(slot.scheduledAt, run.startAt + index * POLICY.intervalMs);
    assert.ok(['pending', 'submitting', 'observing', ...terminal].includes(slot.status));
    if (slot.submittedAt !== undefined) assert.equal(slot.deadline, slot.submittedAt + POLICY.deadlineMs);
    if (slot.status === 'passed') {
      assert.ok(nonempty(slot.taskId) && nonempty(slot.resultId) && nonempty(slot.providerEvidenceRef));
      assert.ok(bounded(slot.dispatchedRequests) && slot.dispatchedRequests > 0);
      assert.match(slot.resultDigest, /^[a-f0-9]{64}$/u);
      assert.ok(bounded(slot.observedAt) && slot.observedAt >= slot.submittedAt && slot.observedAt <= slot.deadline);
      assert.ok(slot.submittedAt >= slot.scheduledAt && slot.submittedAt < slot.scheduledAt + POLICY.intervalMs);
    }
  });
  return run;
}

export function probeCommand(run, slot, binding) {
  const commandId = digest([run.runId, slot.index, 'command']);
  return { schemaVersion: 1, commandId, [run.preflight.scope.kind === 'service' ? 'serviceId' : 'teamId']: run.preflight.scope.id,
    generation: binding.generation, action: 'submit', payload: { conversationId: binding.conversationId,
      messageId: digest([run.runId, slot.index, 'message']), changeId: digest([run.runId, slot.index, 'change']), targets: [binding.sharedBotId],
      text: `只读验收。读取已共享的P0稳定性资料，返回单个JSON对象，只有probeId和checksum两字段。probeId为${slot.probeId}；checksum为共享资料指定的SHA-256校验值。不得写清单、文档或外部系统。` } };
}

export function observe(run, slot, result, now) {
  if (terminal.has(slot.status)) return; // A result redelivery is never another successful probe.
  if (result?.state !== 'completed') return;
  const fail = reason => { slot.status = 'failed'; slot.reason = reason; };
  if (result.taskId !== slot.taskId) return fail('wrong_task');
  if (now > slot.deadline) return fail('late_result');
  if (run.slots.some(s => s !== slot && (s.taskId === result.taskId || (result.resultId && s.resultId === result.resultId)))) return fail('duplicate_result');
  let answer;
  try { answer = JSON.parse(result.text); } catch { return fail('invalid_answer'); }
  if (!answer || Object.keys(answer).sort().join(',') !== 'checksum,probeId' || answer.probeId !== slot.probeId || answer.checksum !== run.checksum) return fail('incorrect_answer');
  const p = result.provider;
  if (!nonempty(result.resultId) || !['provider_api', 'real_process_verified_route'].includes(p?.source) || p.id !== run.preflight.provider.id || p.model !== run.preflight.provider.model || !nonempty(p.evidenceRef)) return fail('provider_evidence_required');
  const dispatched = result.dispatchedRequests ?? result.usage?.modelCalls;
  if (!bounded(dispatched) || dispatched < 1 || dispatched > run.preflight.limits.physical_dispatches) return fail('physical_dispatch_limit_or_evidence');
  // Missing/Partial cost is retained as unknown; it does not invalidate a correct business result.
  slot.usage = reportedUsage(result.usage);
  slot.dispatchedRequests = dispatched;
  slot.resultId = result.resultId; slot.resultDigest = digest(result.text); slot.providerEvidenceRef = p.evidenceRef;
  slot.providerRequestId = p.requestId ?? null; slot.providerRequestIdStatus = p.requestId ? 'reported' : 'missing'; slot.observedAt = now;
  slot.status = 'passed';
}

// One bounded scheduler tick. The caller owns sleeping, signal handling and atomic persistence.
// Persist before admission: a crash with unknown admission is failed on resume, never re-submitted.
export async function tick(run, { clock, adapter, binding, save, signal }) {
  validateRun(run);
  for (const slot of run.slots) {
    if (signal?.aborted) return;
    let now = clock();
    if (slot.status === 'submitting') { slot.status = 'failed'; slot.reason = 'admission_outcome_unknown'; await save(run); continue; }
    if (slot.status === 'pending' && now >= slot.scheduledAt) {
      if (now >= slot.scheduledAt + POLICY.intervalMs) { slot.status = 'missed'; slot.reason = 'missed_slot'; await save(run); continue; }
      slot.dispatchDelayMs = now - slot.scheduledAt; // Diagnostic only; never moves the fixed slot or deadline.
      slot.status = 'submitting'; slot.submittedAt = now; slot.deadline = now + POLICY.deadlineMs;
      await save(run);
      try {
        const response = await adapter.submit(probeCommand(run, slot, binding), { signal });
        assert.ok(nonempty(response.taskId));
        slot.taskId = response.taskId; slot.status = 'observing';
      } catch { slot.status = 'failed'; slot.reason = 'admission_error_or_unknown'; }
      await save(run);
    }
    now = clock();
    if (slot.status === 'observing') {
      if (now > slot.deadline) { slot.status = 'failed'; slot.reason = 'deadline'; }
      else {
        try {
          const result = await adapter.poll(slot.taskId, { signal });
          observe(run, slot, result, clock());
          if (result && ['failed', 'expired', 'cancelled', 'needs_reconciliation', 'interrupted', 'authorization_required'].includes(result.state)) { slot.status = 'failed'; slot.reason = 'task_failed'; }
        } catch { slot.pollErrors = (slot.pollErrors ?? 0) + 1; } // No error/body/credentials are logged.
      }
      await save(run);
    }
  }
}

export function receipt(run, now) {
  validateRun(run);
  const correct = run.slots.filter(s => s.status === 'passed').length;
  const complete = now >= run.endAt && run.slots.every(s => terminal.has(s.status));
  const live = run.preflight.mode === 'live';
  const submitted = run.slots.filter(s => s.submittedAt !== undefined);
  const reported = submitted.filter(s => s.usage);
  const unknown = submitted.length - reported.length;
  const missing = reported.filter(s => s.usage.status === 'missing').length;
  const partial = reported.filter(s => s.usage.status === 'partial').length;
  const completeUsage = submitted.length > 0 && unknown === 0 && missing === 0 && partial === 0;
  const known = key => reported.filter(s => bounded(s.usage[key])).reduce((sum, s) => sum + s.usage[key], 0);
  const knownCalls = submitted.filter(s => bounded(s.dispatchedRequests)).reduce((sum, s) => sum + s.dispatchedRequests, 0);
  const allCallsKnown = submitted.length > 0 && submitted.every(s => bounded(s.dispatchedRequests));
  return { schemaVersion: 1, runId: run.runId, provenance: live ? 'live' : `non-live:${run.preflight.mode}`,
    passed: complete && live && correct === POLICY.slots, complete, correct, denominator: POLICY.slots,
    failed: run.slots.filter(s => ['failed', 'missed'].includes(s.status)).length,
    startAt: run.startAt, endAt: run.endAt, limits: run.preflight.limits,
    usage: { status: reported.length === 0 || missing === reported.length ? 'missing' : completeUsage ? 'complete' : 'partial',
      inputTokens: completeUsage ? known('inputTokens') : null, outputTokens: completeUsage ? known('outputTokens') : null,
      knownInputTokens: known('inputTokens'), knownOutputTokens: known('outputTokens'),
      dispatchedRequests: allCallsKnown ? knownCalls : null, knownDispatchedRequests: knownCalls,
      missingTasks: missing, partialTasks: partial, unknownTasks: unknown, submittedTasks: submitted.length, observedTasks: reported.length,
      costStatus: completeUsage && reported.every(s => bounded(s.usage.costMicros) && typeof s.usage.currency === 'string' && s.usage.currency === reported[0].usage.currency) ? 'reported' : 'missing_or_partial' },
    fixtureDigest: run.fixtureDigest, scope: run.preflight.scope, package: run.preflight.package, provider: run.preflight.provider };
}
