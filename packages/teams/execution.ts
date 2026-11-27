import type { MessageFormat } from './message-format.js';
import { HUMAN_MENTION_MODES, isMemberExecutionCommand } from './execution-validation.js';
export { HUMAN_MENTION_MODES, MAX_REPLIES_LIMIT } from './execution-validation.js';
import type { BotScopeNarrowing, PublishedBotScope } from './bot-scope.js';
export type { BotScopeEntry, BotScopeNarrowing, PublishedBotScope } from './bot-scope.js';
/** Execution 2 contracts. No retired Bot keys or execution request adapters. */
export interface MemberRef { uid: string; generation: number }
export type UsePolicy = { mode: 'all' } | { mode: 'listed'; members: MemberRef[] };
export interface RequestQuotas { team: number; member: number; bot: number }
export interface SharedBotCard {
  sharedBotId: string; name: string; description: string; maintainer: MemberRef;
  revision: number; state: 'draft' | 'published' | 'paused' | 'retired';
  permitted: boolean; ready: boolean; onlineUntil: number; usePolicy: UsePolicy;
  quotas: RequestQuotas; instanceId?: string;
  /** Up to three prompts pinned to this publication; selecting one only fills the member's draft. */
  examples?: string[];
  /** Reported by the active deployment; absence means not yet verified. `delegation`: its Desktop runs `TeamDelegate`, so other Bots may ask it for part of a request. */
  capabilities?: { durableWait: boolean; writeOperations: boolean; delegation?: boolean };
  availability?: 'ready' | 'paused' | 'offline' | 'maintenance' | 'quota_exhausted' | 'unverified';
  /** Refresh this projected status at this server-computed UTC millisecond deadline, even without a signal. */
  statusExpiresAt?: number;
  /** Remaining requests for this member, bounded by team, membership and Bot UTC-day counters. */
  dailyRemaining?: number;
  /** Next UTC midnight, in milliseconds. Admission remains transactional. */
  quotaResetsAt?: number;
  /** The Bot keeps a list its members and its runs change together. With `approval`, a run only proposes a change and the member who asked decides. */
  sharedList?: { title: string; approval?: 'requester' };
  /** The Bot may answer messages nobody asked it about, in Chats that turn this on, at most `daily` times a day. */
  autoReply?: { daily: number };
  /** Whether other Bots of a Chat may hand this Bot part of a request (Bot v3 B8): any Bot of the Team (`team_bots`) or only those in `acceptFrom`, by shared Bot id. Absent: none. */
  acceptDelegation?: 'none' | 'team_bots' | 'listed'; acceptFrom?: string[];
  /** The Bots this Bot may hand part of a request to, and the most such requests it makes a day. Absent: it asks nobody. */
  delegateTo?: string[]; delegation?: { daily: number };
  /** What the maintainer published of their Desktop for this Bot to use in the Team; absent when nothing. A Chat can narrow it (`TeamConversation.botScopes`). */
  scope?: PublishedBotScope;
  /** The maintainer offered the Bot to another member and it is not settled yet. Only those two are told. */
  transfer?: BotTransfer;
}
/** A Bot being handed to another member: `offered` until that member accepts, `accepted` while their computer takes it over. It ends by itself at `expiresAt`. */
export interface BotTransfer { transferId: string; from: MemberRef; to: MemberRef; state: 'offered' | 'accepted'; expiresAt: number }
/**
 * `declined`: a Bot nobody asked chose not to answer, or its reply was held for approval and never posted; nothing was posted. `awaiting_approval`: the Bot's
 * reply is made and its maintainer has not yet decided whether it is posted (see `HeldReply`); it ends `completed` when they approve and `declined` otherwise.
 */
export type TaskState = 'queued' | 'running' | 'waiting' | 'completed' | 'failed' | 'expired' | 'interrupted' | 'authorization_required' | 'revoked' | 'cancelled' | 'needs_reconciliation' | 'declined' | 'awaiting_approval';
export type TaskUsage = { status: 'missing' } | { status: 'partial' | 'complete'; inputTokens: number; outputTokens: number; costMicros?: number; currency?: string };
/** One Bot of a chain of delegations: its shared id and the name it had when the delegation was made. */
export interface DelegationLink { sharedBotId: string; name: string }
/**
 * A task another Bot asked for (Bot v3 B8): `chain` runs from the Bot the person asked to the Bot that runs this task, and `parentBotId` is the Bot that asked for it.
 * With `expectReply` the Bot that asked waits for the answer instead of the Chat getting it.
 */
