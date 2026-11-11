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
export type TaskState = 'queued' | 'running' | 'waiting' | 'completed' | 'failed' | 'expired' | 'interrupted' | 'authorization_required' | 'revoked' | 'cancelled';
export type TaskUsage = { status: 'missing' } | { status: 'partial' | 'complete'; inputTokens: number; outputTokens: number; costMicros?: number; currency?: string };
/** The human question a waiting task is blocked on; present only while `state` is `waiting`. */
export interface TaskWait { waitId: string; kind: 'human_input'; recipient: MemberRef; expiresAt: number; messageId: string }
export interface TeamTaskView {
  taskId: string; conversationId: string; sharedBotId: string; requester: MemberRef;
  /** `deadline` ends the current phase: pick-up limit while queued, running budget while running, wait expiry while waiting. */
  state: TaskState; createdAt: number; deadline: number; epoch: number; activeSpentMs: number; cancelRequestedAt?: number;
  wait?: TaskWait; replyId?: string; artifactId?: string; usage?: TaskUsage;
}
export interface TeamArtifact { artifactId: string; revision: number; digest: string; document: { title: string; body: string } }
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
  readArtifact: { artifactId: string };
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
