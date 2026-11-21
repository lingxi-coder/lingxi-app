import { createHash } from 'node:crypto';
import { z } from 'zod';
import { DATA_CONTRACT_SHA256, DATA_PROTOCOL_VERSION, isResourceRef } from './dataset-resource.mjs';
import { deliverDatasetTaskOutput } from './dataset-task-delivery.mjs';
import { LingxiDataError } from './lingxi-data.mjs';
import { SharedBotsStore } from './shared-bots.mjs';
import { TeamMessagesStore } from './team-messages.mjs';

const id = z.string().regex(/^[A-Za-z0-9_-]{1,64}$/u);
const projection = z.object({ fields: z.array(id).min(1).max(64).optional(),
  partitions: z.array(z.string().min(1).refine(value => Buffer.byteLength(value) <= 160 && !/[\u0000-\u001f\u007f]/u.test(value))).min(1).max(64).optional() }).strict();
const publish = z.object({
  action: z.literal('share'), dataset: id, via: id.optional(), operation: id,
  destination: z.object({ team: id, conversation: id }).strict(),
  generation: z.number().int().min(1).max(Number.MAX_SAFE_INTEGER),
  audienceDigest: id, mode: z.enum(['pinned', 'live', 'edit']).default('pinned'),
  projection: projection.default({}), readers: z.record(id, projection).default({}), reshare: z.boolean().default(false),
  reference: z.custom(isResourceRef).optional(),
  sharedBot: id.optional(),
  text: z.string().min(1).refine(value => Buffer.byteLength(value) <= 8192),
}).strict();

export function createDatasetGateway(origin, fetchRequest = fetch) {
  if (!origin) return null;
  const url = new URL(origin);
  if (url.protocol !== 'https:' || url.pathname !== '/' || url.username || url.password || url.search || url.hash) {
    throw new Error('Expected an HTTPS Dataset API origin');
  }
  url.pathname = '/v2/datasets';
  const contractUrl = new URL('/v2/contract', url);
  let verifiedUntil = 0;
  let checking;
  async function checkContract() {
    if (Date.now() < verifiedUntil) return;
    const response = await fetchRequest(contractUrl, { redirect: 'error', cache: 'no-store', signal: AbortSignal.timeout(5000) });
    const contract = await response.json();
    if (!response.ok || contract.protocol !== DATA_PROTOCOL_VERSION || contract.contractSha256 !== DATA_CONTRACT_SHA256) {
      throw new LingxiDataError(503, 'DATA_CONTRACT_MISMATCH');
    }
    verifiedUntil = Date.now() + 60_000;
  }
  async function forward(path, authorization, body) {
    checking ??= checkContract().finally(() => { checking = undefined; });
    await checking;
    let response;
    try {
      response = await fetchRequest(new URL(path, url), { method: 'POST', redirect: 'error', cache: 'no-store',
        headers: { ...(authorization ? { Authorization: authorization } : {}), 'Content-Type': 'application/json' },
        body: JSON.stringify(body), signal: AbortSignal.timeout(30000) });
    } catch { throw new LingxiDataError(503, 'DATA_UNAVAILABLE'); }
    if (!response.ok) {
      const codes = { 400: 'INVALID_DATA_REQUEST', 401: 'DATA_ACCESS_REQUIRED', 403: 'DATA_ACCESS_REQUIRED',
        404: 'DATA_NOT_FOUND', 409: 'DATA_CONFLICT', 410: 'DATA_BACKFILL_REQUIRED', 429: 'DATA_QUOTA', 503: 'DATA_UNAVAILABLE' };
      throw new LingxiDataError(response.status, codes[response.status] ?? 'DATA_UNAVAILABLE');
    }
    return response.json();
  }
  function command(token, body) {
    const path = body?.command?.action === 'publishCapture' ? '/v2/report-captures' : '/v2/datasets';
    return forward(path, 'Bearer ' + token, body);
  }
  async function datasets(db, user, token, body) {
    if (body?.action !== 'share') return command(token, body);
    const input = publish.parse(body);
    const { dataset, via, operation, destination, mode, projection, readers, reshare, audienceDigest } = input;
    const call = (action, fields = {}) => command(token, { protocol: DATA_PROTOCOL_VERSION, dataset, via, operation, command: { action, ...fields } });
    let reference = input.reference;
    if (mode === 'pinned' && !reference) {
      let build = await call('startSnapshot', { projection });
      if (!isResourceRef(build.reference) || build.reference.kind !== 'snapshot') throw new LingxiDataError(503, 'DATA_UNAVAILABLE');
      reference = build.reference;
      const snapshot = reference.snapshot;
      if (!build.ready) build = await call('advanceSnapshot', { snapshot });
      if (!build.ready) return { status: 'building', reference: build.reference };
    }
    const textDigest = createHash('sha256').update(input.text).digest('base64url');
    const pending = await call('prepareShare', { share: { destination, mode, projection, readers, reshare, audienceDigest, reference, textDigest } });
    if (!isResourceRef(pending.reference) || !id.safeParse(pending.grant).success ||
        pending.messageId !== 'share_' + pending.grant) throw new LingxiDataError(503, 'DATA_UNAVAILABLE');
    // Only this trusted writer can attach a prepared grant. Replaying after a crash uses the same message and change IDs.
    const message = { conversationId: destination.conversation, changeId: pending.messageId, messageId: pending.messageId, text: input.text };
    const shared = { reference: pending.reference, grant: pending.grant };
    if (input.sharedBot) {
      const bots = new SharedBotsStore(db);
      await bots.execute(user, { action: 'submit', schemaVersion: 1, teamId: destination.team, generation: input.generation,
        commandId: pending.messageId, payload: { ...message, targets: [input.sharedBot] } }, shared);
    } else {
      await new TeamMessagesStore(db).execute(user, { action: 'mutate', operation: 'create', teamId: destination.team,
        generation: input.generation, ...message }, shared);
    }
    const result = await call('activateShare', { grant: pending.grant });
    return { status: 'published', messageId: pending.messageId, ...result };
  }
  async function run(db, authorization, body) {
    if (!/^Dataset(?:Task|Run) [A-Za-z0-9_-]{43}\.[A-Za-z0-9_-]{43}$/u.test(authorization)) {
      throw new LingxiDataError(403, 'DATA_ACCESS_REQUIRED');
    }
    const result = await forward('/v2/dataset-runs', authorization, body);
    if (result.status !== 'awaitingDelivery') return result;
    if (body?.command?.action !== 'publish' || !id.safeParse(result.delivery).success) throw new LingxiDataError(503, 'DATA_UNAVAILABLE');
    const state = await deliverDatasetTaskOutput(db, result.delivery);
    if (state !== 'delivered') throw new LingxiDataError(409, 'DATA_SHARE_CHANGED');
    return forward('/v2/dataset-runs', authorization, body);
  }
  const browserPaths = { host: '/v2/dataset-browser-host', open: '/v2/dataset-browser-open', invoke: '/v2/dataset-browser-invoke' };
  function browser(action, authorization, body) {
    if (!Object.hasOwn(browserPaths, action)) throw new LingxiDataError(400, 'INVALID_DATA_REQUEST');
    return forward(browserPaths[action], authorization, body);
  }
  return { datasets, run, browser };
}