export interface TaskDelegation { delegationId: string; rootTaskId: string; parentTaskId: string; parentBotId: string; depth: number; chain: DelegationLink[]; expectReply: boolean }
/** What a task that waits for another Bot's reply waits for: the Bot and the task it asked. */
export interface WaitDelegation { sharedBotId: string; taskId: string }
/**
 * What a waiting task is blocked on: a question to `recipient`, a list change only they can approve (`tool_approval`), or (`delegation`) the reply of another Bot, which
 * nobody types: `recipient` is the person who asked, `delegation` names the Bot waited for. Present only while `state` is `waiting`.
 */
export interface TaskWait { waitId: string; kind: 'human_input' | 'tool_approval' | 'delegation'; recipient: MemberRef; expiresAt: number; messageId: string; delegation?: WaitDelegation }
export interface TeamTaskView {
  taskId: string; conversationId: string; sharedBotId: string; requester: MemberRef;
  /** `deadline` ends the current phase: pick-up limit while queued, running budget while running, wait expiry while waiting, the maintainer's time to decide while awaiting approval. */
  state: TaskState; createdAt: number; deadline: number; epoch: number; activeSpentMs: number; cancelRequestedAt?: number;
  /** Outside actions whose outcome nobody has confirmed yet; absent when there are none. */
  unverifiedOperations?: number;
  wait?: TaskWait; replyId?: string; artifactId?: string; usage?: TaskUsage;
  /** Nobody asked the Bot: it was given the message because auto-reply is on, and may leave it unanswered. */
  optional?: true;
  /** Present only while `queued`: the message was sent while the Bot's Desktop was off, so the task waits up to a day for it to come back instead of five minutes. */
  hostWait?: true;
  /** Another Bot asked for this task (Bot v3 B8). */
  delegation?: TaskDelegation;
}
/**
 * What a Bot's host decided about a message nobody asked the Bot about, which step of its judgement decided it, and why. Codes only: the
 * Bot's maintainer reads them in the auto-reply log, and nothing the model said is kept. `quiet` and `cooldown` are the maintainer's quiet
 * hours and cooldown, `rule` a keyword or a call to the Bot, `decision` the Decision service against the Bot's thresholds, `fallback` no verdict.
 */
export const TRIAGE_ACTIONS = ['decline', 'run', 'run_may_decline'] as const;
export const TRIAGE_SOURCES = ['quiet', 'cooldown', 'rule', 'decision', 'fallback'] as const;
export const TRIAGE_REASONS = ['quiet_hours', 'cooldown', 'keyword', 'addressed', 'below_threshold', 'between_thresholds', 'above_threshold', 'decision_unavailable', 'message_unavailable'] as const;
export type TriageAction = typeof TRIAGE_ACTIONS[number];
export type TriageSource = typeof TRIAGE_SOURCES[number];
export type TriageReason = typeof TRIAGE_REASONS[number];
export interface TriageVerdict {
  action: TriageAction; source: TriageSource; reason?: TriageReason;
  /** What the Decision service gave, from 0 to 1 and rounded to three decimals; only with `source: 'decision'`. */
  probability?: number;
}
/** A verdict as the service keeps it on the task; `at` is the service's clock. */
export interface TaskTriage extends TriageVerdict { at: number }
/**
 * What became of a reply the Bot's maintainer was asked to approve before it was posted: `held` is the reply still waiting, `approved` was posted,
 * `rejected` was dropped by the maintainer, `timeout` was dropped when the ten minutes ran out and `superseded` when someone wrote in the Chat again.
 * Codes only, like a verdict. The service's `approvalOutcomes` (`shared-bot-commands.mjs`) is the same list.
 */
export const APPROVAL_OUTCOMES = ['held', 'approved', 'rejected', 'timeout', 'superseded'] as const;
export type ApprovalOutcome = typeof APPROVAL_OUTCOMES[number];
/**
 * What a Bot's host bids for a message of a reply round (B7), best first, and what became of a task in a round. `addressed` is the service's own tier (a person
 * spoke to the Bot: a reply to it, or its name), which a host only sends for a task the service marked so; `decline` is an abstention. The service's `bidTiers` and
 * `roundOutcomes` (`shared-bot-commands.mjs`) are the same lists.
 */
