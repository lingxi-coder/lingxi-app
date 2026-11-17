import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { constants, createReadStream } from 'node:fs';
import { open, mkdir, readdir, stat } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { isDeepStrictEqual as same } from 'node:util';
import { digest } from './team-value-fixtures.mjs';

const fail = code => { throw new Error(code); };
const id = value => typeof value === 'string' && /^[A-Za-z0-9_-]{8,80}$/u.test(value);
export async function sha256File(path) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest('hex');
}
export async function readPrivateJson(path, maxBytes = 128 * 1024) {
  const file = await open(resolve(path), constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const info = await file.stat();
    if (!info.isFile() || info.size > maxBytes || info.nlink !== 1 || (info.mode & 0o077)
      || (process.getuid && info.uid !== process.getuid())) fail('UNSAFE_QA_RECEIPT_FILE');
    return JSON.parse(await file.readFile('utf8'));
  } finally { await file.close(); }
}

/** Normal Desktop path: no QA wrapper or product change is needed. The existing host
 * writes runtime/results/<taskId>.json before emitting completion, and delivered()
 * removes the execution/outbox but retains that cached TeamRunResult. */
export async function createExistingJournalReader({ candidateFile, ownerRoot, initialTaskId }) {
  const candidate = await verifyCandidate(await readPrivateJson(candidateFile));
  const root = resolve(ownerRoot);
  const bindingPath = join(root, 'bindings', digest([candidate.scope.id, candidate.binding.sharedBotId]));
  const publication = async () => {
    await verifyCandidate(candidate);
    let binding;
    try { binding = await readPrivateJson(bindingPath); } catch { fail('DESKTOP_PUBLISHED_BINDING_UNAVAILABLE'); }
    if (binding.schemaVersion !== 1 || binding.teamId !== candidate.scope.id || binding.sharedBotId !== candidate.binding.sharedBotId
      || binding.revision !== candidate.package.sharedBotRevision || !/^[a-f0-9]{64}$/u.test(binding.snapshot)) fail('DESKTOP_PUBLISHED_BINDING_MISMATCH');
    // Exactly one already-published snapshot, selected by its immutable binding. No account store,
    // credentials file, transport spool or unrelated private QA records are enumerated/read.
    const snapshotPath = join(root, 'snapshots', binding.snapshot);
    let snapshot;
    try {
      if (await sha256File(snapshotPath) !== binding.snapshot) fail('PUBLISHED_SNAPSHOT_DIGEST_MISMATCH');
      snapshot = await readPrivateJson(snapshotPath, 4 * 1024 * 1024);
    } catch { fail('PUBLISHED_SNAPSHOT_UNAVAILABLE_OR_CHANGED'); }
    if (!same(route(snapshot), candidate.modelRoute)) fail('PUBLISHED_MODEL_ROUTE_MISMATCH');
    return binding;
  };
  const optional = async path => {
    try { return await readPrivateJson(path, 512 * 1024); }
    catch (error) { if (error.code === 'ENOENT') return undefined; throw error; }
  };
  const readTask = async ({ scope, taskId }) => {
    if (!id(taskId) || !same(scope, candidate.scope)) fail('RECEIPT_SCOPE_MISMATCH');
    const binding = await publication();
    let result;
    try { result = await readPrivateJson(join(root, 'runtime', 'results', `${taskId}.json`), 512 * 1024); }
    catch { fail('DESKTOP_CACHED_TEAM_RUN_RESULT_UNAVAILABLE'); }
    const s = result.cursor?.scope;
    if (result.version !== 3 || result.type !== 'result' || result.status !== 'completed' || typeof result.text !== 'string'
      || !s || s.teamId !== scope.id || s.sharedBotId !== candidate.binding.sharedBotId || s.conversationId !== candidate.binding.conversationId
      || !Number.isSafeInteger(result.dispatchedRequests) || result.dispatchedRequests < 1
      || result.dispatchedRequests > candidate.limits.physical_dispatches) fail('DESKTOP_RESULT_SCOPE_OR_REAL_DISPATCH_UNVERIFIED');
    const execution = await optional(join(root, 'executions', taskId));
    if (execution && (execution.schemaVersion !== 2 || execution.taskId !== taskId || execution.teamId !== scope.id
      || execution.scope?.teamId !== scope.id || execution.scope?.sharedBotId !== s.sharedBotId
      || execution.scope?.conversationId !== s.conversationId
      || !Number.isSafeInteger(execution.dispatched) || execution.dispatched > result.dispatchedRequests)) fail('DESKTOP_SAVED_EXECUTION_MISMATCH');
    const delivery = await optional(join(root, 'outbox', taskId));
    if (delivery && (delivery.schemaVersion !== 1 || delivery.taskId !== taskId || delivery.teamId !== scope.id
      || delivery.synthesized || delivery.payload?.outcome !== 'completed' || digest(delivery.payload.text) !== digest(result.text))) fail('DESKTOP_SAVED_DELIVERY_MISMATCH');
    return { evidenceKind: 'real_process_verified_route', observation: 'existing_desktop_cached_result', provenance: candidate.provenance,
      scope, taskId, requester: candidate.requester, sharedBotId: s.sharedBotId, conversationId: s.conversationId,
      package: candidate.package, textDigest: digest(result.text), invocationId: null, invocationIdStatus: 'not_in_cached_result',
      provider: { source: 'real_process_verified_route', id: candidate.modelRoute.provider, model: candidate.modelRoute.model,
        requestId: null, requestIdStatus: 'missing', evidenceRef: `desktop-cached-result-${taskId}` },
      dispatchedRequests: result.dispatchedRequests, usage: reportedUsage(delivery?.payload?.usage ?? result.usage),
      limits: candidate.limits, limitEvidence: 'reviewed_fixed_connector_limits', evidenceRef: `desktop-cached-result-${taskId}`,
      publicationSnapshotDigest: binding.snapshot };
  };
  return { readTask, async inspect({ scope }) {
    if (!id(initialTaskId)) fail('INITIAL_REAL_TASK_RECEIPT_REQUIRED');
    const prior = await readTask({ scope, taskId: initialTaskId });
    return { ...prior, inspectedAt: Date.now(), isolatedQa: candidate.isolatedQa, package: candidate.package,
      provider: { ...prior.provider, taskId: initialTaskId }, quotas: candidate.quotas, limits: candidate.limits };
  } };
}
const writeOnce = async (path, value) => {
  const file = await open(path, 'wx', 0o600);
  try { await file.writeFile(`${JSON.stringify(value, null, 2)}\n`); await file.sync(); }
  finally { await file.close(); }
};
export function reportedUsage(raw) {
  if (!raw || raw.status === 'missing') return { status: 'missing' };
  if (!['complete', 'partial'].includes(raw.status)) fail('INVALID_REPORTED_USAGE');
  const value = { status: raw.status };
  for (const key of ['inputTokens', 'outputTokens', 'costMicros']) {
    if (raw[key] !== undefined) {
      if (!Number.isSafeInteger(raw[key]) || raw[key] < 0) fail('INVALID_REPORTED_USAGE');
      value[key] = raw[key];
    }
  }
  if (typeof raw.currency === 'string' && /^[A-Z]{3}$/u.test(raw.currency)) value.currency = raw.currency;
  if (value.status === 'complete' && (value.inputTokens === undefined || value.outputTokens === undefined)) value.status = 'partial';
  return value;
}
function route(model) {
  const endpoint = new URL(model.model_endpoint);
  if (endpoint.protocol !== 'https:' || endpoint.username || endpoint.password || endpoint.search || endpoint.hash) fail('REAL_PROVIDER_HTTPS_ROUTE_REQUIRED');
  return { provider: model.model_provider, model: model.model_id, protocol: model.model_protocol, origin: endpoint.origin };
}
async function verifyCandidate(candidate) {
  if (Object.keys(candidate).some(k => !['schemaVersion', 'provenance', 'scope', 'binding', 'requester', 'isolatedQa', 'package', 'host', 'worker', 'processModule', 'modelRoute', 'limits', 'quotas'].includes(k))) fail('UNEXPECTED_CANDIDATE_FIELD');
  if (!['packaged', 'source', 'fixture'].includes(candidate.provenance) || !id(candidate.scope?.id)
    || !['team', 'service'].includes(candidate.scope.kind) || !candidate.isolatedQa) fail('FROZEN_QA_CANDIDATE_REQUIRED');
  for (const key of ['host', 'worker', 'processModule']) {
    const entry = candidate[key];
    if (!entry?.path || !/^[a-f0-9]{64}$/u.test(entry.sha256) || await sha256File(entry.path) !== entry.sha256) fail('CANDIDATE_BINARY_OR_RUNNER_DRIFT');
    if (key !== 'processModule' && !((await stat(entry.path)).mode & 0o111)) fail('EXECUTABLE_BINARY_REQUIRED');
  }
  if (!candidate.package?.revision || !/^[a-f0-9]{64}$/u.test(candidate.package?.sha256)
    || !Number.isSafeInteger(candidate.package?.sharedBotRevision) || !candidate.package.evidenceRef) fail('FROZEN_PACKAGE_REQUIRED');
  for (const key of ['context_window_tokens', 'output_tokens_per_request', 'physical_dispatches']) {
    if (!Number.isSafeInteger(candidate.limits?.[key]) || candidate.limits[key] < 1) fail('PER_TASK_LIMITS_REQUIRED');
  }
  if (!candidate.modelRoute?.provider || !candidate.modelRoute.model || !candidate.modelRoute.protocol
    || new URL(candidate.modelRoute.origin).protocol !== 'https:') fail('FROZEN_MODEL_ROUTE_REQUIRED');
  return candidate;
}

