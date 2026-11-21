import { createHash } from 'node:crypto';
import { z } from 'zod';
import { LingxiDataError } from './lingxi-data.mjs';

const id = z.string().regex(/^[A-Za-z0-9_-]{1,64}$/u);
const resource = { page: id, version: z.string().regex(/^[a-f0-9]{64}$/u) };
const session = z.uuid();
const bounds = { x: z.number().min(0).max(1440), y: z.number().min(0).max(900) };
const event = z.discriminatedUnion('kind', [
  z.object({ kind: z.literal('tap'), ...bounds }).strict(),
  z.object({ kind: z.literal('select'), start: z.object(bounds).strict(), end: z.object(bounds).strict() }).strict(),
  z.object({ kind: z.literal('selection') }).strict(),
  z.object({ kind: z.literal('scroll'), ...bounds, dy: z.number().min(-2000).max(2000) }).strict(),
  z.object({ kind: z.literal('text'), text: z.string().max(8192) }).strict(),
  z.object({ kind: z.literal('composition'), text: z.string().max(1024) }).strict(),
  z.object({ kind: z.literal('key'), key: z.string().max(32), shift: z.boolean().optional() }).strict(),
  z.object({ kind: z.literal('focusNode'), node: z.number().int().positive().max(2_147_483_647) }).strict(),
]);
const input = z.discriminatedUnion('action', [
  z.object({ action: z.literal('mount'), ...resource, dimensions: z.object({
    width: z.number().int().min(176).max(1440), height: z.number().int().min(160).max(900),
  }).strict() }).strict(),
  z.object({ action: z.enum(['refresh', 'close']), ...resource, session }).strict(),
  z.object({ action: z.literal('input'), ...resource, session, event }).strict(),
  z.object({ action: z.enum(['approve', 'deny']), ...resource, session, id }).strict(),
]);
const hash = value => createHash('sha256').update(JSON.stringify(value)).digest('hex');

function fail(status, code) {
  throw new LingxiDataError(status, code);
}

