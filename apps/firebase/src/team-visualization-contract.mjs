import { createHash } from 'node:crypto';
import { z } from 'zod';
import { isResourceRef } from './dataset-resource.mjs';
import { LingxiDataError } from './lingxi-data.mjs';
import { cancelVisualizationCleanup, rowRef } from './team-visualization-storage.mjs';

export const id = z.string().regex(/^[A-Za-z0-9_-]{1,64}$/u);
export const messageId = id.refine(value => value.startsWith('viz_'));
export const versionId = z.string().regex(/^[a-f0-9]{64}$/u);
export const scope = { teamId: id, conversationId: id, generation: z.number().int().positive() };
export const mode = z.enum(['pinned', 'live', 'edit']);
export const mib = 1024 * 1024;
export const maxPackageMiB = 50;
export const maxPackageBytes = maxPackageMiB * mib;
export const packagePartBytes = 2 * mib;
export const maxPackageParts = Math.ceil(maxPackageBytes / packagePartBytes);
export const visualizationRef = z.custom(value => isResourceRef(value) && value.kind === 'pageVersion');
export const dataRef = z.custom(value => isResourceRef(value) && value.kind !== 'pageVersion');
export const fail = (status, code) => { throw new LingxiDataError(status, code); };
export const hash = (value, encoding = 'hex') => createHash('sha256').update(value).digest(encoding);
export function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(item => canonical(item) ?? 'null').join(',')}]`;
  if (!value || typeof value !== 'object') return JSON.stringify(value);
  const fields = Object.keys(value).sort().filter(field => value[field] !== undefined);
  return `{${fields.map(field => `${JSON.stringify(field)}:${canonical(value[field])}`).join(',')}}`;
}
export const key = value => hash(canonical(value), 'base64url');
export const title = z.string().trim().min(1).max(80);

export function visualizationChannelEnabled(channels, team, conversation) {
  return (channels ?? '').split(',').some(value => value.trim() === '*' || value.trim() === team + '/' + conversation);
}

export const visualizationPackage = z.object({
  title,
  htmlBytes: z.number().int().min(1).max(maxPackageBytes),
  assets: z.array(z.object({
    name: z.string().regex(/^[A-Za-z0-9_-][A-Za-z0-9_.-]{0,63}$/u),
    mime: z.enum(['image/png', 'image/jpeg', 'image/webp', 'text/css', 'text/javascript']),
    bytes: z.number().int().min(0).max(maxPackageBytes),
  }).strict()).max(32),
  parts: z.array(versionId).min(1).max(maxPackageParts),
}).strict().refine(value => {
  const size = packageBytes(value);
  return size <= maxPackageBytes && value.parts.length === Math.ceil(size / packagePartBytes)
    && new Set(value.assets.map(asset => asset.name)).size === value.assets.length;
});

export const packageBytes = value => value.htmlBytes + value.assets.reduce((total, asset) => total + asset.bytes, 0);
export const partSize = (value, index) => Math.min(packagePartBytes, packageBytes(value) - index * packagePartBytes);
export const completeUpload = row => row.package?.parts.every((_, index) => row.uploaded?.includes(index)) === true;
export function visualizationLimit(thread) {
  const saved = thread.visualizationLimit;
  return Number.isInteger(saved?.limitMiB) && saved.limitMiB >= 1 && saved.limitMiB <= maxPackageMiB
    && Number.isSafeInteger(saved.revision) && saved.revision > 0 ? saved : { limitMiB: 1, revision: 0 };
}

export function checkVisualizationSize(thread, size) {
  if (size > visualizationLimit(thread).limitMiB * mib) fail(413, 'VISUALIZATION_TOO_LARGE');
}

export const binding = z.object({ alias: id, reference: dataRef, grant: id, mode }).strict();
export const descriptors = z.array(binding).max(4).refine(items => new Set(items.map(item => item.alias)).size === items.length);
export const descriptor = z.object({ reference: visualizationRef, title, bindings: descriptors }).strict();
export const projection = z.object({
  fields: z.array(id).min(1).max(64).optional(),
  partitions: z.array(z.string().min(1).max(160)).min(1).max(64).optional(),
}).strict();

export function sameAudience(left, right) {
  return left && right && key(left) === key(right);
}

export function conversationRef(db, location) {
  return db.doc(`teams/${location.teamId}/conversations/${location.conversationId}`);
}

export async function threadAccess(tx, db, location, uid, write = false) {
  const ref = conversationRef(db, location);
  const team = ref.parent.parent;
  const [t, c, m] = await tx.getAll(team, ref, team.collection('members').doc(uid));
  const thread = c.data();
  if (t.data()?.state !== 'active' || !thread || thread.kind !== 'channel'
      || !thread.memberUids?.includes(uid) || !m.exists) fail(403, 'DATA_ACCESS_REQUIRED');
  const generation = m.data().generation;
  if (generation !== location.generation) fail(409, 'STALE_MEMBERSHIP');
  if (write && !thread.participants.includes('user:' + uid)) fail(403, 'DATA_ACCESS_REQUIRED');
  return { ref, team, thread, generation, member: m.data() };
}

export async function channelAudience(tx, team, thread) {
  const members = (await tx.getAll(...thread.memberUids.map(uid => team.collection('members').doc(uid)))).filter(m => m.exists);
  if (members.some(m => !Number.isSafeInteger(m.data()?.generation) || m.data().generation < 1)) {
    fail(409, 'DATA_SHARE_CHANGED');
  }
  return Object.fromEntries(members.map(m => [m.id, m.data().generation]));
}

function validGeneration(member, grant, uid, thread) {
  const generation = member?.generation;
  return Number.isSafeInteger(generation) && generation > 0 && thread?.memberUids?.includes(uid)
    && Object.hasOwn(grant.audience ?? {}, uid) && grant.audience[uid] === generation;
}

async function checkDatasetOwner(tx, db, uid, owner) {
  if (owner?.kind === 'user' && owner.id === uid) return;
  if (owner?.kind === 'team' && id.safeParse(owner.id).success) {
    const team = db.collection('teams').doc(owner.id);
    const [root, member] = await tx.getAll(team, team.collection('members').doc(uid));
    if (root.data()?.state === 'active' && root.data().kind !== 'pair'
        && ['owner', 'admin'].includes(member.data()?.role)) return;
  }
  fail(409, 'DATA_SHARE_CHANGED');
}

async function checkGrantDelivery(tx, thread, grant, grantId) {
  if (!id.safeParse(grant.message).success || !dataRef.safeParse(grant.reference).success) fail(409, 'DATA_SHARE_CHANGED');
  const source = (await tx.get(thread.collection('messages').doc(grant.message))).data();
  if (!source || source.recalled) fail(409, 'DATA_SHARE_CHANGED');
  const delivery = grant.visualization;
  if (delivery) {
    if (grant.status !== 'pending' || delivery.message !== grant.message || !versionId.safeParse(delivery.version).success
        || source.kind !== 'visualization' || source.author?.kind !== 'user' || source.author.uid !== grant.issuer
        || source.visualization?.reference?.page !== delivery.message || source.visualization.reference.version !== delivery.version
        || !source.visualization?.bindings?.some(binding => binding.alias === delivery.binding && binding.grant === grantId
          && dataRef.safeParse(binding.reference).success && key(binding.reference) === key(grant.reference))) fail(409, 'DATA_SHARE_CHANGED');
    const publication = (await tx.get(rowRef(thread.firestore, delivery.message, delivery.version))).data();
    if (publication?.status !== 'active' || publication.message !== delivery.message || publication.version !== delivery.version
        || key(publication.destination) !== key(grant.destination)) fail(409, 'DATA_SHARE_CHANGED');
    return;
  }
  if (source.resourceGrant !== grantId || !dataRef.safeParse(source.resourceRef).success
      || key(source.resourceRef) !== key(grant.reference)) fail(409, 'DATA_SHARE_CHANGED');
  const task = grant.task;
  if (!task) {
    if (grant.status !== 'active' || source.author?.kind !== 'user' || source.author.uid !== grant.issuer) fail(409, 'DATA_SHARE_CHANGED');
    return;
  }
  if (!id.safeParse(task.taskKey).success || !id.safeParse(task.permit).success || source.author?.kind !== 'task'
      || source.author.taskId !== task.task || source.author.logicalRun !== task.logicalRun) fail(409, 'DATA_SHARE_CHANGED');
  const db = thread.firestore;
  const [current, permit, receipt] = await tx.getAll(db.doc(`datasetV2Tasks/${task.taskKey}`),
    db.doc(`datasetV2Permits/${task.permit}`), db.doc(`datasetV2Deliveries/${grantId}`));
  if (current.data()?.permit !== task.permit || current.data()?.deleted === true || !permit.exists || permit.data().revoked === true
      || (grant.status !== 'active' && (grant.status !== 'pending' || receipt.data()?.state !== 'delivered'))) {
    fail(409, 'DATA_SHARE_CHANGED');
  }
}

/** Fence the same live grant facts checked by Core's access::effective in the message transaction.
 * A grant prepared for this card has no message yet. Its delivery becomes visible in this transaction. */
async function checkBinding(tx, channel, audience, owner, message, version, title, item) {
  const db = channel.firestore;
  const dataset = db.collection('datasetV2').doc(item.reference.dataset);
  const [root, leaf] = await tx.getAll(dataset, dataset.collection('grants').doc(item.grant));
  const meta = root.data();
  let grant = leaf.data(), grantId = item.grant;
  if (!meta || meta.deleted !== false || !grant || grant.destination?.team !== channel.parent.parent.id
      || grant.destination?.conversation !== channel.id || !dataRef.safeParse(grant.reference).success
      || key(grant.reference) !== key(item.reference)
      || (item.mode !== grant.mode && !(grant.mode === 'edit' && item.mode === 'live'))
      || Object.entries(audience).some(([uid, generation]) => !Object.hasOwn(grant.audience ?? {}, uid) || grant.audience[uid] !== generation)) {
    fail(409, 'DATA_SHARE_CHANGED');
  }
  const beforeDelivery = grant.visualization?.message === message;
  if (beforeDelivery && (grant.status !== 'pending' || grant.issuer !== owner || grant.message !== message
      || grant.visualization.version !== version || grant.visualization.binding !== item.alias
      || grant.textDigest !== hash(title, 'base64url') || !sameAudience(audience, grant.audience))) fail(409, 'DATA_SHARE_CHANGED');
  let subject;
  for (let depth = 0; depth < 8; depth++) {
    if (!grant || !id.safeParse(grant.destination?.team).success || !id.safeParse(grant.destination?.conversation).success
        || typeof grant.issuer !== 'string' || !grant.issuer || grant.issuer.includes('/')) fail(409, 'DATA_SHARE_CHANGED');
    const team = db.collection('teams').doc(grant.destination.team);
    const threadRef = team.collection('conversations').doc(grant.destination.conversation);
    const [teamDoc, threadDoc, issuerDoc, subjectDoc] = await tx.getAll(team, threadRef, team.collection('members').doc(grant.issuer),
      ...(subject ? [team.collection('members').doc(subject)] : []));
    const thread = threadDoc.data();
    if (teamDoc.data()?.state !== 'active' || !validGeneration(issuerDoc.data(), grant, grant.issuer, thread)
        || !thread.participants?.includes(`user:${grant.issuer}`)
        || (subject && (!grant.reshare || !validGeneration(subjectDoc?.data(), grant, subject, thread)
          || !thread.participants.includes(`user:${subject}`)))) fail(409, 'DATA_SHARE_CHANGED');
    if (depth !== 0 || !beforeDelivery) await checkGrantDelivery(tx, threadRef, grant, grantId);
    subject = grant.issuer;
    if (grant.parent == null) return checkDatasetOwner(tx, db, subject, meta.owner);
    if (!id.safeParse(grant.parent).success) fail(409, 'DATA_SHARE_CHANGED');
    grantId = grant.parent;
    grant = (await tx.get(dataset.collection('grants').doc(grantId))).data();
  }
  fail(409, 'DATA_SHARE_CHANGED');
}

export async function confirmVisualizationBindings(tx, channel, audience, owner, message, version, title, items) {
  for (const item of items) await checkBinding(tx, channel, audience, owner, message, version, title, item);
}

/** Used by the normal Teams message transaction before either the message or the delivery is visible. */
export async function confirmVisualizationDelivery(tx, team, thread, input, uid, value, now, task) {
  const spec = descriptor.parse(value);
  const ref = rowRef(team.firestore, spec.reference.page, spec.reference.version);
  const row = (await tx.get(ref)).data();
  if (thread.kind !== 'channel' || !row || row.uid !== uid || row.message !== input.messageId
      || row.destination.team !== input.teamId || row.destination.conversation !== input.conversationId
      || row.version !== spec.reference.version || row.title !== spec.title || row.taskId !== task
      || row.status !== 'pending' || !(row.expiresAt > now) || !completeUpload(row)
      || key(row.bindings) !== key(spec.bindings)) {
    fail(409, 'DATA_SHARE_CHANGED');
  }
  checkVisualizationSize(thread, row.bytes);
  if (!sameAudience(row.audience, await channelAudience(tx, team, thread))) fail(409, 'DATA_SHARE_CHANGED');
  await confirmVisualizationBindings(tx, conversationRef(team.firestore, input), row.audience, uid, input.messageId,
    spec.reference.version, spec.title, spec.bindings);
  return () => {
    tx.update(ref, { status: 'active' });
    cancelVisualizationCleanup(tx, team.firestore, spec.reference);
  };
}
