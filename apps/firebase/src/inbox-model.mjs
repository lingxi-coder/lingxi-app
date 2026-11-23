import { createHash } from 'node:crypto';
import { z } from 'zod';
import { messageMentionRecipients } from './team-mentions.mjs';
export const DAY = 86400000;
export const id = z.string().regex(/^[A-Za-z0-9_-]{1,128}$/u);
const cursor = z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER);
const generation = cursor.positive();
const key = z.string().regex(/^(team|service|assistant):[A-Za-z0-9_-]{1,128}:[A-Za-z0-9_-]{1,128}$/u);
const target = { key, generation };
const page = {
  after: z
    .object({
      lastActivityAt: cursor,
      key: z.string().regex(/^[A-Za-z0-9_:-]{1,300}$/u),
      pinOrder: cursor.nullable().optional(),
      titleSort: z.string().max(512).optional()
    })
    .strict()
    .optional()
};
const secret = z.string().regex(/^[A-Za-z0-9_-]{32,128}$/u);
const read = z
  .object({
    ...target,
    through: cursor,
    manualUnreadRevision: cursor,
    operationId: id.optional(),
    observedPendingActionRevision: cursor.optional(),
    observedFailureRevision: cursor.optional()
  })
  .strict();
const label = max =>
  z
    .string()
    .trim()
    .min(1)
    .max(max)
    .refine(s => !/[\u0000-\u001f\u007f/\\]/u.test(s));
const state = z.enum([
  'idle',
  'running',
  'completed',
  'failed',
  'waiting_for_input',
  'waiting_for_approval',
  'offline'
]);
const preferences = z
  .object({
    pinOrder: cursor.nullable().optional(),
    archived: z.boolean().optional(),
    notificationMode: z.enum(['all', 'mentions', 'none']).optional()
  })
  .strict();
