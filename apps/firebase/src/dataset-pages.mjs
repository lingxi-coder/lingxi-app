import { createHash } from 'node:crypto';
import { z } from 'zod';
import { DATA_PROTOCOL_VERSION, isResourceRef } from './dataset-resource.mjs';
import { LingxiDataError } from './lingxi-data.mjs';
import { SharedBotsStore } from './shared-bots.mjs';
import { TeamMessagesStore } from './team-messages.mjs';

const id = z.string().regex(/^[A-Za-z0-9_-]{1,64}$/u);
const digest = z.string().regex(/^[a-f0-9]{64}$/u);
const generation = z.number().int().positive();
const title = z.string().trim().min(1).max(120);
const maxBytes = 2 * 1024 * 1024;
const teamPageBudget = 512 * 1024 * 1024;
const destination = z.object({ team: id, conversation: id }).strict();
const declaredAction = z.discriminatedUnion('mode', [
  z.object({ mode: z.literal('readFixed'), record: id.optional(), fields: z.array(id).min(1).max(32).optional() }).strict(),
  z.object({ mode: z.literal('readLatest'), record: id.optional(), fields: z.array(id).min(1).max(32).optional() }).strict(),
  z.object({ mode: z.literal('putRecord'), record: id, fields: z.array(id).min(1).max(32) }).strict(),
  z.object({ mode: z.literal('startTask'), bot: id,
    prompt: z.string().trim().min(1).refine(text => Buffer.byteLength(text) <= 4096) }).strict(),
]);
const packageInput = z.object({
  title,
  html: z.string().min(1).max(maxBytes),
  assets: z.record(z.string().regex(/^[A-Za-z0-9_-][A-Za-z0-9_.-]{0,63}$/u), z.object({
    mime: z.enum(['image/png', 'image/jpeg', 'image/webp', 'text/css', 'text/javascript']),
    data: z.string().regex(/^[A-Za-z0-9+/]*={0,2}$/u).max(maxBytes),
  }).strict()),
  actions: z.record(id, declaredAction),
}).strict().refine(bundle => Object.keys(bundle.assets).length <= 32 && Object.keys(bundle.actions).length <= 16 &&
  Buffer.byteLength(JSON.stringify(bundle)) <= maxBytes);
const scope = { page: id, version: digest };
const input = z.discriminatedUnion('action', [
  z.object({ action: z.literal('prepare'), destination, dataset: id, via: id }).strict(),
  z.object({ action: z.literal('publish'), page: id, operation: id, destination, generation, audienceDigest: id, dataset: id, via: id,
    bundle: packageInput }).strict(),
  z.object({ action: z.literal('describe'), ...scope }).strict(),
  z.object({ action: z.literal('invoke'), ...scope, alias: id, operation: id.optional(), parameters: z.record(z.string(), z.unknown()).default({}) }).strict(),
  z.object({ action: z.literal('saveState'), ...scope, expected: z.number().int().nonnegative(),
    state: z.string().max(16_384).refine(text => Buffer.byteLength(text) <= 16_384) }).strict(),
  z.object({ action: z.literal('revoke'), ...scope }).strict(),
]);

function hash(value, encoding = 'hex') {
  return createHash('sha256').update(value).digest(encoding);
}

function fail(status, code) {
  throw new LingxiDataError(status, code);
}

function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (!value || typeof value !== 'object') return value;
  return Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical(value[key])]));
}

function storedPage(db, page, version) {
  return db.doc(`datasetPages/${page}/versions/${version}`);
}

function coreRequest(row, command, operation) {
  return { protocol: DATA_PROTOCOL_VERSION, dataset: row.dataset, via: row.via, command, ...(operation ? { operation } : {}) };
}

function requireShare(share, target) {
  if (share?.status !== 'active' || share.destination?.team !== target.team || share.destination?.conversation !== target.conversation ||
    !isResourceRef(share.reference) || share.reference.kind === 'pageVersion' ||
    !share.audience || typeof share.message !== 'string') fail(403, 'DATA_ACCESS_REQUIRED');
  return share;
}

