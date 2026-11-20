import { createHash } from 'node:crypto';
import { z } from 'zod';
import { createReportGateway } from './lingxi-data-reports.mjs';

export const LINGXI_DATA_BASELINE = '99db9958ab5a59252b977e550bf57e610fa24770';

const id = z.string().regex(/^[A-Za-z0-9_-]{1,64}$/u);
const positive = z.number().int().min(1).max(Number.MAX_SAFE_INTEGER);
const cursor = z.number().int().min(0).max(Number.MAX_SAFE_INTEGER);
const teamScope = z.object({ kind: z.literal('team'), teamId: id, conversationId: id, generation: positive }).strict();
const scope = z.discriminatedUnion('kind', [z.object({ kind: z.literal('personal') }).strict(), teamScope]);

export const dataReference = z.object({
  dataset: id, record: id, revision: positive, schemaVersion: positive,
}).strict();

const command = z.discriminatedUnion('action', [
  z.object({ action: z.literal('describe'), dataset: id }).strict(),
  z.object({ action: z.literal('get'), reference: dataReference }).strict(),
  z.object({ action: z.literal('latest'), dataset: id, record: id }).strict(),
  z.object({ action: z.literal('list'), dataset: id, after: id.optional(), limit: z.number().int().min(1).max(50).optional() }).strict(),
  z.object({ action: z.literal('events'), dataset: id, after: cursor, waitMs: z.number().int().min(0).max(10_000).optional() }).strict(),
  z.object({ action: z.literal('put'), dataset: id, record: id, expectedRevision: cursor,
    schemaVersion: positive, value: z.record(z.string(), z.json()), archive: z.boolean().optional() }).strict(),
]);
const dataRequest = z.object({ scope, operation: id.optional(), command }).strict();
const viewRequest = z.object({ scope, operation: id,
  command: z.object({ action: z.literal('setView'), dataset: id, view: id, expectedEpoch: positive,
    definition: z.object({ kind: z.enum(['table', 'chart', 'form']), fields: z.array(id).min(1).max(64) }).strict().nullable(),
  }).strict(),
}).strict();
const personalScope = z.object({ kind: z.literal('personal') }).strict();
const jsonObject = z.record(z.string(), z.json());
const personalCreate = z.object({
  scope: personalScope, operation: id,
  command: z.object({ action: z.literal('create'), schema: jsonObject }).strict(),
}).strict();
const personalSchema = z.object({
  scope: personalScope, operation: id,
  command: z.object({ action: z.literal('schema'), dataset: id, expectedEpoch: positive, schema: jsonObject,
    migration: z.object({ rename: z.record(id, id).optional(), defaults: jsonObject.optional(), drop: z.array(id).optional() }).strict().optional(),
  }).strict(),
}).strict();
const teamEditors = z.array(id).max(499);
const teamAudiencePreview = z.object({
  scope: teamScope,
  command: z.object({ action: z.literal('previewAudience'), editors: teamEditors }).strict(),
}).strict();
const teamDataCreate = z.object({
  scope: teamScope, operation: id,
  command: z.object({ action: z.literal('createTeam'), schema: jsonObject, editors: teamEditors,
    audienceRevision: z.string().regex(/^[a-f0-9]{64}$/u) }).strict(),
}).strict();
const teamPersonalPublication = z.object({
  scope: teamScope, operation: id,
  command: z.object({ action: z.literal('publishPersonal'), editors: teamEditors,
    audienceRevision: z.string().regex(/^[a-f0-9]{64}$/u),
    dataset: id, schemaVersion: positive, sourceEpoch: positive, records: z.array(dataReference).max(20),
    templates: z.array(z.object({ view: id, version: positive }).strict()).max(20).optional(),
  }).strict().refine(input => new Set(input.records.map(record => record.record)).size === input.records.length
    && input.records.every(record => record.dataset === input.dataset && record.schemaVersion === input.schemaVersion)
    && (!input.templates || new Set(input.templates.map(entry => entry.view)).size === input.templates.length)),
}).strict();
const discovery = z.object({
  scope: teamScope, command: z.object({ action: z.literal('datasets') }).strict(),
}).strict();
const personalDiscovery = z.object({
  scope: personalScope, command: z.object({ action: z.literal('datasets') }).strict(),
}).strict();
const grant = z.object({ uid: z.string().min(1).max(128), generation: positive,
  access: z.enum(['read', 'edit', 'manage']) }).strict();
