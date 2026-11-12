import { z } from 'zod';
import { deploymentRecord, memberRef, usePolicy } from './team-execution-contract.mjs';

export const executionId = z.string().regex(/^[A-Za-z0-9_-]{8,64}$/u);
const positive = z.number().int().positive().max(Number.MAX_SAFE_INTEGER);
const cursor = z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER);
const digest = z.string().regex(/^[a-f0-9]{64}$/u);
const text = z.string().min(1).refine(value => Buffer.byteLength(value, 'utf8') <= 8192);
const thread = { conversationId: executionId };
const task = { taskId: executionId };
const host = { deploymentId: executionId, hostId: executionId, accountEpoch: positive };
const unique = values => new Set(values).size === values.length;
export const publishedInput = z.object({
  resourceId: executionId, revision: positive, digest,
  kind: z.enum(['persona', 'file', 'skill']),
  // A digest is a content identity, not permission to widen the source audience.
  sourceAudience: z.discriminatedUnion('mode', [
    z.object({ mode: z.literal('owner') }).strict(),
    z.object({ mode: z.literal('team') }).strict(),
    z.object({ mode: z.literal('listed'), members: z.array(memberRef).max(200) }).strict(),
  ]),
}).strict();
const quota = z.number().int().min(1).max(10000);
// Results lacking explicit provenance remain owner-only and cannot enter a shared Chat.
const outputAudience = z.discriminatedUnion('mode', [
  z.object({ mode: z.literal('owner') }).strict(),
  z.object({ mode: z.literal('published_inputs') }).strict(),
]);
const grantIds = z.array(executionId).max(50).refine(unique);
/** How an interrupted operation may be recovered (detailed design 12.3). */
export const operationCategory = z.enum(['read', 'store_idempotent', 'provider_idempotent', 'queryable', 'unqueryable']);
const logicalStepId = z.string().regex(/^[A-Za-z0-9_.:/-]{1,200}$/u);
const payloads = {
  createConversation: z.object({ ...thread, kind: z.enum(['bot_dm', 'group', 'channel']),
    title: z.string().trim().min(1).max(120), participantUids: z.array(z.string().regex(/^[A-Za-z0-9_-]{1,128}$/u)).min(1).max(200).refine(unique),
    sharedBotIds: z.array(executionId).min(1).max(8).refine(unique) }).strict(),
  publishSharedBot: z.object({
    sharedBotId: executionId, expectedRevision: cursor, name: z.string().trim().min(1).max(80),
    description: z.string().max(500), usePolicy,
    deployment: deploymentRecord,
    inputs: z.array(publishedInput).min(1).max(50).refine(values => unique(values.map(value => value.resourceId))),
    quotas: z.object({ team: quota, member: quota, bot: quota }).strict(),
  }).strict().refine(value => value.inputs.filter(input => input.kind === 'persona').length === 1, 'Exactly one published persona required'),
  listSharedBots: z.object({}).strict(),
  pauseSharedBot: z.object({ sharedBotId: executionId, expectedRevision: positive }).strict(),
  heartbeatDeployment: z.object({ ...host, ready: z.boolean() }).strict(),
  activateDeployment: z.object({ deploymentId: executionId, hostId: executionId,
    expectedAccountEpoch: positive, newAccountEpoch: positive }).strict()
    .refine(value => value.newAccountEpoch > value.expectedAccountEpoch, 'Account epoch must advance'),
  submit: z.object({ ...thread, messageId: executionId, changeId: executionId, text,
    targets: z.array(executionId).min(1).max(8).refine(unique), replyTo: executionId.optional() }).strict(),
  listHostTasks: z.object({ ...host, after: executionId.optional() }).strict(),
  startTask: z.object({ ...host, ...task, expectedLastChangeSeq: cursor }).strict(),
  prepareTask: z.object({ ...host, ...task }).strict(),
  validateTask: z.object({ ...host, ...task, permitId: executionId, expectedLastChangeSeq: cursor }).strict(),
  completeTask: z.object({ ...host, ...task, permitId: executionId,
    outcome: z.enum(['completed', 'failed']), text, finishedAt: cursor,
    inputGrantIds: grantIds, outputAudience,
    document: z.object({ title: z.string().trim().min(1).max(120), body: z.string().min(1)
      .refine(value => Buffer.byteLength(value, 'utf8') <= 256 * 1024) }).strict().optional(),
    usage: z.discriminatedUnion('status', [
      z.object({ status: z.literal('missing') }).strict(),
      z.object({ status: z.enum(['partial', 'complete']), inputTokens: cursor, outputTokens: cursor,
        costMicros: cursor.optional(), currency: z.string().length(3).optional() }).strict(),
    ]),
  }).strict(),
  pauseTask: z.object({ ...host, ...task, permitId: executionId, waitId: executionId, kind: z.literal('human_input'),
    question: z.string().min(1).refine(value => Buffer.byteLength(value, 'utf8') <= 2000),
    requestDigest: digest, checkpointDigest: digest, pausedAt: cursor,
    expiresInMs: z.number().int().min(60000).max(86400000), dispatchedRequests: cursor,
    inputGrantIds: grantIds, outputAudience }).strict(),
  continueTask: z.object({ ...host, ...task, expectedEpoch: positive, checkpointDigest: digest,
    reason: z.enum(['answer', 'recovery']), dispatchedRequests: cursor }).strict(),
  // The operation ledger (M2 Q04). `operationId` is derived from the task and the durable logical step, never from a provider call id.
  recordOperation: z.object({ ...host, ...task, permitId: executionId, operationId: digest, logicalStepId,
    category: operationCategory, tool: z.object({ name: z.string().min(1).max(128), version: z.string().min(1).max(64) }).strict(),
    digest, approvalRef: executionId.optional(), dispatchedAt: cursor }).strict(),
  settleOperation: z.object({ ...host, ...task, permitId: executionId, operationId: digest,
    outcome: z.enum(['succeeded', 'failed', 'unknown']), resultDigest: digest,
    reference: z.string().min(1).max(200).optional(), settledAt: cursor }).strict(),
  resolveOperation: z.object({ ...task, operationId: digest, resolution: z.enum(['succeeded', 'failed']) }).strict(),
  listOperations: z.object(task).strict(),
  answerWait: z.object({ ...thread, waitId: executionId, messageId: executionId, changeId: executionId, text }).strict(),
  cancelTask: z.object(task).strict(),
  getTask: z.object(task).strict(),
  listTasks: z.object({ conversationId: executionId.optional(), after: executionId.optional() }).strict(),
  readArtifact: z.object({ artifactId: executionId }).strict(),
  setConversationPolicy: z.object({ ...thread, expectedRevision: cursor,
    targets: z.array(executionId).max(8).refine(unique) }).strict(),
};
export const sharedBotCommand = z.discriminatedUnion('action', Object.entries(payloads).map(([action, payload]) => z.object({
  schemaVersion: z.literal(1), commandId: executionId, teamId: executionId,
  generation: positive, action: z.literal(action), payload,
}).strict()));
export const executionActions = new Set(Object.keys(payloads));
export const definitionDigest = digest;
