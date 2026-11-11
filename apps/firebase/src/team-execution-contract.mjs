import { createHash } from 'node:crypto';
import { z } from 'zod';

const id = z.string().regex(/^[A-Za-z0-9_-]{8,64}$/u);
const uid = z.string().regex(/^[A-Za-z0-9_-]{1,128}$/u);
const positive = z.number().int().positive().max(Number.MAX_SAFE_INTEGER);
const cursor = z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER);
export const memberRef = z.object({ uid, generation: positive }).strict();
export const usePolicy = z.discriminatedUnion('mode', [
  z.object({ mode: z.literal('all') }).strict(),
  z.object({ mode: z.literal('listed'), members: z.array(memberRef).max(200)
    .refine(values => new Set(values.map(value => value.uid)).size === values.length, 'Duplicate member') }).strict(),
]);
export const sharedBotRecord = z.object({
  schemaVersion: z.literal(1), sharedBotId: id, teamId: id, maintainer: memberRef,
  revision: positive, state: z.enum(['draft', 'published', 'paused', 'retired']),
  usePolicy, activeDeploymentId: id,
}).strict();
export const deploymentRecord = z.object({
  deploymentId: id, sharedBotId: id, revision: positive, hostId: id, owner: memberRef,
  instanceId: id, definitionDigest: z.string().regex(/^[a-f0-9]{64}$/u),
  capabilitySnapshotDigest: z.string().regex(/^[a-f0-9]{64}$/u), executionVersion: z.literal(2),
  capabilities: z.object({ durableWait: z.boolean(), writeOperations: z.literal(false), delegation: z.literal(false) }).strict(),
  accountEpoch: positive, enabled: z.boolean(),
}).strict();

export const sourceMessage = z.object({
  messageId: id, seq: positive, changeSeq: positive, changeId: id,
  author: z.discriminatedUnion('kind', [
    z.object({ kind: z.literal('user'), uid, displayName: z.string().min(1).max(80) }).strict(),
    z.object({ kind: z.literal('shared_bot'), sharedBotId: id, displayName: z.string().min(1).max(80) }).strict(),
  ]),
  createdAt: z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER),
  editedAt: z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER).optional(),
  text: z.string(), replyTo: id.optional(), recalled: z.boolean().optional(),
  sourceTaskId: id.optional(), deliveryId: id.optional(), artifactId: id.optional(),
}).strict().refine(value => !value.recalled || value.text === '', 'Recalled source must not retain its text');
export const chatInputBatch = z.object({
  schemaVersion: z.literal(1), batchId: id, teamId: id, sharedBotId: id, conversationId: id,
  sessionEpoch: positive, expectedSessionRevision: cursor,
  fromSeqExclusive: cursor, toSeqInclusive: positive, lastChangeSeq: cursor,
  bootstrap: z.boolean(), currentMessageId: id, messages: z.array(sourceMessage).min(1), changes: z.array(sourceMessage),
}).strict().superRefine((value, ctx) => {
  const reject = message => ctx.addIssue({ code: 'custom', message });
  if (value.toSeqInclusive <= value.fromSeqExclusive) reject('Empty or reversed interval');
  if (value.messages.length !== value.toSeqInclusive - value.fromSeqExclusive) reject('Incomplete interval');
  const ids = new Set();
  for (const [index, message] of value.messages.entries()) {
    if (message.seq !== value.fromSeqExclusive + index + 1) reject('Messages must be complete and ordered');
    if (ids.has(message.messageId)) reject('Duplicate source message');
    ids.add(message.messageId);
    if (message.changeSeq > value.lastChangeSeq) reject('Change exceeds batch watermark');
  }
  const current = value.messages.at(-1);
  if (current?.messageId !== value.currentMessageId) reject('Current request must be the interval endpoint');
  if (value.bootstrap && value.fromSeqExclusive !== Math.max(0, value.toSeqInclusive - 200)) reject('Bootstrap must include the latest 200 available messages');
  const changed = new Set();
  for (const message of value.changes) {
    if (value.bootstrap || message.seq > value.fromSeqExclusive || message.changeSeq > value.lastChangeSeq) reject('Invalid prior-source change');
    if (changed.has(message.messageId)) reject('Duplicate source change');
    changed.add(message.messageId);
  }
});

export function admissionId(messageId, sharedBotId) {
  id.parse(messageId); id.parse(sharedBotId);
  return createHash('sha256').update(JSON.stringify([messageId, sharedBotId])).digest('hex');
}
export function sharedBotPermits(bot, requester) {
  const record = sharedBotRecord.parse(bot), member = memberRef.parse(requester);
  return record.state === 'published' && (record.usePolicy.mode === 'all'
    || record.usePolicy.members.some(value => value.uid === member.uid && value.generation === member.generation));
}

/** Called only after the host's shared message service has checked current ACL
 * and interval completeness. This does not fetch/cache messages or grant execution. */
export function buildChatInputBatch(input) {
  return chatInputBatch.parse(input);
}