const teamBinding = z.object({
  kind: z.literal('team'), teamId: id, conversationId: id, audience: z.array(grant).max(500),
}).strict();
const personalBinding = z.object({ kind: z.literal('personal'), ownerUid: z.string().min(1).max(128) }).strict();

export class LingxiDataError extends Error {
  constructor(status, code) {
    super(code);
    this.status = status;
    this.code = code;
  }
}

function deny() {
  throw new LingxiDataError(403, 'DATA_ACCESS_REQUIRED');
}

function datasetId(input) {
  return input.action === 'get' ? input.reference.dataset : input.dataset;
}

// Pinned List uses an exclusive cursor and ASCII identifiers ordered by bytes.
export function beforeRecordId(record) {
  return record.slice(0, -1) + String.fromCharCode(record.charCodeAt(record.length - 1) - 1) + '\uffff';
}

// The binding is server-maintained. A client-provided Team ID, conversation ID or dataset ID alone conveys no permission.
export function authorizeDataRequest(db, uid, requestScope, dataset, write) {
  return db.runTransaction(tx => authorizeDataBinding(tx, db, uid, requestScope, dataset, write));
}

export async function authorizeDataBinding(tx, db, uid, requestScope, dataset, write, manage = false) {
  const binding = (await tx.get(db.doc(`lingxiDataBindings/${dataset}`))).data();
  if (requestScope.kind === 'personal') {
    const result = personalBinding.safeParse(binding);
    if (!result.success || result.data.ownerUid !== uid) deny();
    return;
  }
  const result = teamBinding.safeParse(binding);
  if (!result.success || result.data.teamId !== requestScope.teamId
    || result.data.conversationId !== requestScope.conversationId) deny();
  const canEdit = await teamConversationAccess(tx, db, uid, requestScope);
  const entry = result.data.audience.find(value => value.uid === uid && value.generation === requestScope.generation);
  if (!entry || (write && (entry.access === 'read' || !canEdit)) || (manage && entry.access !== 'manage')) deny();
}

async function teamConversationAccess(tx, db, uid, requestScope) {
  const ref = db.doc(`teams/${requestScope.teamId}`);
  const [team, member, thread] = await tx.getAll(ref, ref.collection('members').doc(uid),
    ref.collection('conversations').doc(requestScope.conversationId));
  const current = member.data();
  const conversation = thread.data();
  if (team.data()?.state !== 'active' || !current || !conversation?.memberUids?.includes(uid)) deny();
  if (current.generation !== requestScope.generation) throw new LingxiDataError(409, 'STALE_MEMBERSHIP');
  return conversation.participants?.includes(`user:${uid}`) === true;
}

function visibleDatasets(db, uid, requestScope) {
  return db.runTransaction(async tx => {
    const canEdit = await teamConversationAccess(tx, db, uid, requestScope);
    const results = await tx.get(db.collection('lingxiDataBindings').where('conversationId', '==', requestScope.conversationId));
    const datasets = [];
    for (const row of results.docs) {
      const binding = teamBinding.safeParse(row.data());
      if (!binding.success || binding.data.teamId !== requestScope.teamId || !id.safeParse(row.id).success) continue;
      const entry = binding.data.audience.find(value => value.uid === uid && value.generation === requestScope.generation);
      if (entry) datasets.push({ dataset: row.id, access: canEdit ? entry.access : 'read' });
    }
    return { datasets };
  });
}

function digest(value) {
  return createHash('sha256').update(JSON.stringify(value)).digest('hex');
}

