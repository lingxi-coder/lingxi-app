import { createHash, createPublicKey, ECDH, verify } from 'node:crypto';
import { z } from 'zod';
import { authorizeDataBinding, LingxiDataError } from './lingxi-data.mjs';

const id = z.string().regex(/^[A-Za-z0-9_-]{1,64}$/u);
const hash = z.string().regex(/^[a-f0-9]{64}$/u);
const version = z.number().int().positive().safe();
const label = z.string().min(1).max(160);
const team = z.object({ kind: z.literal('team'), teamId: id, conversationId: id, generation: version }).strict();
const metric = z.discriminatedUnion('kind', [
  z.object({ kind: z.literal('count') }).strict(),
  z.object({ kind: z.literal('sum'), field: id, scale: z.number().int().min(0).max(12) }).strict(),
]);
const spec = z.object({ title: label, partition: id, group: id, metric }).strict();
const unique = values => new Set(values).size === values.length;
const partitions = z.union([z.literal('*'), z.array(label).max(256).refine(unique)]);
const readers = z.array(z.object({ uid: id, partitions }).strict()).min(1).max(500)
  .refine(values => unique(values.map(entry => entry.uid)));
const origin = z.discriminatedUnion('kind', [
  z.object({ kind: z.literal('source'), source: id, version }).strict(),
  z.object({ kind: z.literal('personal'), dataset: id, epoch: version }).strict(),
  z.object({ kind: z.literal('team'), dataset: id, epoch: version }).strict(),
]);
const plan = z.object({ parent: hash.nullable(), audienceRevision: hash, readers, spec, origin,
  bots: z.array(id).max(16).refine(unique) }).strict();
const fact = z.object({ partition: label, group: label, units: z.string().regex(/^(?:0|-?[1-9][0-9]{0,38})$/u) }).strict();
const facts = z.array(fact).max(4096);
const capture = z.object({ payload: z.string().regex(/^[\w-]+$/u).max(2600000),
  signature: z.string().regex(/^[\w-]{86}$/u) }).strict();
const envelope = z.object({ protocol: z.literal('lingxi.report/1'), binding: hash, source: id,
  preset: z.literal('main'), owner: id, spec, facts, inputDigest: z.string().regex(/^[\w-]{43}$/u) }).strict();
export const readReport = z.object({ action: z.literal('getReport'), dataset: id, release: hash.nullable() }).strict();
export const reviewReport = z.object({ action: z.literal('openReportReview'), dataset: id, release: hash,
  partition: label, group: label }).strict();
const commands = z.discriminatedUnion('action', [
  z.object({ action: z.literal('reportContext'), dataset: id }).strict(),
  z.object({ action: z.literal('prepareReport'), dataset: id, plan }).strict(),
  z.object({ action: z.literal('publishReport'), dataset: id, plan, confirmation: hash, capture: capture.optional() }).strict(),
  z.object({ action: z.literal('limitReportReader'), dataset: id, uid: id, generation: version, partitions,
    expectedVersion: z.number().int().nonnegative().safe() }).strict(),
  readReport, reviewReport,
]);
const request = z.object({ scope: team, command: commands }).strict();

function fail(status, code) { throw new LingxiDataError(status, code); }
function invalid() { fail(400, 'INVALID_DATA_REQUEST'); }
function denied() { fail(403, 'DATA_ACCESS_REQUIRED'); }
function conflict() { fail(409, 'DATA_CONFLICT'); }
function missing() { fail(404, 'DATA_NOT_FOUND'); }
function digest(value) { return createHash('sha256').update(value).digest('hex'); }
function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === 'object') return Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical(value[key])]));
  return value;
}
function fingerprint(value) { return digest(JSON.stringify(canonical(value))); }
function series(db, dataset) { return db.collection('lingxiReportSeries').doc(dataset); }
function included(allow, partition) { return allow === '*' || allow.includes(partition); }

function attest(input, key, owner, binding, definition, source) {
  if (!input) invalid();
  const bytes = Buffer.from(input.payload, 'base64url');
  try {
    const point = ECDH.convertKey(Buffer.from(key, 'base64url'), 'prime256v1', undefined, undefined, 'uncompressed');
    const publicKey = createPublicKey({ format: 'jwk', key: { kty: 'EC', crv: 'P-256',
      x: point.subarray(1, 33).toString('base64url'), y: point.subarray(33).toString('base64url') } });
    if (!verify('sha256', bytes, { key: publicKey, dsaEncoding: 'ieee-p1363' }, Buffer.from(input.signature, 'base64url'))) denied();
  } catch { denied(); }
  let raw;
  try { raw = JSON.parse(bytes.toString('utf8')); } catch { invalid(); }
  const parsed = envelope.safeParse(raw);
  if (!parsed.success) invalid();
  const report = parsed.data;
  if (report.binding !== binding || report.owner !== owner || report.source !== source
    || fingerprint(report.spec) !== fingerprint(definition)) denied();
  return { facts: report.facts, inputDigest: report.inputDigest, confirmation: digest(bytes) };
}

