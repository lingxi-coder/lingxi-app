import assert from 'node:assert/strict';
import { constants } from 'node:fs';
import { open } from 'node:fs/promises';
import { resolve } from 'node:path';
import { randomUUID } from 'node:crypto';
import { isDeepStrictEqual } from 'node:util';
import { digest } from './team-value-fixtures.mjs';
import { createReceiptReader, createExistingJournalReader, reportedUsage } from './team-value-host-receipts.mjs';

const id = value => typeof value === 'string' && /^[A-Za-z0-9_-]{8,64}$/u.test(value);
const present = value => typeof value === 'string' && value.length > 0;
const same = isDeepStrictEqual;
const reject = code => { throw new Error(code); };

/** Reads only the explicitly supplied token file, on every request (external rotation works).
 * Never writes auth state, logs response bodies, follows redirects, or retries submissions. */
export async function tokenFromFile(path) {
  const file = await open(resolve(path), constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const stat = await file.stat();
    if (!stat.isFile() || stat.size > 32_768 || stat.nlink !== 1 || (stat.mode & 0o077)
      || (process.getuid && stat.uid !== process.getuid())) reject('UNSAFE_TOKEN_FILE');
    const value = (await file.readFile('utf8')).trim();
    const token = value.startsWith('{') ? JSON.parse(value).idToken : value;
    if (!present(token) || /\s/u.test(token)) reject('INVALID_TOKEN_FILE');
    return token;
  } finally { await file.close(); }
}

export function createHttpTransport({ baseUrl, tokenFile, getToken, fetcher = fetch }) {
  const base = new URL(baseUrl);
  if (base.username || base.password || base.search || base.hash
    || !(base.protocol === 'https:' || (base.protocol === 'http:' && ['127.0.0.1', 'localhost', '[::1]'].includes(base.hostname)))) reject('INVALID_API_BASE');
  if (!getToken && !tokenFile) reject('EXPLICIT_TOKEN_SOURCE_REQUIRED');
  const acquire = getToken ?? (() => tokenFromFile(tokenFile));
  return async (path, body, { signal } = {}) => {
    if (!['/v1/teams/messages', '/v1/services'].includes(path) && !/^\/v1\/teams\/[A-Za-z0-9_-]{8,64}$/u.test(path)) reject('UNSUPPORTED_API_PATH');
    // Auth is deliberately obtained fresh; refresh callbacks must keep secrets out of errors/logs.
    let credential;
    try { credential = await acquire({ signal }); } catch { reject('QA_AUTH_UNAVAILABLE'); }
    if (!present(credential) || /\s/u.test(credential)) reject('INVALID_TOKEN');
    try {
      const response = await fetcher(`${base.href.replace(/\/$/u, '')}${path}`, {
        method: body === undefined ? 'GET' : 'POST', redirect: 'error', cache: 'no-store',
        headers: { Authorization: `Bearer ${credential}`, ...(body === undefined ? {} : { 'Content-Type': 'application/json' }) },
        ...(body === undefined ? {} : { body: JSON.stringify(body) }),
        signal: signal ? AbortSignal.any([signal, AbortSignal.timeout(20_000)]) : AbortSignal.timeout(20_000),
      });
      if (!response.ok) reject(`QA_HTTP_${response.status}`);
      return await response.json();
    } catch (error) {
      // Do not surface network URLs, error response bodies, token refresh errors, or request bodies.
      if (/^QA_HTTP_\d{3}$/u.test(error?.message ?? '')) throw error;
      reject('QA_TRANSPORT_FAILED');
    }
  };
}

/** Actual API adapter over existing HTTP and QA runTeamHost sidecar receipts.
 * No runtime protocol change; config mode:'live' has no effect. */
