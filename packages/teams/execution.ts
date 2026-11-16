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
  /** The Bot keeps a list its members and its runs change together. With `approval`, a run only proposes a change and the member who asked decides. */
  sharedList?: { title: string; approval?: 'requester' };
  /** The maintainer offered the Bot to another member and it is not settled yet. Only those two are told. */
  transfer?: BotTransfer;
}
/** A Bot being handed to another member: `offered` until that member accepts, `accepted` while their computer takes it over. It ends by itself at `expiresAt`. */
export interface BotTransfer { transferId: string; from: MemberRef; to: MemberRef; state: 'offered' | 'accepted'; expiresAt: number }
export type TaskState = 'queued' | 'running' | 'waiting' | 'completed' | 'failed' | 'expired' | 'interrupted' | 'authorization_required' | 'revoked' | 'cancelled' | 'needs_reconciliation';
export type TaskUsage = { status: 'missing' } | { status: 'partial' | 'complete'; inputTokens: number; outputTokens: number; costMicros?: number; currency?: string };
/** What a waiting task is blocked on: a question to `recipient`, or (`tool_approval`) a list change only they can approve. Present only while `state` is `waiting`. */
export interface TaskWait { waitId: string; kind: 'human_input' | 'tool_approval'; recipient: MemberRef; expiresAt: number; messageId: string }
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
/** A Bot's shared list. Every write names the `dataRevision` it was made from and is refused once the list has moved on. */
export interface SharedListItem { itemId: string; text: string; done: boolean }
export type SharedListChange = { op: 'add'; text: string } | { op: 'update'; itemId: string; text?: string; done?: boolean } | { op: 'remove'; itemId: string };
export interface SharedListWrite { expectedDataRevision: number; changes: SharedListChange[] }
export interface SharedList { sharedBotId: string; title: string; description: string; dataRevision: number; items: SharedListItem[]; updatedAt: number }
export interface SharedListWritten { sharedBotId: string; dataRevision: number }
/** Who made a revision of a list: a member, or the Bot while it worked on a member's request. */
export type SharedListAuthor = { kind: 'user'; uid: string; generation: number }
  | { kind: 'shared_bot'; sharedBotId: string; taskId: string; requester: { uid: string; generation: number } };
/** One change of a revision, with the item as it was before for a change or removal. */
export type SharedListLoggedChange = { op: 'add'; itemId: string; text: string }
  | { op: 'update'; itemId: string; text?: string; done?: boolean; before: { text: string; done: boolean } }
  | { op: 'remove'; itemId: string; before: { text: string; done: boolean } };