const listScope = {
  archived: z.boolean().optional(),
  searchPrefix: z.string().max(120).optional(),
  filter: z.enum(['all', 'unread', 'attention']).optional(),
  category: z.enum(['team', 'friend', 'service', 'assistant', 'messages']).optional()
};
export const command = z.discriminatedUnion('action', [
  z.object({ action: z.literal('list'), ...page, ...listScope }).strict(),
  z
    .object({ action: z.literal('notice'), id: z.string().regex(/^[a-f0-9]{64}$/u), deviceId: id.optional() })
    .strict(),
  z.object({ action: z.literal('notices'), deviceId: id.optional(), ...page }).strict(),
  z.object({ action: z.literal('get'), key, generation: generation.optional() }).strict(),
  z.object({ action: z.literal('markRead'), ...read.shape }).strict(),
  z.object({ action: z.literal('markAllReadSnapshot'), operationId: id, requestedAt: cursor.optional(), ...listScope }).strict(),
  z.object({ action: z.literal('readBatchStatus'), jobId: id }).strict(),
  z.object({ action: z.literal('markAllRead'), entries: z.array(read).min(1).max(50) }).strict(),
  z
    .object({
      action: z.literal('markUnread'),
      ...target,
      operationId: id,
      expectedManualUnreadRevision: cursor.optional()
    })
    .strict(),
  z
    .object({
      action: z.literal('preferences'),
      ...target,
      expectedPreferencesRevision: cursor.optional(),
      patch: preferences
    })
    .strict(),
  z.object({ action: z.literal('accountPreferences'), dmReceipts: z.boolean().optional() }).strict(),
  z.object({ action: z.literal('peerReceipt'), ...target }).strict(),
  z
    .object({
      action: z.literal('presence'),
      deviceId: id,
      key: key.nullable(),
      generation: generation.optional(),
      foreground: z.boolean()
    })
    .strict(),
  z
    .object({
      action: z.literal('assistantSource'),
      sourceId: id,
      sourceSecret: secret,
      enabled: z.boolean(),
      consentRevision: generation.optional()
    })
    .strict(),
  z
    .object({
      action: z.literal('assistantRemove'),
      sourceId: id,
      sourceSecret: secret,
      conversationId: id,
      generation,
      sourceRevision: cursor,
      conversationConsentRevision: generation.optional()
    })
    .strict(),
  z
    .object({
      action: z.literal('assistantPublish'),
      sourceId: id,
      sourceSecret: secret,
      conversationId: id,
      generation,
      baseline: cursor,
      conversationConsentRevision: generation.optional(),
      failureAt: cursor.optional(),
      failureRevision: cursor.optional(),
      sourceRevision: cursor,
      sourceCursor: cursor,
      lastActivityAt: cursor,
      state,
      pendingActionCount: cursor,
      pendingActionAt: cursor.optional(),
      pendingActionRevision: cursor.optional(),
      title: label(120).optional(),
      sourceLabel: label(80).optional(),
      remoteHostId: z
        .string()
        .regex(/^[A-Za-z0-9_-]{22}$/u)
        .optional(),
      previewKind: z.enum(['reply', 'taskResult', 'failure']).nullable()
    })
    .strict()
]);
export const digest = (...parts) => createHash('sha256').update(JSON.stringify(parts)).digest('hex');
export const millis = v => v?.toMillis?.() ?? v?.getTime?.() ?? Number(v ?? 0);
export function splitKey(key) {
  const [kind, sourceId, conversationId] = key.split(':');
  return { kind, sourceId, conversationId };
}
export const makeKey = r => [r.kind, r.sourceId, r.conversationId].join(':');
export const capped = n => Math.min(100, Math.max(0, n));
export const attention = r => !!r && (!!r.attentionKind || r.mentionCount > 0 || r.pendingActionCount > 0);
export const unread = r => !!r && (r.manualUnread || r.unreadCount > 0);
export const badge = r => Boolean(r && !r.archived && (unread(r) || attention(r)));
export const messageCategories = ['friend', 'team', 'service'];
export const shard = key => String(parseInt(digest(key).slice(0, 2), 16) % 16);
export const deny = (status, code) => {
  throw Object.assign(new Error(code), { status, code });
};
export function emptyRow(ref) {
  return {
    schemaVersion: 1,
    key: makeKey(ref),
    ref,
    category: ref.kind,
    title: '',
    preview: '',
    lastActivityAt: 0,
    sourceCursor: 0,
    sourceRevision: 0,
    readThrough: 0,
    unreadCount: 0,
    mentionCount: 0,
    pendingActionCount: 0,
    pendingActionAt: 0,
    pendingActionRevision: 0,
    state: 'idle',
    preferencesRevision: 0,
    pinOrder: null,
    notificationMode: 'mentions',
    archived: false,
    manualUnread: false,
    manualUnreadRevision: 0,
    stateRevision: 0,
    offline: false
  };
}
export function applyRead(row, through, manualRevision) {
  const clear = row.manualUnread && row.manualUnreadRevision === manualRevision;
  const readThrough = Math.max(row.readThrough, through);
  return {
    ...row,
    readThrough,
    manualUnread: clear ? false : row.manualUnread,
    manualUnreadRevision: row.manualUnreadRevision + Number(clear),
    stateRevision: row.stateRevision + Number(clear || row.readThrough !== readThrough)
  };
}
export function messageEvent(m, uid, conversation, reply) {
  if (
    !m ||
    m.recalled ||
    m.author?.kind !== 'user' ||
    m.author.uid === uid ||
    !['text', 'status', 'question', 'task_ref', 'app_ref'].includes(m.kind)
  )
    return null;
  const mention = messageMentionRecipients(m).includes(uid);
  return {
    cursor: m.seq,
    kind: mention
      ? 'mention'
      : reply?.author?.kind === 'user' && reply.author.uid === uid
        ? 'reply'
        : conversation.kind === 'dm'
          ? 'dm'
          : 'message',
    mention: Boolean(mention)
  };
}
export const delaySeconds = kind => (['input', 'approval'].includes(kind) ? 6 : kind === 'completion' ? 5 : 0);
export function alertEnabled(mode, kind) {
  return mode !== 'none' && (kind !== 'message' || mode === 'all');
}

export const searchTitle = value => value.normalize('NFKC').trim().toLowerCase();
export function prefixEnd(prefix) {
  const points = Array.from(prefix);
  while (points.length) {
    const cp = points.pop().codePointAt(0);
    if (cp < 0x10ffff) return points.join('') + String.fromCodePoint(cp === 0xd7ff ? 0xe000 : cp + 1);
  }
  return null;
}
export function deliveryTime(event, deviceId) {
  const mode = event.timing;
  const fallback = Boolean(deviceId) && (!mode || mode.multiple || (mode.primary && mode.primary !== deviceId));
  return event.lastActivityAt + Math.max(fallback ? 15 : 0, delaySeconds(event.kind)) * 1000;
}