function decimal(units, scale) {
  const digits = (units < 0n ? -units : units).toString().padStart(scale + 1, '0');
  const magnitude = scale ? digits.slice(0, -scale) + '.' + digits.slice(-scale) : digits;
  return units < 0n ? '-' + magnitude : magnitude;
}

// Used inside the caller's transaction, including the Bot's live task checks.
export async function deliverReport(tx, db, uid, scope, operation, actor = { uid }) {
  const write = operation.action === 'openReportReview';
  await authorizeDataBinding(tx, db, uid, scope, operation.dataset, write);
  const root = series(db, operation.dataset);
  const release = operation.release ?? (await tx.get(root)).data()?.head;
  if (!release) missing();
  const ref = root.collection('releases').doc(release);
  const [meta, approved, limit] = (await tx.getAll(ref, ref.collection('audience').doc(uid),
    root.collection('limits').doc(uid))).map(entry => entry.data());
  if (!meta || approved?.generation !== scope.generation) missing();
  const current = limit?.generation === scope.generation ? limit.partitions : '*';
  const parts = await tx.get(ref.collection('chunks'));
  if (parts.size !== meta.chunkCount) fail(503, 'DATA_UNAVAILABLE');
  const permissions = [approved.partitions, current];
  if (actor.sharedBotId) {
    if (!meta.bots.includes(actor.sharedBotId)) denied();
    // A shared Bot's transcript is visible in its room. Every participant must
    // be able to receive each aggregate that is handed to that task.
    const teamRef = db.collection('teams').doc(scope.teamId);
    const room = (await tx.get(teamRef.collection('conversations').doc(scope.conversationId))).data();
    if (!room?.memberUids?.length || room.memberUids.length > 500) denied();
    const binding = (await tx.get(db.collection('lingxiDataBindings').doc(operation.dataset))).data();
    const admitted = new Map(binding.audience.map(member => [member.uid, member.generation]));
    for (let i = 0; i < room.memberUids.length; i += 60) {
      const ids = room.memberUids.slice(i, i + 60);
      const entries = await tx.getAll(...ids.flatMap(reader => [teamRef.collection('members').doc(reader),
        ref.collection('audience').doc(reader), root.collection('limits').doc(reader)]));
      for (const [index, reader] of ids.entries()) {
        const [member, admission, restriction] = entries.slice(index * 3, index * 3 + 3).map(doc => doc.data());
        if (!member || member.generation !== admission?.generation || admitted.get(reader) !== member.generation) denied();
        permissions.push(admission.partitions);
        if (restriction?.generation === member.generation) permissions.push(restriction.partitions);
      }
    }
  }
  const cells = parts.docs.flatMap(part => part.data().facts)
    .filter(cell => permissions.every(allow => included(allow, cell.partition)));
  if (write) {
    if (!cells.some(cell => cell.partition === operation.partition && cell.group === operation.group)) denied();
    const key = fingerprint([release, operation.partition, operation.group, 'review']);
    const item = root.collection('reviews').doc(key);
    const prior = await tx.get(item);
    if (!prior.exists) tx.create(item, { release, partition: operation.partition, group: operation.group, actor, state: 'open' });
    return { review: key, state: prior.data()?.state ?? 'open' };
  }
  const totals = new Map();
  const scale = meta.spec.metric.scale ?? 0;
  for (const cell of cells) totals.set(cell.group, (totals.get(cell.group) ?? 0n) + BigInt(cell.units));
  let canReview = false;
  if (!actor.sharedBotId) {
    try { await authorizeDataBinding(tx, db, uid, scope, operation.dataset, true); canReview = true; }
    catch (error) { if (!(error instanceof LingxiDataError) || error.status !== 403) throw error; }
  }
  return { dataset: operation.dataset, release, spec: meta.spec, createdAt: meta.createdAt, canReview,
    provenance: { kind: meta.origin.kind, version: meta.origin.epoch ?? meta.origin.version,
      ...(uid === meta.creator && !actor.sharedBotId ? { inputDigest: meta.inputDigest } : {}),
      ...(meta.schemaVersion ? { schemaVersion: meta.schemaVersion } : {}) },
    points: [...totals.keys()].sort().map(group => ({ group, value: decimal(totals.get(group), scale) })),
    cells: cells.map(cell => ({ partition: cell.partition, group: cell.group, value: decimal(BigInt(cell.units), scale) })) };
}