export interface SharedListRevision { dataRevision: number; author: SharedListAuthor; changes: SharedListLoggedChange[]; createdAt: number }
/** A page of a list's history, newest first. The service keeps the latest 200 revisions. */
export interface SharedListHistory { sharedBotId: string; dataRevision: number; hasMore: boolean; revisions: SharedListRevision[] }
export interface ExecutionScope { teamId: string; generation: number }
export interface MemberExecutionPayloads {
  listSharedBots: Record<string, never>;
  createConversation: { conversationId: string; kind: 'bot_dm' | 'group' | 'channel'; title: string; participantUids: string[]; sharedBotIds: string[] };
  /** A message that asks Bots, carries files (`attachments`: checked uploads of this sender for this conversation), or both. */
  submit: { conversationId: string; messageId: string; changeId: string; text: string; targets: string[]; replyTo?: string; attachments?: string[] };
  createUpload: { conversationId: string; uploadId: string; name: string; mime: 'image/png' | 'image/jpeg' | 'application/pdf' | 'text/plain'; byteLength: number; sha256: string };
  putUploadPart: { uploadId: string; index: number; data: string };
  finalizeUpload: { uploadId: string };
  /** Take a file out of an unsent message. */
  cancelUpload: { uploadId: string };
  readAttachmentPart: { attachmentId: string; index: number };
  /** The sender takes a file back out of a sent message. */
  revokeAttachment: { attachmentId: string; changeId: string };
  pauseSharedBot: { sharedBotId: string; expectedRevision: number };
  answerWait: { conversationId: string; waitId: string; messageId: string; changeId: string; text: string };
  decideWait: { conversationId: string; waitId: string; messageId: string; changeId: string; decision: 'approve' | 'deny' };
  cancelTask: { taskId: string };
  getTask: { taskId: string };
  listTasks: { conversationId?: string; after?: string };
  readArtifact: { artifactId: string; revision?: number };
  listArtifactRevisions: { artifactId: string };
  reviseArtifact: { artifactId: string; expectedRevision: number; operationId: string; document: TeamDocument };
  listOperations: { taskId: string };
  resolveOperation: { taskId: string; operationId: string; resolution: 'succeeded' | 'failed' };
  readList: { sharedBotId: string };
  writeList: SharedListWrite & { sharedBotId: string; operationId: string };
  listHistory: { sharedBotId: string; beforeRevision?: number };
  setConversationPolicy: { conversationId: string; expectedRevision: number; targets: string[] };
  /** The maintainer takes an offered Bot back, or the member it was offered to declines it. */
  endTransfer: { transferId: string };
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
/**
 * How one Bot is served from the computer that publishes it. `checking` until a pass has an answer; `not_ready` names what its maintainer
 * repairs (`bot`: the local Bot is gone, `runtime`: this app cannot run it, `model`: its model account needs attention); `unreachable`: the
 * computer cannot reach the service; `stopped`: paused, published again elsewhere, or its team was left.
 */
export interface SharedHostBot { teamId: string; sharedBotId: string; state: 'checking' | 'ready' | 'not_ready' | 'unreachable' | 'stopped'; reason?: 'bot' | 'runtime' | 'model'; /** Requests being answered now. */ running: number }
/** A Desktop as the host of the Bots it publishes, and what keeps it serving while its owner is away. The system owns `openAtLogin`. */
export interface SharedHostStatus { bots: SharedHostBot[]; openAtLogin: { available: boolean; enabled: boolean; needsApproval: boolean }; keepAwake: boolean; onBattery: boolean }
/** One read-only file or Skill a Bot is published with. Its text stays on the maintainer's Desktop. */
export interface PublishedKnowledgeDocument { resourceId: string; revision: number; kind: 'file' | 'skill'; name: string; sha256: string; bytes: number }
/** What a Bot is published with, as its maintainer's Desktop holds it: the role and the shared files of publication `revision`. */
export interface PublishedKnowledge { sharedBotId: string; revision: number; role: string; documents: PublishedKnowledgeDocument[] }
/** Offer a Bot this Desktop publishes to another member of its team. The published role and files go with it; the local Bot, its model and its sessions do not. */
export interface OfferSharedBotInput extends ExecutionScope { transferId: string; sharedBotId: string; expectedRevision: number; to: MemberRef }
/** Take over an offered Bot on this Desktop: `instanceId` is the local Bot, with its own model, that answers for it from now on. */
export interface AcceptSharedBotInput extends ExecutionScope { transferId: string; instanceId: string }
export interface PublishLocalBotInput extends ExecutionScope {
  publicationId: string; sharedBotId: string; instanceId: string; expectedRevision: number;
  role: string; usePolicy: UsePolicy; fileTokens: string[]; resourceBindings?: Record<string, string>; quotas: RequestQuotas;
  /** Files of the Bot's current publication that stay shared unchanged, by resource id. A current file that is neither kept nor replaced stops being shared. */
  keptResources?: string[];
  /** Files of the current publication whose text is replaced: resource id to the token of the newly selected file. The resource keeps its id and gets its next revision. */
  replacedResources?: Record<string, string>;
  /** Give the Bot a shared list: its title, and what the Bot is told the list is for. With `approval: 'requester'` the Bot asks the member it works for before each change. */
  sharedList?: { title: string; description: string; approval?: 'none' | 'requester' };
}
export const sharedBotUsable = (card: SharedBotCard): boolean => card.state === 'published' && card.permitted;
