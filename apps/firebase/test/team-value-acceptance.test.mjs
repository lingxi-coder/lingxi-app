import test from 'node:test';
import assert from 'node:assert/strict';
import { FIXTURES, gradeWorkflows, SHARED_CHECKSUM, workflowCommand } from '../scripts/team-value-fixtures.mjs';
import { createRun, tick, observe, receipt, probeCommand, POLICY, validateRun } from '../scripts/team-value-stability.mjs';
import { sharedBotCommand, serviceBotCommand } from '../src/shared-bot-commands.mjs';

const preflight = mode => ({ mode, scope: { kind: 'team', id: 'qa_team_01', evidenceRef: 'scope-inspection.json' },
  isolatedQa: true, package: { revision: 'qa-published-r1', sha256: 'a'.repeat(64), evidenceRef: 'package-inspection.json' },
  provider: { id: 'actual-provider', model: 'actual-model', evidenceRef: 'provider-inspection.json' },
  quotas: { limits: { team: 200, member: 200, bot: 200 }, remaining: { team: 200, member: 200, bot: 200 } },
  limits: { context_window_tokens: 2_000_000, output_tokens_per_request: 4096, physical_dispatches: 32 } });
const newRun = (mode = 'live') => createRun({ runId: 'qa_run_0001', startAt: 1_000_000, preflight: preflight(mode) });
const binding = { conversationId: 'qa_chat_01', sharedBotId: 'qa_bot_001', generation: 1 };
const result = slot => ({ taskId: slot.taskId, resultId: `result_${slot.index}`, state: 'completed',
  text: JSON.stringify({ probeId: slot.probeId, checksum: SHARED_CHECKSUM }),
  provider: { source: 'provider_api', id: 'actual-provider', model: 'actual-model', evidenceRef: `trace_${slot.index}`, requestId: `provider_request_${slot.index}` },
  dispatchedRequests: 1, usage: { status: 'complete', inputTokens: 10, outputTokens: 5 } });