async function readTeamAudience(tx, db, uid, scope, editors) {
  const team = db.doc(`teams/${scope.teamId}`);
  const member = id => team.collection('members').doc(id);
  const [teamDoc, selfDoc, threadDoc] = await tx.getAll(team, member(uid), team.collection('conversations').doc(scope.conversationId));
  const self = selfDoc.data(), conversation = threadDoc.data();
  if (teamDoc.data()?.state !== 'active' || !['owner', 'admin'].includes(self?.role)
    || !conversation?.participants?.includes('user:' + uid) || !conversation.memberUids?.includes(uid)) deny();
  if (self.generation !== scope.generation) throw new LingxiDataError(409, 'STALE_MEMBERSHIP');
  if (conversation.memberUids.length > 500 || new Set(editors).size !== editors.length
    || editors.some(editor => !conversation.participants.includes('user:' + editor) || !conversation.memberUids.includes(editor))) {
    throw new LingxiDataError(400, 'INVALID_DATA_REQUEST');
  }
  const readers = [...new Set(conversation.memberUids)].filter(reader => reader !== uid).sort();
  if (readers.some(reader => !id.safeParse(reader).success)) deny();
  const members = readers.length ? await tx.getAll(...readers.map(member)) : [];
  if (members.some(doc => editors.includes(doc.id) && !doc.exists)) deny();
  const audience = [{ uid, generation: self.generation, access: 'manage' }];
  for (const row of members) {
    if (!row.exists) continue;
    const generation = positive.safeParse(row.data().generation);
    if (!generation.success) deny();
    audience.push({ uid: row.id, generation: generation.data, access: editors.includes(row.id) ? 'edit' : 'read' });
  }
  return { audience, audienceRevision: digest([scope.teamId, scope.conversationId, audience]) };
}

export function createPinnedDataClient(endpoint, fetchRequest = fetch) {
  const upstream = new URL(endpoint);
  if (upstream.protocol !== 'https:' || upstream.username || upstream.password
    || upstream.pathname !== '/' || upstream.search || upstream.hash) throw new Error('Expected an HTTPS Data service origin');
  const url = new URL('/v1/data', upstream);
  return async function forward(token, operation, input, taskToken) {
    let result;
    try {
      result = await fetchRequest(url, {
        method: 'POST', redirect: 'error',
        headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json',
          ...(taskToken ? { 'X-Lingxi-Task-Token': taskToken } : {}) },
        body: JSON.stringify({ operation: operation ?? null, command: input }),
        signal: AbortSignal.timeout(18_000),
      });
    } catch {
      throw new LingxiDataError(503, 'DATA_UNAVAILABLE');
    }
    if (!result.ok) {
      const errors = { 400: 'INVALID_DATA_REQUEST', 401: 'DATA_ACCESS_REQUIRED', 403: 'DATA_ACCESS_REQUIRED',
        404: 'DATA_NOT_FOUND', 409: 'DATA_CONFLICT', 507: 'DATA_CAPACITY' };
      const code = errors[result.status] ?? 'DATA_UNAVAILABLE';
      throw new LingxiDataError(code === 'DATA_UNAVAILABLE' ? 503 : result.status, code);
    }
    try { return await result.json(); }
    catch { throw new LingxiDataError(503, 'DATA_UNAVAILABLE'); }
  };
}