/** QA-only wrapper around the ACTUAL, frozen process module's runTeamHost export.
 * No injected run/spawn implementation, provider proxy or protocol change. Source and fixture
 * candidates are labeled separately and cannot establish signed-package acceptance. */
export async function createHostReceiptRecorder({ receiptDir, candidateFile }) {
  const candidate = await verifyCandidate(await readPrivateJson(candidateFile));
  const root = resolve(receiptDir);
  await mkdir(root, { recursive: true, mode: 0o700 });
  const info = await stat(root);
  if (info.mode & 0o077) fail('PRIVATE_RECEIPT_DIRECTORY_REQUIRED');
  const manifestPath = join(root, 'candidate.json');
  try { await writeOnce(manifestPath, candidate); }
  catch (error) { if (error.code !== 'EEXIST') throw error; assert.deepEqual(await readPrivateJson(manifestPath), candidate, 'Frozen candidate cannot change'); }
  const { runTeamHost } = await import(pathToFileURL(resolve(candidate.processModule.path)).href);
  if (typeof runTeamHost !== 'function') fail('ACTUAL_RUN_TEAM_HOST_EXPORT_REQUIRED');
  return async function recordedRunTeamHost(options) {
    const b = options.bootstrap;
    // Introspection calls need no provider receipt. The same real process implementation handles them.
    if (!['run', 'continue'].includes(b?.type)) return runTeamHost(options);
    await verifyCandidate(candidate);
    if (options.spawnProcess || resolve(options.executable) !== resolve(candidate.host.path)
      || resolve(b.workerExecutable) !== resolve(candidate.worker.path)) fail('ACTUAL_FIXED_BINARIES_REQUIRED');
    if (!id(b.taskId) || !id(b.invocationId) || b.scope?.teamId !== candidate.scope.id
      || b.scope.sharedBotId !== candidate.binding.sharedBotId || b.scope.conversationId !== candidate.binding.conversationId
      || !same(route(b.model), candidate.modelRoute)) fail('HOST_SCOPE_TASK_OR_ROUTE_MISMATCH');
    for (const key of ['context_window_tokens', 'output_tokens_per_request', 'physical_dispatches']) {
      if (!Number.isSafeInteger(b.limits?.[key]) || b.limits[key] < 1 || b.limits[key] > candidate.limits[key]) fail('PER_TASK_LIMIT_MISMATCH');
    }
    const before = Date.now();
    let result;
    try { result = await runTeamHost(options); }
    catch (error) {
      await writeOnce(join(root, `${b.taskId}--${b.invocationId}.json`), {
        schemaVersion: 1, evidenceKind: 'real_process_verified_route', provenance: candidate.provenance,
        taskId: b.taskId, invocationId: b.invocationId, scope: candidate.scope, startedAt: before, finishedAt: Date.now(), outcome: 'process_error',
      });
      throw error; // Caller owns its existing handling; the receipt never copies the error text.
    }
    const value = { schemaVersion: 1, evidenceKind: 'real_process_verified_route', provenance: candidate.provenance,
      scope: candidate.scope, requester: candidate.requester, sharedBotId: b.scope.sharedBotId, conversationId: b.scope.conversationId,
      taskId: b.taskId, invocationId: b.invocationId, startedAt: before, finishedAt: Date.now(),
      candidateDigest: digest(candidate), package: candidate.package,
      binaries: { host: candidate.host.sha256, worker: candidate.worker.sha256, processModule: candidate.processModule.sha256 },
      modelRoute: route(b.model), limits: Object.fromEntries(['context_window_tokens', 'output_tokens_per_request', 'physical_dispatches'].map(k => [k, b.limits[k]])),
      outcome: result.status, dispatchedRequests: result.dispatchedRequests ?? null,
      attemptDispatchedRequests: result.attemptDispatchedRequests ?? null,
      textDigest: typeof result.text === 'string' ? digest(result.text) : null,
      usage: reportedUsage(result.usage), providerRequestId: { status: 'missing' },
    };
    await writeOnce(join(root, `${b.taskId}--${b.invocationId}.json`), value);
    return result; // Unchanged TeamRunResult delivered through the existing connector/API.
  };
}