export async function createAdapter({ config, getToken, fetcher, receiptReader } = {}) {
  const h = config?.http, binding = config?.binding, scope = h?.scope;
  assert.ok(h && ['team', 'service'].includes(scope?.kind) && id(scope.id), 'EXPLICIT_HTTP_SCOPE_REQUIRED');
  assert.ok(id(binding?.conversationId) && id(binding?.sharedBotId) && Number.isSafeInteger(binding?.generation) && binding.generation > 0);
  assert.ok(present(h.requesterUid), 'EXPECTED_REQUESTER_REQUIRED');
  const api = createHttpTransport({ baseUrl: h.baseUrl, tokenFile: h.tokenFile, getToken, fetcher });
  if (!receiptReader && h.desktopJournal) receiptReader = await createExistingJournalReader(h.desktopJournal);
  if (!receiptReader && h.hostReceipts) receiptReader = await createReceiptReader(h.hostReceipts);
  const scoped = () => ({ [scope.kind === 'service' ? 'serviceId' : 'teamId']: scope.id, generation: binding.generation });
  const execution = (action, payload, signal) => api('/v1/teams/messages', {
    schemaVersion: 1, commandId: randomUUID(), ...scoped(), action, payload,
  }, { signal });
  let verified;
  const expectedRequester = { uid: h.requesterUid, generation: binding.generation };
  const inspectReceipts = async signal => {
    if (!receiptReader || !['inspect', 'readTask'].every(k => typeof receiptReader[k] === 'function')) reject('ACTUAL_RUN_TEAM_HOST_RECEIPTS_UNAVAILABLE');
    const inspection = await receiptReader.inspect({ scope, binding, runId: config.runId, signal });
    if (inspection?.evidenceKind !== 'real_process_verified_route' || !['packaged', 'source', 'fixture'].includes(inspection.provenance) || !same(inspection.scope, scope)
      || !same(inspection.requester, expectedRequester) || inspection.sharedBotId !== binding.sharedBotId
      || inspection.conversationId !== binding.conversationId || inspection.isolatedQa !== true
      || !Number.isSafeInteger(inspection.inspectedAt) || Math.abs(Date.now() - inspection.inspectedAt) > 60_000) reject('UNVERIFIED_LIVE_SCOPE');
    if (!present(inspection.package?.evidenceRef) || !present(inspection.package?.revision)
      || !/^[a-f0-9]{64}$/u.test(inspection.package?.sha256) || !Number.isSafeInteger(inspection.package?.sharedBotRevision)) reject('UNVERIFIED_PUBLISHED_PACKAGE');
    if (inspection.provider?.source !== 'real_process_verified_route' || !present(inspection.provider?.id) || !present(inspection.provider?.model)
      || !present(inspection.provider?.evidenceRef)
      || !id(inspection.provider?.taskId)) reject('UNVERIFIED_REAL_PROVIDER');
    if (!['context_window_tokens', 'output_tokens_per_request', 'physical_dispatches'].every(k => Number.isSafeInteger(inspection.limits?.[k]) && inspection.limits[k] > 0)) reject('EXISTING_PER_TASK_LIMITS_UNAVAILABLE');
    return inspection;
  };
  return {
    async preflight({ signal } = {}) {
      // Always inspect the existing authenticated API scope first; never restore/create an account or scope.
      let directory;
      if (scope.kind === 'team') {
        const detail = await api(`/v1/teams/${scope.id}`, undefined, { signal });
        if (detail?.team?.id !== scope.id || !detail.members?.some(m => m.uid === h.requesterUid && m.generation === binding.generation)) reject('CURRENT_MEMBERSHIP_REQUIRED');
        directory = (await execution('listSharedBots', {}, signal)).bots?.find(b => b.sharedBotId === binding.sharedBotId);
        if (!directory?.permitted || !directory.ready || directory.state !== 'published') reject('QA_BOT_NOT_READY');
      } else {
        const service = await api('/v1/services', { action: 'getService', serviceId: scope.id }, { signal });
        if (service?.serviceId !== scope.id || service.sharedBotId !== binding.sharedBotId || !service.entered
          || service.generation !== binding.generation || service.availability !== 'available') reject('CURRENT_SERVICE_ENTRY_REQUIRED');
      }
      // Verify the existing Chat is readable, without saving any message body to local evidence.
      await api('/v1/teams/messages', { action: 'sync', ...scoped(), conversationId: binding.conversationId, after: 0 }, { signal });
      const inspection = await inspectReceipts(signal);
      const prior = await execution('getTask', { taskId: inspection.provider.taskId }, signal);
      const priorReceipt = await receiptReader.readTask({ scope, taskId: inspection.provider.taskId, signal });
      if (prior.taskId !== inspection.provider.taskId || prior.conversationId !== binding.conversationId || prior.sharedBotId !== binding.sharedBotId
        || !same(prior.requester, expectedRequester) || prior.state !== 'completed'
        || priorReceipt?.evidenceKind !== 'real_process_verified_route' || !same(priorReceipt.scope, scope)
        || priorReceipt.taskId !== prior.taskId || !same(priorReceipt.requester, expectedRequester)
        || priorReceipt.provider?.source !== 'real_process_verified_route' || !(priorReceipt.dispatchedRequests > 0)
        || priorReceipt.provider?.id !== inspection.provider.id || priorReceipt.provider?.model !== inspection.provider.model) reject('INITIAL_PROVIDER_TASK_IDENTITY_UNVERIFIED');
      if (directory && directory.revision !== inspection.package.sharedBotRevision) reject('PUBLISHED_REVISION_MISMATCH');
      const quotas = directory ? { limits: directory.quotas,
        remaining: { team: directory.dailyRemaining, member: directory.dailyRemaining, bot: directory.dailyRemaining } } : inspection.quotas;
      if (!quotas || !['team', 'member', 'bot'].every(k => quotas.limits?.[k] === 200 && Number.isSafeInteger(quotas.remaining?.[k]) && quotas.remaining[k] >= 0)) reject('QA_200_QUOTAS_UNVERIFIED');
      verified = inspection;
      return { mode: inspection.provenance === 'fixture' ? 'fixture' : 'live', isolatedQa: true, scope: { ...scope, evidenceRef: `qa-api-scope-${digest(scope)}` },
        package: { revision: inspection.package.revision, sha256: inspection.package.sha256, evidenceRef: inspection.package.evidenceRef, provenance: inspection.provenance },
        provider: { id: inspection.provider.id, model: inspection.provider.model, evidenceRef: inspection.provider.evidenceRef },
        quotas, limits: inspection.limits };
    },
    async submit(command, { signal } = {}) {
      if (!verified) reject('PREFLIGHT_REQUIRED');
      if (command.action !== 'submit' || command[scope.kind === 'service' ? 'serviceId' : 'teamId'] !== scope.id
        || command.generation !== binding.generation || command.payload?.conversationId !== binding.conversationId
        || !same(command.payload.targets, [binding.sharedBotId])) reject('COMMAND_BINDING_MISMATCH');
      // Exactly one HTTP admission, with original command IDs. Unknown outcomes are never retried here.
      const response = await api('/v1/teams/messages', command, { signal });
      if (response.tasks?.length !== 1 || !id(response.tasks[0].taskId)) reject('UNIQUE_TASK_ADMISSION_REQUIRED');
      return { taskId: response.tasks[0].taskId };
    },
    async poll(taskId, { signal } = {}) {
      if (!verified || !id(taskId)) reject('PREFLIGHT_OR_TASK_REQUIRED');
      const task = await execution('getTask', { taskId }, signal);
      if (task.taskId !== taskId || task.conversationId !== binding.conversationId || task.sharedBotId !== binding.sharedBotId
        || !same(task.requester, expectedRequester)) reject('TASK_IDENTITY_MISMATCH');
      if (task.state !== 'completed') return { taskId, state: task.state };
      if (!id(task.replyId)) reject('TASK_REPLY_REQUIRED');
      let after = 0, reply;
      // Each sync page is capped by the existing API. A nonadvancing cursor is an error, never silent truncation.
      do {
        const page = await api('/v1/teams/messages', { action: 'sync', ...scoped(), conversationId: binding.conversationId, after }, { signal });
        for (const message of page.messages ?? []) if (message.id === task.replyId) {
          if (message.taskId !== taskId || message.author?.kind !== 'shared_bot' || message.author.sharedBotId !== binding.sharedBotId
            || message.recalled || typeof message.text !== 'string') reject('REPLY_IDENTITY_MISMATCH');
          reply = message;
        }
        if (!page.hasMore) break;
        const next = page.changes?.at(-1)?.seq;
        if (!Number.isSafeInteger(next) || next <= after) reject('MESSAGE_SYNC_CURSOR_GAP');
        after = next;
      } while (!signal?.aborted);
      if (signal?.aborted) reject('QA_POLL_ABORTED');
      if (!reply) reject('COMPLETED_REPLY_NOT_VISIBLE');
      const evidence = await receiptReader.readTask({ scope, taskId, signal });
      if (evidence?.evidenceKind !== 'real_process_verified_route' || !same(evidence.scope, scope) || evidence.taskId !== taskId
        || evidence.conversationId !== binding.conversationId || evidence.sharedBotId !== binding.sharedBotId
        || !same(evidence.requester, expectedRequester) || evidence.textDigest !== digest(reply.text)
        || evidence.package?.revision !== verified.package.revision || evidence.package?.sha256 !== verified.package.sha256
        || evidence.provider?.source !== 'real_process_verified_route' || evidence.provider?.id !== verified.provider.id
        || evidence.provider?.model !== verified.provider.model || !(evidence.dispatchedRequests > 0)
        || evidence.dispatchedRequests > verified.limits.physical_dispatches
        || !present(evidence.evidenceRef)) reject('TASK_PROVIDER_RECEIPT_UNVERIFIED');
      return { taskId, resultId: task.replyId, state: 'completed', text: reply.text,
        provider: { source: 'real_process_verified_route', id: evidence.provider.id, model: evidence.provider.model,
          requestId: evidence.provider.requestId ?? null, requestIdStatus: evidence.provider.requestId ? 'reported' : 'missing', evidenceRef: evidence.evidenceRef },
        dispatchedRequests: evidence.dispatchedRequests, usage: reportedUsage(task.usage ?? evidence.usage) };
    },
  };
}