async function liveShare(tx, db, dataset, via, target, uid) {
  const team = db.doc(`teams/${target.team}`);
  const thread = team.collection('conversations').doc(target.conversation);
  const [t, c, g, m] = await tx.getAll(team, thread, db.doc(`datasetV2/${dataset}/grants/${via}`), team.collection('members').doc(uid));
  const share = requireShare(g.data(), target);
  const current = m.data();
  const people = c.data()?.memberUids ?? [];
  if (t.data()?.state !== 'active' || !people.includes(uid) || !current || share.audience[uid] !== current.generation) {
    fail(403, 'DATA_ACCESS_REQUIRED');
  }
  const source = (await tx.get(thread.collection('messages').doc(share.message))).data();
  if (!source || source.recalled || (source.resourceGrant ?? source.datasetGrantId) !== via) fail(403, 'DATA_ACCESS_REQUIRED');
  return { share, people, thread, generation: current.generation };
}

export function createDatasetPages({ db, bucket, datasets }) {
  if (!bucket || !datasets) return undefined;
  function file(row) {
    return bucket.file(`dataset-pages/${row.ownerKey}/${row.page}/${row.version}.json`);
  }
  async function load(row) {
    const [bytes] = await file(row).download({ validation: 'crc32c' });
    if (bytes.length > maxBytes || hash(bytes) !== row.version) fail(503, 'DATA_UNAVAILABLE');
    return packageInput.parse(JSON.parse(bytes));
  }
  async function authority(tx, ref, user) {
    const row = (await tx.get(ref)).data();
    if (row?.status !== 'active') fail(403, 'DATA_ACCESS_REQUIRED');
    const facts = await liveShare(tx, db, row.dataset, row.via, row.destination, user.uid);
    if (row.audience[user.uid] !== facts.generation) fail(403, 'DATA_ACCESS_REQUIRED');
    const message = (await tx.get(facts.thread.collection('messages').doc(row.message))).data();
    if (!message || message.recalled || message.resourceGrant !== row.grant ||
        message.resourceRef?.page !== row.page || message.resourceRef?.version !== row.version) fail(403, 'DATA_ACCESS_REQUIRED');
    return { row, generation: facts.generation };
  }

  async function describe(ref, user, token, includeCode = false) {
    const view = await db.runTransaction(async tx => {
      const { row } = await authority(tx, ref, user);
      const saved = (await tx.get(ref.collection('readerStates').doc(hash(user.uid)))).data();
      return { row, state: saved?.state ?? null, stateVersion: saved?.version ?? 0 };
    });
    await datasets(db, user, token, coreRequest(view.row, { action: 'describe' }));
    const bundle = includeCode ? await load(view.row) : null;
    await db.runTransaction(tx => authority(tx, ref, user));
    const { row, state, stateVersion } = view;
    return { tenant: row.destination.team, actions: row.actions, reference: { kind: 'pageVersion', page: row.page, version: row.version }, title: row.title, state, stateVersion,
      ...(bundle ? { bundle } : {}) };
  }

  async function publish(request, user, token) {
    const { bundle, dataset, via, page, destination, generation, operation } = request;
    await datasets(db, user, token, coreRequest(request, { action: 'describe' }));
    const encoded = JSON.stringify(canonical(bundle));
    const version = hash(encoded), ownerKey = hash(user.uid);
    const grant = hash(JSON.stringify([ownerKey, page, operation]), 'base64url');
    const message = 'page_' + grant;
    const ref = storedPage(db, page, version), reference = { kind: 'pageVersion', page, version };
    const row = await db.runTransaction(async tx => {
      const { share, people, generation: current } = await liveShare(tx, db, dataset, via, destination, user.uid);
      if (current !== generation || share.issuer !== user.uid || people.length !== Object.keys(share.audience).length ||
          !people.every(uid => Object.hasOwn(share.audience, uid)) || hash(JSON.stringify(canonical(share.audience)), 'base64url') !== request.audienceDigest) {
        fail(409, 'DATA_SHARE_CHANGED');
      }
      if (Object.values(bundle.actions).some(action => (action.mode === 'readLatest' && share.mode === 'pinned') ||
          (action.mode === 'putRecord' && share.mode !== 'edit'))) fail(403, 'DATA_ACCESS_REQUIRED');
      const existing = (await tx.get(ref)).data();
      const owner = db.doc(`datasetPages/${page}`);
      const existingOwner = (await tx.get(owner)).data();
      if (existingOwner && existingOwner.uid !== user.uid) fail(409, 'DATA_CONFLICT');
      if (existing) {
        if (existing.grant !== grant || existing.via !== via || existing.status === 'revoked') fail(409, 'DATA_CONFLICT');
        return existing;
      }
      const quota = db.doc(`datasetPageQuotas/${destination.team}`);
      const usage = (await tx.get(quota)).data()?.bytes ?? 0;
      const bytes = Buffer.byteLength(encoded);
      if (!Number.isSafeInteger(usage) || usage < 0 || usage + bytes > teamPageBudget) fail(429, 'DATA_QUOTA');
      const next = { bytes, ownerKey, uid: user.uid, status: 'pending', page, version, title: bundle.title, audience: share.audience,
        dataset, via, destination, grant, message, dataReference: share.reference, actions: bundle.actions };
      if (!existingOwner) tx.create(owner, { uid: user.uid });
      tx.create(ref, next);
      tx.set(quota, { bytes: usage + bytes });
      return next;
    });
    try { await file(row).save(encoded, { resumable: false, preconditionOpts: { ifGenerationMatch: 0 },
      metadata: { contentType: 'application/json', cacheControl: 'no-store' } }); }
    catch (error) { if (Number(error.code) !== 412) throw error; await load(row); }
    await new TeamMessagesStore(db).execute(user, { action: 'mutate', operation: 'create',
      teamId: destination.team, conversationId: destination.conversation, generation, changeId: message, messageId: message, text: bundle.title },
    { reference, grant });
    await db.runTransaction(async tx => {
      const latest = (await tx.get(ref)).data();
      if (latest?.grant !== grant || latest.status === 'revoked') fail(409, 'DATA_SHARE_CHANGED');
      if (latest.status !== 'active') tx.update(ref, { status: 'active' });
    });
    return { reference, messageId: message };
  }

  async function invoke(ref, { row, generation: memberGeneration }, request, user, token, approved) {
    const action = row.actions[request.alias];
    if (!Object.hasOwn(row.actions, request.alias) || !action) fail(403, 'DATA_ACCESS_REQUIRED');
    const data = row.dataReference, params = request.parameters;
    let command, operation;
    switch (action.mode) {
      case 'readFixed':
        if (data.kind === 'record') command = { action: 'read', reference: data };
        if (data.kind === 'snapshot') command = { action: 'openSnapshot', snapshot: data.snapshot, block: z.number().int().nonnegative().parse(params.block ?? 0) };
        if (data.kind === 'reportRelease') command = { action: 'openReport', release: data.release,
          after: z.string().min(1).max(160).optional().parse(params.after),
          limit: z.number().int().min(1).max(50).optional().parse(params.limit) };
        break;
      case 'readLatest':
        command = { action: 'latest', record: id.parse(action.record ?? params.record) };
        break;
      case 'putRecord': {
        if (!approved) fail(403, 'DATA_APPROVAL_REQUIRED');
        const value = z.record(id, z.unknown()).parse(params.value);
        if (Object.keys(value).some(key => !action.fields.includes(key))) fail(403, 'DATA_ACCESS_REQUIRED');
        operation = hash(JSON.stringify([row.page, row.version, request.alias, id.parse(request.operation)]), 'base64url');
        command = { action: 'put', record: action.record, value, partition: z.string().min(1).max(160).parse(params.partition),
          schemaVersion: generation.parse(params.schemaVersion), expectedRevision: z.number().int().nonnegative().parse(params.expectedRevision) };
        break;
      }
      case 'startTask': {
        if (!approved) fail(403, 'DATA_APPROVAL_REQUIRED');
        z.object({}).strict().parse(params);
        await datasets(db, user, token, coreRequest(row, { action: 'describe' }));
        const messageId = 'page_task_' + hash(JSON.stringify([row.page, row.version, request.alias, user.uid, id.parse(request.operation)]), 'base64url');
        return new SharedBotsStore(db).execute(user, {
          schemaVersion: 1, teamId: row.destination.team, generation: memberGeneration, commandId: messageId, action: 'submit',
          payload: { conversationId: row.destination.conversation, messageId, changeId: messageId,
            replyTo: row.message, text: action.prompt, targets: [action.bot] },
        }, undefined, tx => authority(tx, ref, user));
      }
    }
    if (!command) fail(400, 'INVALID_DATA_REQUEST');
    return datasets(db, user, token, coreRequest(row, command, operation));
  }

  async function execute(user, token, raw) {
    const request = input.parse(raw);
    if (request.action === 'prepare') {
      const { share, people } = await db.runTransaction(tx => liveShare(tx, db, request.dataset, request.via, request.destination, user.uid));
      if (share.issuer !== user.uid || people.length !== Object.keys(share.audience).length ||
          !people.every(uid => Object.hasOwn(share.audience, uid))) fail(403, 'DATA_ACCESS_REQUIRED');
      return { audience: share.audience, audienceDigest: hash(JSON.stringify(canonical(share.audience)), 'base64url'), reference: share.reference };
    }
    if (request.action === 'publish') return publish(request, user, token);
    const ref = storedPage(db, request.page, request.version);
    if (request.action === 'describe') {
      const { tenant, actions, ...view } = await describe(ref, user, token);
      return view;
    }
    if (request.action === 'revoke') {
      return db.runTransaction(async tx => {
        const row = (await tx.get(ref)).data();
        if (!row || row.uid !== user.uid) fail(403, 'DATA_ACCESS_REQUIRED');
        tx.update(ref, { status: 'revoked' });
        return { revoked: true };
      });
    }
    if (request.action === 'saveState') {
      const { row } = await db.runTransaction(tx => authority(tx, ref, user));
      await datasets(db, user, token, coreRequest(row, { action: 'describe' }));
      return db.runTransaction(async tx => {
        await authority(tx, ref, user);
        const state = ref.collection('readerStates').doc(hash(user.uid));
        const old = (await tx.get(state)).data();
        if ((old?.version ?? 0) !== request.expected) fail(409, 'DATA_CONFLICT');
        try { JSON.parse(request.state); } catch { fail(400, 'INVALID_DATA_REQUEST'); }
        const version = request.expected + 1;
        tx.set(state, { state: request.state, version });
        return { version };
      });
    }
    return runAction(ref, request, user, token, false);
  }

  async function runAction(ref, request, user, token, approved) {
    const facts = await db.runTransaction(tx => authority(tx, ref, user));
    const response = await invoke(ref, facts, request, user, token, approved);
    await db.runTransaction(tx => authority(tx, ref, user));
    return response;
  }

  function executeApprovedAction(user, token, raw) {
    const request = input.parse(raw);
    if (request.action !== 'invoke') fail(400, 'INVALID_DATA_REQUEST');
    return runAction(storedPage(db, request.page, request.version), request, user, token, true);
  }

  // Only the renderer service obtains executable code; product clients receive metadata/state and isolated frames.
  function rendererView(user, token, page, version, mount) {
    return describe(storedPage(db, id.parse(page), digest.parse(version)), user, token, mount);
  }
  return { execute, executeApprovedAction, openForRenderer: (user, token, page, version) => rendererView(user, token, page, version, true),
    verifyRenderer: (user, token, page, version) => rendererView(user, token, page, version, false) };
}