export function createReportGateway({ db, forward, projectActor, readTeamAudience }) {
  return async function report(uid, token, raw) {
    const parsed = request.safeParse(raw);
    if (!parsed.success) invalid();
    const { scope, command } = parsed.data;
    const { dataset } = command;
    const root = series(db, dataset);
    if (command.action === 'getReport' || command.action === 'openReportReview') {
      return db.runTransaction(tx => deliverReport(tx, db, uid, scope, command));
    }
    async function context(tx) {
      await authorizeDataBinding(tx, db, uid, scope, dataset, true, true);
      const roster = await readTeamAudience(tx, db, uid, scope, []);
      const [target, state] = (await tx.getAll(db.collection('lingxiDataBindings').doc(dataset), root)).map(doc => doc.data());
      const members = roster.audience.filter(member => target.audience.some(entry => entry.uid === member.uid && entry.generation === member.generation));
      return { members, head: state?.head ?? null, audienceRevision: fingerprint([scope, members]) };
    }
    if (command.action === 'reportContext') return db.runTransaction(context);
    if (command.action === 'limitReportReader') return db.runTransaction(async tx => {
      const { members } = await context(tx);
      if (!members.some(member => member.uid === command.uid && member.generation === command.generation)) denied();
      const ref = root.collection('limits').doc(command.uid);
      const previous = (await tx.get(ref)).data()?.version ?? 0;
      if (previous !== command.expectedVersion) conflict();
      tx.set(ref, { generation: command.generation, partitions: command.partitions, version: previous + 1 });
      return { version: previous + 1 };
    });
    const definition = command.plan;
    const input = definition.origin;
    const inputScope = input.kind === 'personal' ? { kind: 'personal' } : scope;
    async function authority(tx) {
      const ctx = await context(tx);
      if (ctx.audienceRevision !== definition.audienceRevision) conflict();
      const audience = definition.readers.map(entry => {
        const member = ctx.members.find(member => member.uid === entry.uid);
        if (!member) denied();
        return { ...entry, generation: member.generation };
      });
      let key;
      if (input.kind === 'source') {
        const source = (await tx.get(db.collection('lingxiManagedDataSources').doc(input.source))).data();
        if (!source || source.uid !== uid || source.pending || !source.summary.enabled || source.summary.version !== input.version) denied();
        if (source.scope.kind === 'team' && (source.scope.teamId !== scope.teamId || source.scope.conversationId !== scope.conversationId
          || source.scope.generation !== scope.generation)) denied();
        key = source.key;
      } else {
        if (input.kind === 'team' && input.dataset !== dataset) denied();
        await authorizeDataBinding(tx, db, uid, inputScope, input.dataset, true, true);
      }
      const binding = fingerprint(['lingxi.report/1', uid, scope, dataset, definition, audience, key ?? null]);
      return { ctx, binding, audience, key };
    }
    const initial = await db.runTransaction(authority);
    const prepare = command.action === 'prepareReport';
    if (!prepare && input.kind !== 'source') {
      if (command.capture) invalid();
      const release = fingerprint([initial.binding, command.confirmation]);
      if ((await root.collection('releases').doc(release).get()).exists) return { dataset, release };
    }
    if (prepare && initial.ctx.head !== definition.parent) conflict();
    if (prepare && input.kind === 'source') return { binding: initial.binding };
    let result;
    if (input.kind === 'source') {
      result = attest(command.capture, initial.key, uid, initial.binding, definition.spec, input.source);
    } else {
      if (command.capture) invalid();
      const authorize = () => db.runTransaction(authority);
      const read = () => forward(token, null, { action: 'summarize', dataset: input.dataset,
        expectedEpoch: input.epoch, spec: definition.spec });
      const value = projectActor ? await projectActor(uid, inputScope, authorize, read) : await read();
      const checked = facts.safeParse(value.facts);
      if (!checked.success || value.epoch !== input.epoch) conflict();
      const inputDigest = fingerprint([input, value.schemaVersion, checked.data]);
      result = { facts: checked.data, inputDigest, schemaVersion: value.schemaVersion, confirmation: fingerprint([initial.binding, inputDigest]) };
    }
    if (prepare) {
      await db.runTransaction(authority);
      return { binding: initial.binding, confirmation: result.confirmation, facts: result.facts };
    }
    if (result.confirmation !== command.confirmation) conflict();
    const release = fingerprint([initial.binding, result.confirmation]);
    const ref = root.collection('releases').doc(release);
    return db.runTransaction(async tx => {
      const { ctx, binding, audience } = await authority(tx);
      if (binding !== initial.binding) conflict();
      if ((await tx.get(ref)).exists) return { dataset, release };
      if (ctx.head !== definition.parent) conflict();
      const chunks = [];
      for (let i = 0; i < result.facts.length; i += 100) chunks.push(result.facts.slice(i, i + 100));
      if (chunks.length + audience.length + 2 > 500) fail(507, 'DATA_CAPACITY');
      tx.create(ref, { parent: definition.parent, spec: definition.spec, creator: uid, createdAt: Date.now(),
        origin: input, bots: definition.bots, inputDigest: result.inputDigest, confirmation: result.confirmation, chunkCount: chunks.length,
        ...(result.schemaVersion ? { schemaVersion: result.schemaVersion } : {}) });
      chunks.forEach((part, index) => tx.create(ref.collection('chunks').doc(String(index)), { facts: part }));
      audience.forEach(({ uid: recipient, ...grant }) => tx.create(ref.collection('audience').doc(recipient), grant));
      tx.set(root, { head: release });
      return { dataset, release };
    });
  };
}
