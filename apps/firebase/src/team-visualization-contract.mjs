import { createHash } from 'node:crypto';
import { z } from 'zod';
import { isResourceRef } from './dataset-resource.mjs';
import { LingxiDataError } from './lingxi-data.mjs';

export const id = z.string().regex(/^[A-Za-z0-9_-]{1,64}$/u);
export const messageId = id.refine(value => value.startsWith('viz_'));
export const versionId = z.string().regex(/^[a-f0-9]{64}$/u);
export const scope = { teamId: id, conversationId: id, generation: z.number().int().positive() };
export const mode = z.enum(['pinned', 'live', 'edit']);
export const maxPackageBytes = 2 * 1024 * 1024;
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

export const bundle = z.object({
  title,
  html: z.string().min(1).max(maxPackageBytes).refine(value => Boolean(value.trim())),
  assets: z.record(z.string().regex(/^[A-Za-z0-9_-][A-Za-z0-9_.-]{0,63}$/u), z.object({
    mime: z.enum(['image/png', 'image/jpeg', 'image/webp', 'text/css', 'text/javascript']),
    data: z.string().regex(/^[A-Za-z0-9+/]*={0,2}$/u).max(maxPackageBytes),
  }).strict()).default({}),
}).strict().refine(value => Object.keys(value.assets).length <= 32
  && Buffer.byteLength(JSON.stringify(value)) <= maxPackageBytes);

export const binding = z.object({ alias: id, reference: dataRef, grant: id, mode }).strict();
export const descriptors = z.array(binding).max(4).refine(items => new Set(items.map(item => item.alias)).size === items.length);
export const descriptor = z.object({ reference: visualizationRef, title, bindings: descriptors }).strict();
export const projection = z.object({
  fields: z.array(id).min(1).max(64).optional(),
  partitions: z.array(z.string().min(1).max(160)).min(1).max(64).optional(),
}).strict();

export function rowRef(db, page, version) {
  return db.doc(`datasetPages/${page}/versions/${version}`);
}

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
  return { ref, team, thread, generation };
}

export async function channelAudience(tx, team, thread) {
  const members = await tx.getAll(...thread.memberUids.map(uid => team.collection('members').doc(uid)));
  if (members.some(m => !Number.isSafeInteger(m.data()?.generation) || m.data().generation < 1)) {
    fail(409, 'DATA_SHARE_CHANGED');
  }
  return Object.fromEntries(members.map(m => [m.id, m.data().generation]));
}

export async function confirmVisualizationBindings(tx, channel, audience, owner, message, version, title, items) {
  if (!items.length) return;
  const db = channel.firestore;
  const snapshots = await tx.getAll(...items.map(item => db.doc(`datasetV2/${item.reference.dataset}/grants/${item.grant}`)));
  for (const [index, snapshot] of snapshots.entries()) {
    const item = items[index], grant = snapshot.data();
    const delivery = grant?.visualization;
    const owned = delivery?.message === message;
    if (!grant || !dataRef.safeParse(grant.reference).success
        || grant.destination?.team !== channel.parent.parent.id || grant.destination?.conversation !== channel.id
        || !sameAudience(audience, grant.audience) || key(grant.reference) !== key(item.reference)
        || (item.mode !== grant.mode && !(grant.mode === 'edit' && item.mode === 'live'))) {
      fail(409, 'DATA_SHARE_CHANGED');
    }
    if (owned) {
      if (grant.status !== 'pending' || grant.issuer !== owner || delivery.version !== version
          || delivery.binding !== item.alias || grant.textDigest !== hash(title, 'base64url')) fail(409, 'DATA_SHARE_CHANGED');
      continue;
    }
    if (!id.safeParse(grant.message).success) fail(409, 'DATA_SHARE_CHANGED');
    const source = (await tx.get(channel.collection('messages').doc(grant.message))).data();
    if (!source || source.recalled) fail(409, 'DATA_SHARE_CHANGED');
    if (delivery) {
      if (grant.status !== 'pending' || delivery.message !== grant.message || !versionId.safeParse(delivery.version).success
          || source.author?.kind !== 'user' || source.author.uid !== grant.issuer
          || source.visualization?.reference?.version !== delivery.version
          || !source.visualization?.bindings?.some(binding => binding.alias === delivery.binding && binding.grant === item.grant)) {
        fail(409, 'DATA_SHARE_CHANGED');
      }
      const publication = (await tx.get(rowRef(db, delivery.message, delivery.version))).data();
      if (publication?.status !== 'active') fail(409, 'DATA_SHARE_CHANGED');
    } else if (grant.status !== 'active' || source.resourceGrant !== item.grant || !dataRef.safeParse(source.resourceRef).success
        || key(source.resourceRef) !== key(item.reference)) {
      fail(409, 'DATA_SHARE_CHANGED');
    }
  }
}

/** Used by the normal Teams message transaction before either the message or the delivery is visible. */
export async function confirmVisualizationDelivery(tx, team, thread, input, uid, value, task) {
  const spec = descriptor.parse(value);
  const ref = rowRef(team.firestore, spec.reference.page, spec.reference.version);
  const row = (await tx.get(ref)).data();
  if (thread.kind !== 'channel' || !row || row.uid !== uid || row.message !== input.messageId
      || row.destination.team !== input.teamId || row.destination.conversation !== input.conversationId
      || row.version !== spec.reference.version || row.title !== spec.title || row.taskId !== task
      || !['pending', 'active'].includes(row.status) || key(row.bindings) !== key(spec.bindings)) {
    fail(409, 'DATA_SHARE_CHANGED');
  }
  if (!sameAudience(row.audience, await channelAudience(tx, team, thread))) fail(409, 'DATA_SHARE_CHANGED');
  await confirmVisualizationBindings(tx, conversationRef(team.firestore, input), row.audience, uid, input.messageId,
    spec.reference.version, spec.title, spec.bindings);
  return () => tx.update(ref, { status: 'active' });
}