export function createLingxiDataGateway({ db, endpoint, projectActor, fetchRequest = fetch }) {
  if (!endpoint) return null;
  const forward = createPinnedDataClient(endpoint, fetchRequest);
  const reports = createReportGateway({ db, forward, projectActor, readTeamAudience });

  return async function forwardLingxiData(uid, token, raw) {
    if (['reportContext', 'prepareReport', 'publishReport', 'limitReportReader', 'getReport', 'openReportReview'].includes(raw?.command?.action)) {
      return reports(uid, token, raw);
    }
    if (personalDiscovery.safeParse(raw).success) {
      const result = await db.collection('lingxiDataBindings').where('ownerUid', '==', uid).limit(100).get();
      return { datasets: result.docs.filter(row => personalBinding.safeParse(row.data()).success)
        .map(row => ({ dataset: row.id, access: 'manage' })) };
    }
    const discover = discovery.safeParse(raw);
    if (discover.success) return visibleDatasets(db, uid, discover.data.scope);
    const preview = teamAudiencePreview.safeParse(raw);
    if (preview.success) return db.runTransaction(tx => readTeamAudience(tx, db, uid, preview.data.scope, preview.data.command.editors));
    const publication = teamPersonalPublication.safeParse(raw);
    const teamCreate = publication.success ? publication : teamDataCreate.safeParse(raw);
    if (teamCreate.success) {
      if (!projectActor) throw new LingxiDataError(503, 'DATA_UNAVAILABLE');
      const { scope: team, operation, command: input } = teamCreate.data;
      const approvedAudience = async tx => {
        const current = await readTeamAudience(tx, db, uid, team, input.editors);
        if (current.audienceRevision !== input.audienceRevision) throw new LingxiDataError(409, 'DATA_CONFLICT');
        return current.audience;
      };
      const authorize = () => db.runTransaction(approvedAudience);
      const dataset = 'team-' + digest([uid, team, operation]).slice(0, 40);
      const intent = db.collection('lingxiDataPublications').doc(dataset);
      const templates = datasetId => db.collection('lingxiDataTemplateViews').doc(datasetId).collection('views');
      const readTemplates = async tx => {
        if (input.action !== 'publishPersonal' || !input.templates?.length) return [];
        await authorizeDataBinding(tx, db, uid, { kind: 'personal' }, input.dataset, false);
        return Promise.all(input.templates.map(async ({ view, version }) => {
          const entry = (await tx.get(templates(input.dataset).doc(view))).data();
          if (!entry?.definition || entry.version !== version || entry.schemaVersion !== input.schemaVersion) {
            throw new LingxiDataError(409, 'DATA_CONFLICT');
          }
          return { view, definition: entry.definition };
        }));
      };
      const requestDigest = digest([uid, team, input]);
      const { audience, previous } = await db.runTransaction(async tx => {
        const audience = await approvedAudience(tx);
        const existing = await tx.get(intent);
        if (existing.exists && existing.data().requestDigest !== requestDigest) throw new LingxiDataError(409, 'DATA_CONFLICT');
        if (!existing.data()?.result) await readTemplates(tx);
        if (!existing.exists) tx.create(intent, { requestDigest });
        return { audience, previous: existing.data()?.result };
      });
      if (previous) return previous;
      let schema, records = [], views = {};
      if (input.action === 'publishPersonal') {
        const personal = { kind: 'personal' };
        const authorizeSource = () => authorizeDataRequest(db, uid, personal, input.dataset, false);
        const snapshot = await projectActor(uid, personal, authorizeSource, async () => {
          const source = await forward(token, null, { action: 'describe', dataset: input.dataset });
          if (source.schemaVersion !== input.schemaVersion || source.epoch !== input.sourceEpoch) throw new LingxiDataError(409, 'DATA_CONFLICT');
          const records = [];
          for (const reference of input.records) {
            const revision = await forward(token, null, { action: 'get', reference });
            if (revision.record?.archived || !jsonObject.safeParse(revision.record?.value).success) deny();
            records.push({ reference, value: revision.record.value });
          }
          return { schema: source.schema, records, views: source.views };
        });
        schema = snapshot.schema;
        records = snapshot.records;
        views = snapshot.views ?? {};
      } else {
        schema = input.schema;
      }
      const created = await projectActor(uid, team, authorize, () => forward(token, operation, {
        action: 'create', dataset, team: team.teamId, schema,
      }));
      if (created?.dataset !== dataset) throw new LingxiDataError(503, 'DATA_UNAVAILABLE');

      // Each recipient receives an idempotent grant on the pinned service. The
      // product binding stays invisible until all grants and the audience match.
      let epoch = created.epoch;
      for (const entry of audience.slice(1)) {
        const share = await projectActor(uid, team, authorize, () => forward(token, digest([operation, entry]), {
          action: 'share', dataset, recipient: entry.uid, access: entry.access,
        }), [entry]);
        epoch = share.epoch;
      }
      const publishedRecords = [];
      for (const [view, definition] of Object.entries(views)) {
        const saved = await projectActor(uid, team, authorize, () => forward(token, digest([operation, 'view', view]), {
          action: 'set_view', dataset, view, definition, expectedEpoch: epoch,
        }));
        epoch = saved.epoch;
      }
      for (const { reference, value } of records) {
        const copied = await projectActor(uid, team, authorize, () => forward(token, digest([operation, reference]), {
          action: 'put', dataset, record: reference.record, expectedRevision: 0, schemaVersion: 1, value,
        }));
        if (!dataReference.safeParse(copied).success) throw new LingxiDataError(503, 'DATA_UNAVAILABLE');
        publishedRecords.push(copied);
        epoch += 1;
      }
      const result = { ...created, epoch, ...(input.action === 'publishPersonal' ? { records: publishedRecords } : {}) };
      return db.runTransaction(async tx => {
        await approvedAudience(tx);
        const ref = db.collection('lingxiDataBindings').doc(dataset), old = await tx.get(ref);
        const completed = (await tx.get(intent)).data()?.result;
        if (completed) return completed;
        const selectedTemplates = await readTemplates(tx);
        if (!old.exists) tx.create(ref, { kind: 'team', teamId: team.teamId, conversationId: team.conversationId, audience });
        else {
          const existing = teamBinding.safeParse(old.data());
          if (!existing.success || existing.data.teamId !== team.teamId || existing.data.conversationId !== team.conversationId
            || digest(existing.data.audience) !== digest(audience)) deny();
        }
        for (const { view, definition } of selectedTemplates) {
          tx.create(templates(dataset).doc(view), { version: 1, schemaVersion: 1, definition });
        }
        tx.update(intent, { result });
        return result;
      });
    }
    const created = personalCreate.safeParse(raw);
    if (created.success) {
      const { operation, command: input } = created.data;
      const hash = createHash('sha256').update(JSON.stringify([uid, operation])).digest('hex');
      const dataset = 'personal-' + hash.slice(0, 40);
      const create = () => forward(token, operation, { ...input, dataset });
      const response = projectActor ? await projectActor(uid, created.data.scope, async () => {}, create) : await create();
      if (response?.dataset !== dataset) throw new LingxiDataError(503, 'DATA_UNAVAILABLE');
      const ref = db.collection('lingxiDataBindings').doc(dataset);
      await db.runTransaction(async tx => {
        const existing = await tx.get(ref);
        if (!existing.exists) tx.create(ref, { kind: 'personal', ownerUid: uid });
        else if (existing.data()?.kind !== 'personal' || existing.data()?.ownerUid !== uid) deny();
      });
      return response;
    }
    const schemaEdit = personalSchema.safeParse(raw);
    const viewEdit = viewRequest.safeParse(raw);
    let parsed = schemaEdit;
    if (!parsed.success) parsed = viewEdit;
    if (!parsed.success) parsed = dataRequest.safeParse(raw);
    if (!parsed.success) throw new LingxiDataError(400, 'INVALID_DATA_REQUEST');
    const { scope: requestScope, operation, command: input } = parsed.data;
    const dataset = datasetId(input);
    const manage = input.action === 'setView';
    const write = input.action === 'put' || input.action === 'schema' || manage;
    if (write && operation === undefined) throw new LingxiDataError(400, 'OPERATION_ID_REQUIRED');
    const authorize = () => db.runTransaction(tx => authorizeDataBinding(tx, db, uid, requestScope, dataset, write, manage), { readOnly: true });
    async function run() {
      if (input.action !== 'latest') return forward(token, operation, manage ? { ...input, action: 'set_view' } : input);
      const result = await forward(token, null, { action: 'list', dataset, after: beforeRecordId(input.record), limit: 1 });
      const item = result.records?.[0];
      if (item?.id !== input.record) throw new LingxiDataError(404, 'DATA_NOT_FOUND');
      return dataReference.parse({ dataset, record: input.record, revision: item.record.revision, schemaVersion: item.record.schemaVersion });
    }
    let response;
    if (projectActor) response = await projectActor(uid, requestScope, authorize, run);
    else {
      await authorize();
      response = await run();
    }
    // Long polls and delayed requests may span removal or a changed publication audience.
    await authorize();
    return response;
  };
}