export function createDatasetRender({ origin, secret, pages, db, fetchRequest = fetch }) {
  if (!origin || !pages) return undefined;
  const endpoint = new URL(origin);
  if (endpoint.protocol !== 'https:' || endpoint.href !== endpoint.origin + '/' || !/^[A-Za-z0-9_-]{43}$/u.test(secret ?? '')) {
    throw new Error('Expected the private renderer origin and a scoped service credential');
  }
  async function render(path, body) {
    const response = await fetchRequest(new URL(path, endpoint), { method: 'POST', redirect: 'error', cache: 'no-store',
      signal: AbortSignal.timeout(15_000), headers: { 'Content-Type': 'application/json', Authorization: 'Bearer ' + secret },
      body: JSON.stringify(body) });
    const result = await response.json();
    if (response.status === 404) fail(410, 'RENDER_GONE');
    if (response.status === 429) fail(429, 'RENDER_QUOTA');
    if (response.status === 409) fail(409, 'RENDER_BUSY');
    if (!response.ok || result.error) fail(503, 'RENDER_UNAVAILABLE');
    return result;
  }

  async function fold(frame, target, view, request, user, token) {
    const selectedText = request.action === 'input' && request.event.kind === 'selection' ? frame.selectedText : undefined;
    const pending = db.collection(`datasetRenderApprovals/${hash([target.owner, target.scope, target.session])}/pending`);
    const queue = [...(frame.actions ?? [])];
    for (let step = 0; queue.length && step < 32; step++) {
      const action = queue.shift();
      if (!action || !id.safeParse(action.id).success || Buffer.byteLength(JSON.stringify(action)) > 32_768) continue;
      if (action.kind === 'state') {
        const outcome = await pages.execute(user, token, { action: 'saveState', page: request.page, version: request.version,
          expected: action.expected, state: action.state }).then(result => ({ result }), error => ({
          error: error.status === 409 ? 'PAGE_STATE_CHANGED' : 'PAGE_STATE_REJECTED',
        }));
        frame = await render('/frame', { ...target, command: { action: 'result', id: action.id, ...outcome } });
        queue.push(...(frame.actions ?? []));
        continue;
      }
      const policy = view.actions[action.alias];
      if (!Object.hasOwn(view.actions, action.alias)) continue;
      const intent = { action: 'invoke', page: request.page, version: request.version, alias: action.alias,
        ...(action.operation ? { operation: action.operation } : {}), parameters: action.parameters ?? {} };
      if (policy.mode === 'putRecord' || policy.mode === 'startTask') {
        if (policy.mode === 'startTask' && (Object.keys(intent.parameters).length || !id.safeParse(intent.operation).success)) continue;
        const ref = pending.doc(action.id);
        await db.runTransaction(async tx => {
          const old = (await tx.get(ref)).data();
          if (!old) tx.create(ref, { intent: JSON.stringify(intent), expiresAt: new Date(Date.now() + 5 * 60_000) });
        });
        continue;
      }
      const outcome = await pages.execute(user, token, intent).then(result => ({ result }), () => ({ error: 'DATA_ACCESS_REQUIRED' }));
      frame = await render('/frame', { ...target, command: { action: 'result', id: action.id, ...outcome } });
      queue.push(...(frame.actions ?? []));
    }
    const approvals = (await pending.limit(32).get()).docs.flatMap(doc => {
      const row = doc.data();
      if (row.decision || (row.expiresAt?.toMillis?.() ?? 0) < Date.now()) return [];
      const intent = JSON.parse(row.intent), policy = view.actions[intent.alias];
      return [policy.mode === 'startTask' ?
        { id: doc.id, mode: 'startTask', alias: intent.alias, bot: policy.bot, prompt: policy.prompt } :
        { id: doc.id, mode: 'putRecord', alias: intent.alias, record: policy.record, parameters: intent.parameters }];
    });
    if (typeof frame.png !== 'string' || frame.png.length > 4 * 1024 * 1024 || !Array.isArray(frame.accessibility)) {
      fail(503, 'RENDER_UNAVAILABLE');
    }
    return { session: target.session, png: frame.png, accessibility: frame.accessibility, approvals,
      ...(typeof selectedText === 'string' && selectedText.length <= 8192 ? { selectedText } : {}) };
  }

  return async (user, token, raw) => {
    const request = input.parse(raw);
    const target = { owner: hash(user.uid), scope: hash([request.page, request.version]), session: request.session };
    if (request.action === 'close') return render('/close', target);
    const mount = request.action === 'mount';
    async function verify() {
      try {
        return await pages.verifyRenderer(user, token, request.page, request.version);
      } catch (failure) {
        await render('/close', target).catch(() => undefined);
        throw failure;
      }
    }
    const view = mount ? await pages.openForRenderer(user, token, request.page, request.version) : await verify();
    target.tenant = hash(view.tenant);
    let frame;
    if (mount) {
      const result = await render('/open', { ...target, bundle: view.bundle, state: view.state === null ? null : JSON.parse(view.state),
        stateVersion: view.stateVersion, dimensions: request.dimensions });
      target.session = session.parse(result.session);
      frame = result;
    } else if (request.action === 'approve' || request.action === 'deny') {
      const pending = db.doc(`datasetRenderApprovals/${hash([target.owner, target.scope, target.session])}/pending/${request.id}`);
      const decision = request.action;
      const approval = await db.runTransaction(async tx => {
        const row = (await tx.get(pending)).data();
        if (!row || (row.expiresAt?.toMillis?.() ?? 0) < Date.now() || (row.decision && row.decision !== decision)) {
          fail(409, 'RENDER_APPROVAL_CHANGED');
        }
        if (!row.decision) tx.update(pending, { decision });
        return row;
      });
      const outcome = decision === 'deny' ? { error: 'DATA_ACTION_DENIED' } :
        await pages.executeApprovedAction(user, token, JSON.parse(approval.intent)).then(result => ({ result }), failure => {
          if (![400, 403, 404, 409].includes(failure.status)) throw failure;
          return { error: 'DATA_ACTION_REJECTED' };
        });
      frame = await render('/frame', { ...target, command: { action: 'result', id: request.id, ...outcome } });
    } else {
      frame = await render('/frame', { ...target, command: request.action === 'input' ?
        { action: 'input', event: request.event } : { action: 'frame' } });
    }
    const result = await fold(frame, target, view, request, user, token);
    await verify();
    return result;
  };
}
