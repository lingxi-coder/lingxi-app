// Exercises a fixture process module only. No real model or signed package acceptance is claimed.
import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, readFile, rm, readdir, mkdir, rename } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { randomUUID } from 'node:crypto';
import { createHostReceiptRecorder, createReceiptReader, createExistingJournalReader, sha256File } from '../scripts/team-value-host-receipts.mjs';
import { digest } from '../scripts/team-value-fixtures.mjs';

async function fixture() {
  const root = await mkdtemp(join(tmpdir(), 'team-host-receipts-'));
  const processPath = join(root, 'fixture-process.mjs'), host = join(root, 'fixture-host'), worker = join(root, 'fixture-worker');
  await writeFile(host, '#!/bin/sh\nexit 0\n', { mode: 0o700 });
  await writeFile(worker, '#!/bin/sh\nexit 0\n', { mode: 0o700 });
  await writeFile(processPath, 'export async function runTeamHost(options) {return {status:"completed",text:"synthetic-result",dispatchedRequests:1,usage:{status:"missing"}}}');
  const candidate = { schemaVersion: 1, provenance: 'fixture', scope: { kind: 'team', id: 'qa_team_01' }, isolatedQa: true,
    binding: { sharedBotId: 'qa_bot_001', conversationId: 'qa_chat_01' }, requester: { uid: 'qa_member_01', generation: 1 },
    package: { revision: 'fixture-package', sha256: 'a'.repeat(64), sharedBotRevision: 1, evidenceRef: 'fixture-only' },
    host: { path: host, sha256: await sha256File(host) }, worker: { path: worker, sha256: await sha256File(worker) }, processModule: { path: processPath, sha256: await sha256File(processPath) },
    modelRoute: { provider: 'fixture-provider', model: 'fixture-model', protocol: 'responses', origin: 'https://fixture.example.test' },
    limits: { context_window_tokens: 1000, output_tokens_per_request: 64, physical_dispatches: 2 } };
  const candidateFile = join(root, 'frozen-candidate.json'), receiptDir = join(root, 'receipts');
  await writeFile(candidateFile, JSON.stringify(candidate), { mode: 0o600 });
  const options = () => ({ executable: host, bootstrap: { type: 'run', workerExecutable: worker, taskId: 'qa_task_01', invocationId: randomUUID(),
    scope: { teamId: candidate.scope.id, ...candidate.binding, sessionEpoch: 1 },
    model: { model_provider: 'fixture-provider', model_id: 'fixture-model', model_protocol: 'responses', model_endpoint: 'https://fixture.example.test/v1', model_profile: {} },
    credentials: { neverPersist: 'secret-credential-marker' }, instructions: 'secret-instructions-marker', knowledge: [{ text: 'secret-shared-data-marker' }],
    limits: candidate.limits } });
  return { root, candidate, candidateFile, receiptDir, options };
}
test('wrapper calls fixed process module unchanged and read-only reader binds task/route/binaries/limits without request IDs', async () => {
  const f = await fixture();
  try {
    const run = await createHostReceiptRecorder(f), o = f.options(), original = structuredClone(o);
    const result = await run(o);
    assert.equal(result.status, 'completed'); assert.deepEqual(o, original);
    const reader = await createReceiptReader({ receiptDir: f.receiptDir, initialTaskId: 'qa_task_01' });
    const p = await reader.inspect({ scope: f.candidate.scope });
    assert.equal(p.provenance, 'fixture'); assert.equal(p.provider.requestId, null);
    assert.equal(p.dispatchedRequests, 1); assert.deepEqual(p.usage, { status: 'missing' });
    assert.equal(p.textDigest, digest('synthetic-result')); assert.equal(p.invocationId, o.bootstrap.invocationId);
    const contents = (await Promise.all((await readdir(f.receiptDir)).map(name => readFile(join(f.receiptDir, name), 'utf8')))).join('\n');
    for (const secret of ['secret-credential-marker', 'secret-instructions-marker', 'secret-shared-data-marker', 'synthetic-result']) assert.ok(!contents.includes(secret));
    await assert.rejects(reader.readTask({ scope: { ...f.candidate.scope, id: 'other_team_01' }, taskId: 'qa_task_01' }), /SCOPE_MISMATCH/u);
  } finally { await rm(f.root, { recursive: true }); }
});
test('actual wrapper refuses spawn injection, route/scope drift and per-task output/call excess', async () => {
  const f = await fixture();
  try {
    const run = await createHostReceiptRecorder(f);
    for (const mutate of [o => { o.spawnProcess = () => {}; }, o => { o.bootstrap.model.model_id = 'another-model'; },
      o => { o.bootstrap.scope.teamId = 'other_team_01'; }, o => { o.bootstrap.limits = { ...o.bootstrap.limits, physical_dispatches: 3 }; },
      o => { o.bootstrap.limits = { ...o.bootstrap.limits, output_tokens_per_request: 65 }; }]) {
      const o = f.options(); mutate(o); await assert.rejects(run(o));
    }
    assert.deepEqual(await readdir(f.receiptDir), ['candidate.json']);
  } finally { await rm(f.root, { recursive: true }); }
});
test('reader rejects missing actual receipts, changed binary hash and tampered provenance/task/result metadata', async () => {
  const f = await fixture();
  try {
    const run = await createHostReceiptRecorder(f);
    const reader = await createReceiptReader({ receiptDir: f.receiptDir, initialTaskId: 'qa_task_01' });
    await assert.rejects(reader.inspect({ scope: f.candidate.scope }), /ACTUAL_TASK_RECEIPT_UNAVAILABLE/u);
    const o = f.options(); await run(o);
    const path = join(f.receiptDir, `qa_task_01--${o.bootstrap.invocationId}.json`), original = JSON.parse(await readFile(path));
    for (const mutate of [r => { r.provenance = 'packaged'; }, r => { r.taskId = 'other_task_01'; },
      r => { r.package.sha256 = 'b'.repeat(64); }, r => { r.dispatchedRequests = 0; }]) {
      const r = structuredClone(original); mutate(r); await writeFile(path, JSON.stringify(r));
      await assert.rejects(reader.readTask({ scope: f.candidate.scope, taskId: 'qa_task_01' }));
    }
    await writeFile(path, JSON.stringify(original));
    await writeFile(f.candidate.worker.path, 'changed binary');
    await assert.rejects(reader.readTask({ scope: f.candidate.scope, taskId: 'qa_task_01' }), /BINARY_OR_RUNNER_DRIFT/u);
  } finally { await rm(f.root, { recursive: true }); }
});
async function desktopJournal(f) {
  const ownerRoot = join(f.root, 'normal-desktop-owner');
  for (const name of ['bindings', 'snapshots', 'executions', 'outbox', 'runtime/results']) await mkdir(join(ownerRoot, name), { recursive: true, mode: 0o700 });
  const snapshotTemp = join(ownerRoot, 'snapshots', 'temporary');
  await writeFile(snapshotTemp, JSON.stringify({ model_provider: 'fixture-provider', model_id: 'fixture-model', model_protocol: 'responses', model_endpoint: 'https://fixture.example.test/v1',
    credential_id: 'reference-only', instructions: 'private-published-body-marker', documents: [], provider_account_profiles: [] }), { mode: 0o600 });
  const snapshot = await sha256File(snapshotTemp); await rename(snapshotTemp, join(ownerRoot, 'snapshots', snapshot));
  await writeFile(join(ownerRoot, 'bindings', digest([f.candidate.scope.id, f.candidate.binding.sharedBotId])), JSON.stringify({ schemaVersion: 1,
    teamId: f.candidate.scope.id, sharedBotId: f.candidate.binding.sharedBotId, revision: 1, snapshot }), { mode: 0o600 });
  const result = { version: 3, type: 'result', status: 'completed', text: 'normal-desktop-result', finishedAt: Date.now(), dispatchedRequests: 1,
    cursor: { scope: { teamId: f.candidate.scope.id, ...f.candidate.binding, sessionEpoch: 1 } }, usage: { status: 'missing' } };
  const resultPath = join(ownerRoot, 'runtime', 'results', 'qa_task_01.json');
  await writeFile(resultPath, JSON.stringify(result), { mode: 0o600 });
  return { ownerRoot, result, resultPath };
}
test('normal Desktop reader works after ACK removed execution/outbox, without calling QA wrapper or a model', async () => {
  const f = await fixture();
  try {
    const j = await desktopJournal(f);
    const reader = await createExistingJournalReader({ candidateFile: f.candidateFile, ownerRoot: j.ownerRoot, initialTaskId: 'qa_task_01' });
    const p = await reader.inspect({ scope: f.candidate.scope });
    assert.equal(p.observation, 'existing_desktop_cached_result'); assert.equal(p.dispatchedRequests, 1);
    assert.equal(p.textDigest, digest('normal-desktop-result')); assert.equal(p.invocationId, null);
    assert.equal(p.provider.requestIdStatus, 'missing'); assert.equal(p.provenance, 'fixture');
    assert.ok(!JSON.stringify(p).includes('private-published-body-marker'));
    assert.deepEqual(await readdir(join(j.ownerRoot, 'executions')), []);
    assert.deepEqual(await readdir(join(j.ownerRoot, 'outbox')), []);
  } finally { await rm(f.root, { recursive: true }); }
});
test('normal Desktop cached reader rejects zero dispatch, wrong Chat, missing result and mismatched SavedDelivery', async () => {
  const f = await fixture();
  try {
    const j = await desktopJournal(f);
    const reader = await createExistingJournalReader({ candidateFile: f.candidateFile, ownerRoot: j.ownerRoot, initialTaskId: 'qa_task_01' });
    for (const mutate of [r => { r.dispatchedRequests = 0; }, r => { r.cursor.scope.conversationId = 'other_chat_01'; }]) {
      const r = structuredClone(j.result); mutate(r); await writeFile(j.resultPath, JSON.stringify(r));
      await assert.rejects(reader.readTask({ scope: f.candidate.scope, taskId: 'qa_task_01' }), /REAL_DISPATCH_UNVERIFIED/u);
    }
    await writeFile(j.resultPath, JSON.stringify(j.result));
    await writeFile(join(j.ownerRoot, 'outbox', 'qa_task_01'), JSON.stringify({ schemaVersion: 1, teamId: f.candidate.scope.id, taskId: 'qa_task_01', payload: { outcome: 'completed', text: 'a different task result' } }), { mode: 0o600 });
    await assert.rejects(reader.readTask({ scope: f.candidate.scope, taskId: 'qa_task_01' }), /SAVED_DELIVERY_MISMATCH/u);
    await rm(j.resultPath);
    await assert.rejects(reader.readTask({ scope: f.candidate.scope, taskId: 'qa_task_01' }), /CACHED_TEAM_RUN_RESULT_UNAVAILABLE/u);
  } finally { await rm(f.root, { recursive: true }); }
});