/** Read-only adapter over receipts produced by the wrapper. Reads only its dedicated directory,
 * never a Desktop snapshot (which contains instructions/credentials) or all private QA records. */
export async function createReceiptReader({ receiptDir, initialTaskId }) {
  const root = resolve(receiptDir);
  let candidate;
  try { candidate = await verifyCandidate(await readPrivateJson(join(root, 'candidate.json'))); }
  catch { fail('ACTUAL_HOST_CANDIDATE_RECEIPT_UNAVAILABLE'); }
  const readTask = async ({ scope, taskId }) => {
    if (!id(taskId) || !same(scope, candidate.scope)) fail('RECEIPT_SCOPE_MISMATCH');
    await verifyCandidate(candidate);
    const names = (await readdir(root)).filter(name => name.startsWith(`${taskId}--`) && name.endsWith('.json'));
    const rows = [];
    for (const name of names) {
      if (!/^[A-Za-z0-9_-]+--[A-Za-z0-9_-]+\.json$/u.test(name)) fail('INVALID_RECEIPT_NAME');
      rows.push(await readPrivateJson(join(root, name)));
    }
    const r = rows.filter(r => r.outcome === 'completed').sort((a, b) => b.finishedAt - a.finishedAt)[0];
    if (!r) fail('ACTUAL_TASK_RECEIPT_UNAVAILABLE');
    if (r.evidenceKind !== 'real_process_verified_route' || r.provenance !== candidate.provenance
      || r.candidateDigest !== digest(candidate) || r.taskId !== taskId || !same(r.scope, scope) || !same(r.package, candidate.package)
      || !same(r.modelRoute, candidate.modelRoute) || r.sharedBotId !== candidate.binding.sharedBotId
      || r.conversationId !== candidate.binding.conversationId || !same(r.requester, candidate.requester)
      || r.binaries?.host !== candidate.host.sha256 || r.binaries?.worker !== candidate.worker.sha256
      || r.binaries?.processModule !== candidate.processModule.sha256 || !id(r.invocationId)
      || !Number.isSafeInteger(r.dispatchedRequests) || r.dispatchedRequests < 1 || r.dispatchedRequests > candidate.limits.physical_dispatches) fail('ACTUAL_TASK_RECEIPT_IDENTITY_MISMATCH');
    for (const key of ['context_window_tokens', 'output_tokens_per_request', 'physical_dispatches']) {
      if (!Number.isSafeInteger(r.limits?.[key]) || r.limits[key] < 1 || r.limits[key] > candidate.limits[key]) fail('PER_TASK_LIMIT_MISMATCH');
    }
    return { evidenceKind: r.evidenceKind, provenance: r.provenance, scope, taskId,
      requester: r.requester, sharedBotId: r.sharedBotId, conversationId: r.conversationId,
      package: r.package, textDigest: r.textDigest, invocationId: r.invocationId,
      provider: { source: 'real_process_verified_route', id: r.modelRoute.provider, model: r.modelRoute.model,
        requestId: null, requestIdStatus: 'missing', evidenceRef: `${taskId}--${r.invocationId}.json` },
      dispatchedRequests: r.dispatchedRequests, usage: reportedUsage(r.usage), limits: r.limits,
      evidenceRef: `${taskId}--${r.invocationId}.json` };
  };
  return { readTask, async inspect({ scope }) {
    if (!id(initialTaskId)) fail('INITIAL_REAL_TASK_RECEIPT_REQUIRED');
    const prior = await readTask({ scope, taskId: initialTaskId });
    return { ...prior, inspectedAt: Date.now(), isolatedQa: candidate.isolatedQa,
      package: candidate.package, provider: { ...prior.provider, taskId: initialTaskId },
      quotas: candidate.quotas, limits: candidate.limits };
  } };
}
