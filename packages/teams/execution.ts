import { isMemberExecutionCommand } from './execution-validation.js';
/** Execution 2 contracts. No retired Bot keys or execution request adapters. */
export interface MemberRef { uid: string; generation: number }
export type UsePolicy = { mode: 'all' } | { mode: 'listed'; members: MemberRef[] };
export interface RequestQuotas { team: number; member: number; bot: number }
export interface SharedBotCard {
  sharedBotId: string; name: string; description: string; maintainer: MemberRef;
  revision: number; state: 'draft' | 'published' | 'paused' | 'retired';
  permitted: boolean; ready: boolean; onlineUntil: number; usePolicy: UsePolicy;
  quotas: RequestQuotas; instanceId?: string;
}
export type TaskState = 'queued' | 'running' | 'waiting' | 'completed' | 'failed' | 'expired' | 'interrupted' | 'authorization_required' | 'revoked' | 'cancelled' | 'needs_reconciliation';
export type TaskUsage = { status: 'missing' } | { status: 'partial' | 'complete'; inputTokens: number; outputTokens: number; costMicros?: number; currency?: string };
/** The human question a waiting task is blocked on; present only while `state` is `waiting`. */
export interface TaskWait { waitId: string; kind: 'human_input'; recipient: MemberRef; expiresAt: number; messageId: string }
export interface TeamTaskView {
  taskId: string; conversationId: string; sharedBotId: string; requester: MemberRef;
  /** `deadline` ends the current phase: pick-up limit while queued, running budget while running, wait expiry while waiting. */
  state: TaskState; createdAt: number; deadline: number; epoch: number; activeSpentMs: number; cancelRequestedAt?: number;
  /** Outside actions whose outcome nobody has confirmed yet; absent when there are none. */
  unverifiedOperations?: number;
  wait?: TaskWait; replyId?: string; artifactId?: string; usage?: TaskUsage;
}
export interface TeamDocument { title: string; body: string }
/** One revision of a shared document. `currentRevision` is the newest one, so `revision < currentRevision` is history. */
export interface TeamArtifact { artifactId: string; revision: number; digest: string; document: TeamDocument; currentRevision: number }
export type RevisionAuthor = { kind: 'user'; uid: string; generation: number } | { kind: 'shared_bot'; sharedBotId: string };
export interface ArtifactRevision { revision: number; parentRevision: number | null; digest: string; title: string; author: RevisionAuthor; createdAt: number; sourceTaskId?: string }
export interface ArtifactRevisions { artifactId: string; currentRevision: number; revisions: ArtifactRevision[] }
export interface ArtifactRevised { artifactId: string; revision: number; digest: string }
export type OperationCategory = 'read' | 'store_idempotent' | 'provider_idempotent' | 'queryable' | 'unqueryable';
export type OperationState = 'dispatched' | 'succeeded' | 'failed' | 'unknown';
/** One outside action of a task. Its parameters and result stay with the Bot's host. */
export interface TaskOperation {
  operationId: string; logicalStepId: string; category: OperationCategory; tool: { name: string; version: string };
  state: OperationState; epoch: number; dispatchedAt: number; settledAt?: number; resolvedBy?: MemberRef; resolvedAt?: number;
}
export interface TaskOperations { taskId: string; operations: TaskOperation[] }
export interface OperationResolved { taskId: string; state: TaskState; operationId: string; operationState: 'succeeded' | 'failed' }
/** Replays are safe for the other categories; these two need a confirmed outcome before a task may go on. */
export const operationUnverified = (operation: TaskOperation): boolean =>
  (operation.category === 'queryable' || operation.category === 'unqueryable') && (operation.state === 'dispatched' || operation.state === 'unknown');
export interface ExecutionScope { teamId: string; generation: number }
export interface MemberExecutionPayloads {
  listSharedBots: Record<string, never>;
  createConversation: { conversationId: string; kind: 'bot_dm' | 'group' | 'channel'; title: string; participantUids: string[]; sharedBotIds: string[] };
  submit: { conversationId: string; messageId: string; changeId: string; text: string; targets: string[]; replyTo?: string };
  pauseSharedBot: { sharedBotId: string; expectedRevision: number };
  answerWait: { conversationId: string; waitId: string; messageId: string; changeId: string; text: string };
  cancelTask: { taskId: string };
  getTask: { taskId: string };
  listTasks: { conversationId?: string; after?: string };
  readArtifact: { artifactId: string; revision?: number };
  listArtifactRevisions: { artifactId: string };
  reviseArtifact: { artifactId: string; expectedRevision: number; operationId: string; document: TeamDocument };
  listOperations: { taskId: string };
  resolveOperation: { taskId: string; operationId: string; resolution: 'succeeded' | 'failed' };
  setConversationPolicy: { conversationId: string; expectedRevision: number; targets: string[] };
}
export type MemberExecutionAction = keyof MemberExecutionPayloads;
export type MemberExecutionCommand = { [A in MemberExecutionAction]: ExecutionScope & {
  schemaVersion: 1; commandId: string; action: A; payload: MemberExecutionPayloads[A];
} }[MemberExecutionAction];
export function executionCommand<A extends MemberExecutionAction>(scope: ExecutionScope, action: A,
  payload: MemberExecutionPayloads[A], commandId: string = crypto.randomUUID()): Extract<MemberExecutionCommand, { action: A }> {
  const command = { schemaVersion: 1, commandId, ...scope, action, payload };
  if (!isMemberExecutionCommand(command)) throw new Error('INVALID_INPUT');
  return command as unknown as Extract<MemberExecutionCommand, { action: A }>;
}
export interface LocalPublishableBot { instanceId: string; name: string; description: string; model?: string; requiredResources?: string[] }
export interface SelectedSharedFile { token: string; kind: 'file' | 'skill'; name: string; sha256: string; bytes: number; mediaType: string }
export interface PublishLocalBotInput extends ExecutionScope {
  publicationId: string; sharedBotId: string; instanceId: string; expectedRevision: number;
  role: string; usePolicy: UsePolicy; fileTokens: string[]; resourceBindings?: Record<string, string>; quotas: RequestQuotas;
}
export const sharedBotUsable = (card: SharedBotCard): boolean => card.state === 'published' && card.permitted;