const save = async () => {};
function observing(run, index = 0) {
  const slot = run.slots[index];
  Object.assign(slot, { status: 'observing', taskId: `task_${index}`, submittedAt: slot.scheduledAt, deadline: slot.scheduledAt + POLICY.deadlineMs });
  return slot;
}
test('144 missed/failed slots cannot pass, and denominator remains fixed', async () => {
  const run = newRun();
  await tick(run, { clock: () => run.endAt, adapter: { submit: () => assert.fail('must not backfill') }, binding, save });
  assert.equal(receipt(run, run.endAt).passed, false);
  assert.equal(receipt(run, run.endAt).failed, 144);
  assert.equal(receipt(run, run.endAt).denominator, 144);
});
test('144 correctly shaped provider receipts pass only after the full anchored 24h window (injected transport)', async () => {
  const run = newRun();
  let now = run.startAt, submitted = 0;
  const adapter = { async submit(command) {
    sharedBotCommand.parse(command);
    return { taskId: `task_${submitted++}` };
  }, async poll(taskId) { return result(run.slots.find(s => s.taskId === taskId)); } };
  for (let index = 0; index < 144; index++) {
    now = run.startAt + index * POLICY.intervalMs;
    await tick(run, { clock: () => now, adapter, binding, save });
  }
  assert.equal(submitted, 144);
  assert.equal(receipt(run, now).passed, false);
  assert.equal(receipt(run, run.endAt).passed, true);
  assert.equal(receipt(run, run.endAt).usage.inputTokens, 1440);
  assert.equal(receipt(run, run.endAt).usage.outputTokens, 720);
  assert.equal(receipt(run, run.endAt).usage.dispatchedRequests, 144);
});
test('fixture/mock runs always remain non-live even with 144 correct results', () => {
  for (const mode of ['fixture', 'mock']) {
    const run = newRun(mode);
    run.slots.forEach((_, i) => { const slot = observing(run, i); observe(run, slot, result(slot), slot.scheduledAt); });
    const summary = receipt(run, run.endAt);
    assert.equal(summary.correct, 144); assert.equal(summary.passed, false);
    assert.equal(summary.provenance, `non-live:${mode}`);
  }
});
test('wrong probe, checksum, task, provider and late result fail', () => {
  for (const mutate of [
    r => { r.text = JSON.stringify({ probeId: 'wrong', checksum: SHARED_CHECKSUM }); },
    r => { r.text = JSON.stringify({ probeId: 'qa_run_0001_000', checksum: 'wrong' }); },
    r => { r.taskId = 'wrong'; }, r => { r.provider.source = 'fixture'; },
  ]) {
    const run = newRun(), slot = observing(run), r = result(slot); mutate(r);
    observe(run, slot, r, slot.scheduledAt + 1);
    assert.equal(slot.status, 'failed');
  }
  const run = newRun(), slot = observing(run);
  observe(run, slot, result(slot), slot.deadline + 1);
  assert.equal(slot.reason, 'late_result');
});
test('duplicate delivery is not counted twice; reused task/result IDs fail another slot', () => {
  const run = newRun(), first = observing(run);
  observe(run, first, result(first), first.scheduledAt);
  observe(run, first, result(first), first.scheduledAt + 1);
  assert.equal(receipt(run, first.scheduledAt).usage.dispatchedRequests, 1);
  const second = observing(run, 1), r = result(second); r.resultId = first.resultId;
  observe(run, second, r, second.scheduledAt);
  assert.equal(second.reason, 'duplicate_result');
});
test('resume preserves old deadline, observes admitted task and never backfills missed slots', async () => {
  const original = newRun(), first = observing(original);
  const run = validateRun(JSON.parse(JSON.stringify(original)));
  let polls = 0;
  await tick(run, { clock: () => first.deadline - 1, adapter: { submit: () => assert.fail('no resubmit'),
    async poll() { polls++; return result(run.slots[0]); } }, binding, save });
  assert.equal(polls, 1); assert.equal(run.slots[0].status, 'passed');
  await tick(run, { clock: () => run.slots[1].scheduledAt + POLICY.intervalMs,
    adapter: { async submit(command) { assert.ok(command.payload.text.includes(run.slots[2].probeId)); return { taskId: 'task_2' }; } }, binding, save });
  assert.equal(run.slots[1].status, 'missed');
  assert.equal(run.slots[0].deadline, first.deadline);
  run.slots[0].deadline++;
  assert.throws(() => validateRun(run));
});
test('1.1 second or longer dispatch delay within its own fixed slot is diagnostic only', async () => {
  for (const delay of [1100, 120000, POLICY.intervalMs - 1]) {
    const run = newRun(), now = run.startAt + delay;
    const adapter = { async submit() { return { taskId: 'task_0' }; }, async poll() { return result(run.slots[0]); } };
    await tick(run, { clock: () => now, adapter, binding, save });
    assert.equal(run.slots[0].status, 'passed'); assert.equal(run.slots[0].dispatchDelayMs, delay);
    assert.equal(run.slots[0].scheduledAt, run.startAt); assert.equal(run.slots[0].deadline, now + POLICY.deadlineMs);
  }
});
test('one complete usage and 143 submitted unknown/failed costs cannot aggregate as complete or zero', () => {
  const run = newRun();
  for (let i = 0; i < 144; i++) {
    const slot = observing(run, i);
    if (i === 0) observe(run, slot, result(slot), slot.submittedAt);
    else { slot.status = 'failed'; slot.reason = i % 2 ? 'admission_error_or_unknown' : 'deadline'; }
  }
  const summary = receipt(run, run.endAt);
  assert.equal(summary.usage.status, 'partial'); assert.equal(summary.usage.unknownTasks, 143);
  assert.equal(summary.usage.submittedTasks, 144); assert.equal(summary.usage.inputTokens, null);
  assert.equal(summary.usage.outputTokens, null); assert.equal(summary.usage.dispatchedRequests, null);
  assert.equal(summary.usage.knownInputTokens, 10); assert.equal(summary.usage.knownDispatchedRequests, 1);
  assert.equal(summary.usage.costStatus, 'missing_or_partial');
});
test('unknown admission after restart fails and is never resubmitted', async () => {
  const run = newRun(), slot = run.slots[0];
  Object.assign(slot, { status: 'submitting', submittedAt: slot.scheduledAt, deadline: slot.scheduledAt + POLICY.deadlineMs });
  await tick(run, { clock: () => slot.scheduledAt, adapter: { submit: () => assert.fail('must not replay uncertain admission') }, binding, save });
  assert.equal(slot.reason, 'admission_outcome_unknown');
});
test('poll errors preserve evidence and eventually fail without moving deadline', async () => {
  const run = newRun(), slot = observing(run), deadline = slot.deadline;
  const adapter = { async poll() { throw new Error('private response must not persist'); } };
  await tick(run, { clock: () => deadline - 1, adapter, binding, save });
  assert.equal(slot.pollErrors, 1); assert.equal(slot.deadline, deadline);
  await tick(run, { clock: () => deadline + 1, adapter, binding, save });
  assert.equal(slot.reason, 'deadline');
  assert.ok(!JSON.stringify(run).includes('private response'));
});
test('preflight requires quota, package, provider and existing per-task output/call limits', () => {
  for (const mutate of [p => { p.quotas.limits.bot = 50; }, p => { p.quotas.remaining.member = 143; },
    p => { p.provider.evidenceRef = ''; }, p => { p.package.sha256 = ''; }, p => { p.limits.output_tokens_per_request = 0; }]) {
    const p = preflight('live'); mutate(p);
    assert.throws(() => createRun({ runId: 'qa_run_0001', startAt: 1, preflight: p }));
  }
});
test('Missing/Partial usage and absent/shared provider IDs do not fail correct business delivery', () => {
  for (const usage of [undefined, { status: 'missing' }, { status: 'partial', inputTokens: 10, outputTokens: 2 }]) {
    const run = newRun(), slot = observing(run), r = result(slot);
    r.usage = usage; delete r.provider.requestId;
    observe(run, slot, r, slot.scheduledAt);
    assert.equal(slot.status, 'passed'); assert.equal(slot.providerRequestIdStatus, 'missing');
    const summary = receipt(run, slot.scheduledAt);
    assert.equal(summary.usage.inputTokens, null); assert.equal(summary.usage.outputTokens, null);
    assert.equal(summary.usage.costStatus, 'missing_or_partial');
  }
  const run = newRun();
  for (let i = 0; i < 2; i++) {
    const slot = observing(run, i), r = result(slot); r.provider.requestId = 'same-provider-id';
    observe(run, slot, r, slot.scheduledAt); assert.equal(slot.status, 'passed');
  }
});
test('existing physical call cap is checked without any cumulative-window hard budget', () => {
  const run = newRun(), slot = observing(run), r = result(slot); r.dispatchedRequests = 33;
  observe(run, slot, r, slot.scheduledAt); assert.equal(slot.status, 'failed');
  assert.equal(slot.reason, 'physical_dispatch_limit_or_evidence');
  assert.ok(!('budget' in run.preflight));
});
test('stable envelope addresses exactly the existing scope and never publishes/edits', () => {
  const run = newRun();
  const command = probeCommand(run, run.slots[0], binding);
  sharedBotCommand.parse(command); assert.equal(command.action, 'submit');
  assert.ok(!command.payload.text.includes(SHARED_CHECKSUM));
  run.preflight.scope.kind = 'service';
  const service = probeCommand(run, run.slots[0], binding);
  serviceBotCommand.parse(service); assert.ok(!('teamId' in service));
});
function workflowRows() {
  const command = { schemaVersion: 1, commandId: 'family_f03_command', teamId: 'qa_team_01', generation: 1, action: 'submit',
    payload: { conversationId: 'qa_chat_01', messageId: 'family_f03_message', changeId: 'family_f03_change', targets: ['qa_bot_001'], text: FIXTURES.family[2].prompt } };
  const snapshot = { taskIds: ['family_f03_task'], quotaCounts: { team: 3, member: 2, bot: 3 }, usage: { status: 'complete', costMicros: 100 }, operationIds: ['op1'], listDataRevision: 3, historyDigest: 'history' };
  const rows = FIXTURES.family.concat(FIXTURES.team).map(f => ({ caseId: f.id, items: f.expected.map((i, index) => ({ ...i, itemId: `item_${f.id[0]}_${index}` })), taskId: f.id.startsWith('F0') && ['F03', 'F04'].includes(f.id) ? 'family_f03_task' : `task_${f.id}`,
    state: 'completed', sourceRef: 'redacted-evidence', participantRef: f.actor, observedAt: '2026-11-16T12:00:00Z',
    documentBody: f.expectedDocument, answer: f.expectedAnswer, differentDevice: true, historyVerified: true, sameArtifact: true,
    ...(['F03', 'F04'].includes(f.id) ? { command } : {}), ...(f.id === 'F04' ? { before: snapshot, after: snapshot } : {}) }));
  const edit = rows.find(r => r.caseId === 'T03');
  edit.participantIdentity = { uid: 'second_member_uid', generation: 2 };
  edit.command = { ...command, commandId: 'team_t03_command', generation: 2, payload: {
    ...command.payload, messageId: 'team_t03_message', changeId: 'team_t03_change', text: FIXTURES.team[2].prompt } };
  edit.requestMessage = { id: 'team_t03_message', changeId: 'team_t03_change', author: { kind: 'user', uid: 'second_member_uid', displayName: '团队成员二' }, createdAt: '2026-11-16T11:59:30Z' };
  edit.requestChange = { id: 'team_t03_change', messageId: 'team_t03_message', actorUid: 'second_member_uid', generation: 2, at: '2026-11-16T11:59:30Z' };
  const followup = rows.find(r => r.caseId === 'T05');
  followup.answer = `${FIXTURES.team[4].expectedAnswer}；B最后修改者团队成员二`;
  followup.modifierSource = { messageId: edit.requestChange.messageId, changeId: edit.requestChange.id,
    actorUid: edit.requestChange.actorUid, generation: edit.requestChange.generation, at: edit.requestChange.at };
  return rows;
}
test('independent F/T 5/5 and untouched C denominator; C identifiers do not satisfy missing cases', () => {
  const rows = workflowRows(), report = gradeWorkflows(rows);
  assert.equal(report.family.count, 5); assert.equal(report.team.count, 5); assert.equal(report.passed, true);
  assert.deepEqual(report.legacy, { denominator: 30, minimum: 27, status: 'not_graded_here' });
  rows[0].caseId = 'C01';
  const missing = gradeWorkflows(rows);
  assert.equal(missing.family.count, 0); assert.equal(missing.team.count, 5); assert.equal(missing.passed, false);
  assert.deepEqual(missing.unexpectedCases, ['C01']);
});
test('F04 must replay exact F03 command/task and preserve quota, usage and side effects', () => {
  for (const mutate of [r => { r.command.commandId = 'new_command_id'; }, r => { r.command.payload.messageId = 'new_message_id'; },
    r => { r.command.payload.changeId = 'new_change_id'; }, r => { r.taskId = 'new_task'; }, r => { r.after.usage.costMicros++; },
    r => { r.after.taskIds.push('duplicate'); }, r => { delete r.before.quotaCounts; }]) {
    const rows = JSON.parse(JSON.stringify(workflowRows())); mutate(rows.find(r => r.caseId === 'F04'));
    assert.equal(gradeWorkflows(rows).family.cases[3].passed, false);
  }
});
test('workflow command generator reuses F03 exact ids, text, actor and scope for F04', () => {
  const config = { runId: 'qa_workflow_01', scope: { kind: 'team', id: 'qa_team_01' }, binding: {
    ...binding, first: { generation: 1 }, second: { generation: 2 }, secondDevice: { generation: 2 } } };
  for (const f of FIXTURES.family.concat(FIXTURES.team)) sharedBotCommand.parse(workflowCommand(f.id, config).command);
  assert.deepEqual(workflowCommand('F04', config).command, workflowCommand('F03', config).command);
  assert.equal(workflowCommand('F04', config).actor, 'first');
  assert.throws(() => workflowCommand('C01', config));
});
test('reopening must prove another device, original artifact and history; stale team date fails', () => {
  const rows = workflowRows(); rows.find(r => r.caseId === 'F05').historyVerified = false;
  rows.find(r => r.caseId === 'T04').documentBody = 'B截止2026-11-21';
  const report = gradeWorkflows(rows);
  assert.equal(report.family.cases[4].passed, false); assert.equal(report.team.cases[3].passed, false);
});
test('same text in freshly recreated list items fails identity continuity', () => {
  const rows = workflowRows(); rows.find(r => r.caseId === 'F02').items[0].itemId = 'new_duplicate_item';
  assert.equal(gradeWorkflows(rows).family.cases[1].passed, false);
});
test('business prompts ask from shared source and never supply computed procurement or T05 answer', () => {
  const first = FIXTURES.family[0], updated = FIXTURES.family[1], last = FIXTURES.team[4];
  assert.ok(!first.prompt.includes('采购2盒') && !first.prompt.includes('采购4个'));
  assert.ok(!updated.prompt.includes('采购2个'));
  assert.ok(!last.prompt.includes(last.expectedAnswer));
  assert.ok(!last.prompt.includes('2026-11-22') && !last.prompt.includes('依赖B') && !last.prompt.includes('负责人丙'));
  assert.ok(!last.prompt.includes('团队成员二') && !last.prompt.includes('second_member_uid'));
  const rows = workflowRows();
  rows.find(r => r.caseId === 'T05').answer = `用户提供的期望答案是：${last.expectedAnswer}，但我没有读取实际清单。`;
  assert.equal(gradeWorkflows(rows).team.cases[4].passed, false);
});
test('T05 requires actual T03 author/change binding, correct modifier answer and historical ordering', () => {
  for (const mutate of [rows => { rows.find(r => r.caseId === 'T05').answer = FIXTURES.team[4].expectedAnswer; },
    rows => { rows.find(r => r.caseId === 'T05').answer = `${FIXTURES.team[4].expectedAnswer}；B最后修改者团队成员一`; },
    rows => { rows.find(r => r.caseId === 'T03').requestMessage.author.uid = 'first_member_uid'; },
    rows => { rows.find(r => r.caseId === 'T03').requestChange.generation = 1; },
    rows => { rows.find(r => r.caseId === 'T03').command.payload.messageId = 'different_message'; },
    rows => { rows.find(r => r.caseId === 'T05').modifierSource.changeId = 'different_change'; },
    rows => { rows.find(r => r.caseId === 'T05').observedAt = '2026-11-16T11:50:00Z'; }]) {
    const rows = workflowRows(); mutate(rows); assert.equal(gradeWorkflows(rows).team.cases[4].passed, false);
  }
  const rows = workflowRows(); rows.find(r => r.caseId === 'T05').answer = `${FIXTURES.team[4].expectedAnswer}；B最后修改者second_member_uid`;
  assert.equal(gradeWorkflows(rows).team.cases[4].passed, true);
  const publicReceipt = JSON.stringify(gradeWorkflows(rows));
  assert.ok(!publicReceipt.includes('second_member_uid') && !publicReceipt.includes('团队成员二'));
});