export const BID_TIERS = ['addressed', 'rule', 'run', 'may_decline', 'decline'] as const;
export const ROUND_OUTCOMES = ['won', 'lost', 'abstained', 'promoted', 'superseded'] as const;
export type BidTier = typeof BID_TIERS[number];
export type RoundOutcome = typeof ROUND_OUTCOMES[number];
/** What became of one Bot's task in the round of its message, for its maintainer's log: its own tier is shown, other Bots' bids never are. No `tier` when its Desktop did not bid. */
export interface TaskRound { outcome: RoundOutcome; tier?: BidTier }
export type HumanMentionMode = typeof HUMAN_MENTION_MODES[number];
/** One message the Bot's host judged, as its maintainer reads the log. `state` is where the task stands now: it settles after the verdict. */
export interface TriageLogItem {
  taskId: string; conversationId: string; messageId: string; createdAt: number; triage: TaskTriage; state: TaskState;
  conversationTitle?: string;
  /** Only when the Bot's reply was held for approval. */
  approval?: ApprovalOutcome;
  /** Only when the message was a round of several Bots and it has been settled. */
  round?: TaskRound;
  /** Only when another Bot asked for the task (Bot v3 B8): the message was no one's judgement, a Bot passed it on. */
  delegation?: TaskDelegation;
}
/** A page of the log, newest verdict first; pass `nextCursor` back for the page after. */
export interface TriageLogPage { items: TriageLogItem[]; nextCursor?: string }
/** The log of one Bot published to one Team, optionally of one Chat. At most 50 rows a page. */
export interface TriageLogRequest { teamId: string; sharedBotId: string; conversationId?: string; cursor?: string; limit?: number }
/** A Team one of this Desktop's Bots is published to: where its auto-reply log is read. */
export interface TriageTarget { teamId: string; teamName: string; sharedBotId: string }
export interface TeamDocument { title: string; body: string }
/**
 * A reply of one of your Bots that waits for you to let it go, exactly as the Bot made it: what its maintainer reads before anyone else does. The service keeps
 * it where only the maintainer can read it, and drops it at `expiresAt` (ten minutes after `heldAt`) or when someone writes in the Chat again, whichever is first.
 */
export interface HeldReply {
  taskId: string; conversationId: string; messageId: string; requester: MemberRef;
  /** Nobody asked the Bot: it was given the message because auto-reply is on. Only such a reply is dropped by a newer message. */
  unasked: boolean;
  heldAt: number; expiresAt: number;
  text: string; document?: TeamDocument; conversationTitle?: string;
}
/** The held replies of one Bot, newest first. */
export interface HeldReplies { replies: HeldReply[] }
/**
 * What a decision on a held reply came to. Approving posts it (`completed`, with the `replyId` of the message and the `artifactId` of the document when it has one);
 * rejecting drops it (`declined`, `approval_rejected`). A reply that may not be posted any more (the message it answers was edited, its requester left) is dropped
 * too: `declined`, `revoked`.
 */
export interface HeldReplyDecision { taskId: string; state: TaskState; declineReason?: 'approval_rejected' | 'revoked'; replyId?: string | null; artifactId?: string | null }
/** The Desktop worker uses this title for an automatic copy of the result. */
export function isAutomaticReplyCopy(document: TeamDocument, reply: string): boolean {
  return document.title === 'Shared Bot result' && document.body === reply;
}
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
  submit: { channelMode?: 'chat' | 'posts'; conversationId: string; messageId: string; changeId: string; text: string; targets: string[]; replyTo?: string; attachments?: string[]; privateDataSource?: string; mentionEntities?: import('./mentions.js').MentionEntity[]; formats?: MessageFormat[] };
  createUpload: { conversationId: string; uploadId: string; name: string; mime: 'image/png' | 'image/jpeg' | 'application/pdf' | 'text/plain'; byteLength: number; sha256: string };
  putUploadPart: { uploadId: string; index: number; data: string };
  finalizeUpload: { uploadId: string };
  /** Take a file out of an unsent message. */
  cancelUpload: { uploadId: string };
  readAttachmentPart: { attachmentId: string; index: number };
  /** The sender takes a file back out of a sent message. */
  revokeAttachment: { attachmentId: string; changeId: string };
  pauseSharedBot: { sharedBotId: string; expectedRevision: number };
  answerWait: { conversationId: string; waitId: string; messageId: string; changeId: string; text: string; formats?: MessageFormat[] };
  decideWait: { conversationId: string; waitId: string; messageId: string; changeId: string; decision: 'approve' | 'deny' };
  cancelTask: { taskId: string };
  getTask: { taskId: string };
  listTasks: { conversationId?: string; after?: string };
  /** The auto-reply log of one Bot, read by its maintainer: newest verdict first, optionally of one Chat; `cursor` is the `nextCursor` of the page before, `limit` at most 50. Anyone else is refused (`MAINTAINER_REQUIRED`). */
  listTriage: { sharedBotId: string; conversationId?: string; cursor?: string; limit?: number };
  /** The replies of one Bot that wait for you, optionally of one Chat: the drafts themselves, which only the Bot's maintainer is shown (`MAINTAINER_REQUIRED` for anyone else). */
  listHeldReplies: { sharedBotId: string; conversationId?: string };
  /** The maintainer lets a held reply go: as it is, or with `text` as they changed it to (trimmed, at most 8 KiB). The service posts it, so it works while the Bot's Desktop is off; a reply that is no longer held is refused (`REPLY_APPROVED`, `REPLY_REJECTED`, `REPLY_EXPIRED`, `REPLY_SUPERSEDED`). */
  approveReply: { taskId: string; text?: string };
  /** The maintainer drops a held reply: nothing is posted. Refused like an approval when the reply is no longer held. */
  rejectReply: { taskId: string };
  readArtifact: { artifactId: string; revision?: number };
  listArtifactRevisions: { artifactId: string };
  reviseArtifact: { artifactId: string; expectedRevision: number; operationId: string; document: TeamDocument };
  listOperations: { taskId: string };
  resolveOperation: { taskId: string; operationId: string; resolution: 'succeeded' | 'failed' };
  readList: { sharedBotId: string };
  writeList: SharedListWrite & { sharedBotId: string; operationId: string };
  listHistory: { sharedBotId: string; beforeRevision?: number };
  setConversationPolicy: { conversationId: string; expectedRevision: number; targets: string[] };
  /** The Bots of this Chat that judge for themselves whether to answer its messages: each must be in the Chat and published to do so. */
  setConversationAutoReply: { conversationId: string; expectedRevision: number; targets: string[]; maxReplies?: number; humanMentions?: HumanMentionMode };
  /** What one Bot of this Chat may use here, within what it published; `null` lifts the narrowing. `expectedRevision` is the revision of that Bot's entry in `TeamConversation.botScopes` (0 when it has none). */
  setConversationBotScope: { conversationId: string; sharedBotId: string; expectedRevision: number; scope: BotScopeNarrowing | null };
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
export interface LocalPublishableBot {
  instanceId: string; name: string; description: string; model?: string; requiredResources?: string[]; examples?: string[]; capabilities?: { durableWait: boolean; writeOperations: boolean };
  /** The most of this Desktop the Bot may be published with: its own scope, with the Local Apps that are installed here named. Absent when it has none. */
  scope?: PublishedBotScope;
}
export interface SelectedSharedFile { token: string; kind: 'file' | 'skill'; name: string; sha256: string; bytes: number; mediaType: string }
/**
 * How one Bot is served from the computer that publishes it. `checking` until a pass has an answer; `not_ready` names what its maintainer
 * repairs (`bot`: the local Bot is gone, `runtime`: this app cannot run it, `model`: its model account needs attention); `unreachable`: the
 * computer cannot reach the service; `stopped`: paused, published again elsewhere, or its team was left.
 */
export interface SharedHostBot { teamId: string; sharedBotId: string; state: 'checking' | 'ready' | 'not_ready' | 'unreachable' | 'stopped'; reason?: 'bot' | 'runtime' | 'model'; /** Requests being answered now. */ running: number }
/** A Desktop as the host of the Bots it publishes, and what keeps it serving while its owner is away. The system owns `openAtLogin`. */
export interface SharedHostStatus { bots: SharedHostBot[]; openAtLogin: { available: boolean; enabled: boolean; needsApproval: boolean }; keepAwake: boolean; onBattery: boolean;
  /** A Decision service is set up on this Desktop: its Bots can be published with auto-reply. */
  decisionConfigured: boolean;
  /** This computer can isolate the Bots it publishes (macOS, or Linux with bubblewrap installed); where it cannot, none is published or served from it. */
  isolationAvailable: boolean }
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
  examples?: string[];
  /** Files of the Bot's current publication that stay shared unchanged, by resource id. A current file that is neither kept nor replaced stops being shared. */
  keptResources?: string[];
  /** Files of the current publication whose text is replaced: resource id to the token of the newly selected file. The resource keeps its id and gets its next revision. */
  replacedResources?: Record<string, string>;
  /** Give the Bot a shared list: its title, and what the Bot is told the list is for. With `approval: 'requester'` the Bot asks the member it works for before each change. */
  sharedList?: { title: string; description: string; approval?: 'none' | 'requester' };
  /** Let the Bot answer messages nobody asked it about, at most `daily` runs a day. Needs a Decision service on this Desktop. */
  autoReply?: { daily: number };
  /** Whether other Bots may hand this Bot part of a request: `listed` needs `acceptFrom`, the shared ids of those Bots, and only it takes one. Left out, none may. */
  acceptDelegation?: 'none' | 'team_bots' | 'listed'; acceptFrom?: string[];
  /** The Bots this Bot may hand part of a request to (shared ids, not itself) and the most such requests it makes a day. The two go together. Left out, it asks nobody. */
  delegateTo?: string[]; delegation?: { daily: number };
  /** The Local Apps and datasets of this Desktop the Bot may use in the Team: no more than the local Bot's own scope allows. Leaving it out, or publishing again without it, shares none. */
  scope?: PublishedBotScope;
}
export const sharedBotUsable = (card: SharedBotCard): boolean => card.state === 'published' && card.permitted;
