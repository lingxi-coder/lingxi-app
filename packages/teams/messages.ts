import type { ChannelCommand, ChannelPosting } from './channel-settings.js';
import type { MentionEntity } from './mentions.js';
import type { ResourceRef } from './dataset-resource.js';
import { parseDataRef, type DataRef } from './lingxi-data.js';
import type { TeamVisualization } from './visualization.js';
import { isMemberExecutionCommand } from './execution-validation.js';
import { executionCommand, type HumanMentionMode, type MemberExecutionCommand } from './execution.js';
import { ATTACHMENT_MESSAGE_FILES, type MessageAttachment } from './attachments.js';
import { sameBotScopes, type BotScopeEntry } from './bot-scope.js';
export type MessageAuthor = { kind: 'user'; uid: string; displayName?: string } | { kind: 'shared_bot'; sharedBotId: string; displayName: string }
  | { kind: 'task'; taskId: string; logicalRun: string; displayName: string };
export function authorIdentity(author: MessageAuthor): string {
  switch (author.kind) {
    case 'user': return 'user:' + author.uid;
    case 'shared_bot': return 'shared_bot:' + author.sharedBotId;
    case 'task': return 'task:' + author.taskId;
  }
}
export interface TeamConversation {
  id: string; kind: 'dm' | 'group' | 'channel' | 'bot_dm'; title: string; participants: string[];
  /** Who can read it, as written when it was made: its people and the maintainers its Bots had then. Absent when the source does not say. */
  readers?: string[];
  description?: string;
  posting?: ChannelPosting;
  channelAdmins?: Record<string, number>;
  unreadCount?: number;
  manualUnreadRevision?:number;
  pendingActionRevision?:number;
  failureRevision?:number;
  lastSeq: number; lastChangeSeq: number; lastReadSeq: number;
  /** Newest message for conversation lists. Absent until the conversation has a message; then always the full pointer. */
  lastMessage?: { id: string; seq: number; at: string | null; author: MessageAuthor; preview: string; recalled?: true };
  /** The Bots that judge for themselves whether to answer this Chat's messages. Absent until a member first sets it. `maxReplies` and `humanMentions` are absent until set: 1 Bot answers a message nobody asked them about, and a message that mentions only people is for them (`quiet`). */
  autoReply?: { revision: number; targets: string[]; maxReplies?: number; humanMentions?: HumanMentionMode };
  /** What this Chat narrowed each of its Bots to use here, by shared Bot id, within what the Bot published (`effectiveBotScope`). A Bot with no entry is not narrowed. */
  botScopes?: Record<string, BotScopeEntry>;
}
/** One change a Bot asks to make to its list. `item` is the text of the item it touches as it read when the Bot asked, `null` if it was gone. */
export interface ListApprovalChange { op: 'add' | 'update' | 'remove'; text?: string; itemId?: string; done?: boolean; item?: string | null }
export interface ListApproval { title: string; changes: ListApprovalChange[] }
export interface PostStats { replies: number; likes: number; views: number; revision: number }
export interface PostInteraction { stats: PostStats; liked: boolean }
export interface TeamMessage {
  id: string; seq: number; author: MessageAuthor;
  channelPost?: true; postId?: string; postStats?: PostStats;
  kind: string; text: string; replyTo?: string; createdAt: string | null; editedAt?: string | null;
  recalled?: boolean; changeSeq: number; changeId: string;
  taskId?: string; taskIds?: string[]; artifactId?: string;
  /** Set on a Bot's question and on the human answer to it. */
  waitId?: string;
  /** On a Bot's request to approve a list change: the list and exactly the changes the approval is for. */
  approval?: ListApproval;
  /** On the requester's reply to such a request. */
  decision?: 'approve' | 'deny';
  mentions?: string[];
  mentionEntities?: MentionEntity[];
  /** Files the message carries, in the order they were added. */
  attachments?: MessageAttachment[];
  dataRef?: DataRef;
  resourceRef?: ResourceRef;
  resourceGrant?: string;
  visualization?: TeamVisualization;
  /** On a Bot's reply: the files of this conversation its model was given, as the service checked them. */
  readFiles?: { attachmentId: string; name: string }[];
}
export interface MessageScope { teamId: string; generation: number }
export type MessageMutation = MessageScope & { action: 'mutate'; conversationId: string; messageId: string; changeId: string;
  operation: 'create' | 'edit' | 'recall'; channelMode?: ChannelPosting['mode']; text: string; replyTo?: string; dataRef?: DataRef; mentionEntities?: MentionEntity[] };
export type MessageCommand = MessageMutation | MemberExecutionCommand | ChannelCommand
  | MessageScope & { action: 'create'; conversationId: string; kind: TeamConversation['kind']; title: string; participantUids: string[];  }
  | MessageScope & { action:'history';conversationId:string;before?:number }
  | MessageScope & { action: 'postHistory'; conversationId: string; postId: string; before?: number }
  | MessageScope & { action: 'postInteraction'; conversationId: string; postId: string; liked?: boolean }
  | MessageScope & { action: 'read'; conversationId: string; lastReadSeq: number };
export interface MessageSignal { bots?: boolean; conversationId?: string }
export interface MessageApi {
  messages<T>(command: MessageCommand): Promise<T>;
  historyPage?(scope: MessageScope, conversationId: string, beforeSeq:number):Promise<TeamMessage[]>;
  peerReceipt?(scope:MessageScope,id:string):Promise<{readThrough:number|null}>;
  readVisible?(scope:MessageScope,conversationId:string,through:number,manualUnreadRevision:number,observedPendingActionRevision?:number,observedFailureRevision?:number):Promise<unknown>;
  onMessagesChanged(callback: (signal?: MessageSignal) => void): () => void;
  watchMessages(scope: MessageScope, conversationId: string, next: (snapshot: MessageSnapshot) => void, postId?: string): () => void;
  prefetchMessages?(scope: MessageScope, conversationId: string): () => void;
}
export type MessageSnapshot = MessageScope & { uid: string; fromCache: boolean } & (
  | { kind: 'postStats'; conversationId: string; postId: string; stats?: PostStats; reaction?: { liked: boolean; revision: number } }
  | { kind: 'directory'; conversations: TeamConversation[] }
  | { kind: 'messages'; conversationId: string; messages: TeamMessage[]; removed?: string[]; replace?: boolean; older?:boolean; rebase?:boolean; updatedIds?:string[] }
  | { kind: 'read'; conversationId: string; lastReadSeq: number; unreadCount?:number; manualUnreadRevision?:number;pendingActionRevision?:number;failureRevision?:number }
  | { kind: 'signal'; signal?: MessageSignal }
  | { kind: 'error'; error: string }
);
interface CommandMetadata { error?: string; leaseUntil?: number; attempted?: boolean; retryDelay?: number; enqueuedAt?: string }
type CreationCommand = Extract<MessageCommand, { action: 'create' | 'createConversation' }>;
type OutboxCommand = MessageMutation | Extract<MemberExecutionCommand, { action: 'submit' | 'answerWait' }>;
export interface PendingCommand<C> extends CommandMetadata { command: C }
export interface PendingCommands {
  version: 2; uid: string; teamId: string; generation: number;
  creation?: PendingCommand<CreationCommand>;
  outbox: PendingCommand<OutboxCommand>[];
}
export type VisiblePendingMessage = PendingCommand<OutboxCommand> & { acknowledged?: true };
export function pendingPayload<C extends MessageCommand>(value: PendingCommand<C>): C extends { payload: infer P } ? P : C {
  return ('payload' in value.command ? value.command.payload : value.command) as C extends { payload: infer P } ? P : C;
}
export const createsMessage = (intent: PendingCommands['outbox'][number]) =>
  intent.command.action !== 'mutate' || intent.command.operation === 'create';
const EMPTY_PENDING: VisiblePendingMessage[] = [];
/** The projection is ephemeral. Firestore owns all durable message/read caches. */
export interface MessageProjection extends PendingCommands {
  conversations: TeamConversation[]; threads: Record<string, { messages: TeamMessage[] }>;
}
export interface CommandStorage {
  onChanged?(key: string, callback: () => void): () => void;
  load(key: string): Promise<PendingCommands | undefined>;
  /** Only command identities/payloads commit atomically, including across tabs. */
  update(key: string, reduce: (old: PendingCommands | undefined) => PendingCommands): Promise<PendingCommands>;
}
export function pendingCommands(value: PendingCommands): PendingCommands {
  const { version, uid, teamId, generation, creation, outbox } = value;
  return { version, uid, teamId, generation, creation, outbox };
}
export const projectionKey = (uid: string, scope: MessageScope) => JSON.stringify([uid, scope.teamId, scope.generation]);
export const emptyProjection = (uid: string, scope: MessageScope): MessageProjection => ({ version: 2, uid, ...scope, conversations: [], threads: {}, outbox: [] });
const sameLastMessage = (a: TeamConversation['lastMessage'], b: TeamConversation['lastMessage']) => a === b || (!!a && !!b && a.id === b.id && a.seq === b.seq && a.at === b.at
  && a.preview === b.preview && a.recalled === b.recalled && authorIdentity(a.author) === authorIdentity(b.author));
export function mergeConversation(old: TeamConversation | undefined, value: TeamConversation): TeamConversation {
  // Like the cursors, never step back to an older message; the same seq is newer when its edit/recall changed the preview.
  const lastMessage = old?.lastMessage && (!value.lastMessage || old.lastMessage.seq > value.lastMessage.seq) ? old.lastMessage : value.lastMessage;
  const merged = { ...value, lastSeq: Math.max(old?.lastSeq ?? 0, value.lastSeq), lastChangeSeq: Math.max(old?.lastChangeSeq ?? 0, value.lastChangeSeq), lastReadSeq: Math.max(old?.lastReadSeq ?? 0, value.lastReadSeq),
    ...(lastMessage ? { lastMessage } : {}) };
  if (old && old.kind === merged.kind && old.title === merged.title
    && old.unreadCount === merged.unreadCount && old.manualUnreadRevision === merged.manualUnreadRevision && old.pendingActionRevision === merged.pendingActionRevision && old.failureRevision===merged.failureRevision && old.lastSeq === merged.lastSeq && old.lastChangeSeq === merged.lastChangeSeq && old.lastReadSeq === merged.lastReadSeq
    && sameLastMessage(old.lastMessage, merged.lastMessage)
    && old.participants.length === merged.participants.length && old.participants.every((value, index) => value === merged.participants[index])
    && (old.readers ?? []).join('\n') === (merged.readers ?? []).join('\n')
    && old.autoReply?.revision === merged.autoReply?.revision && (old.autoReply?.targets ?? []).join('\n') === (merged.autoReply?.targets ?? []).join('\n')
    && old.autoReply?.maxReplies === merged.autoReply?.maxReplies && old.autoReply?.humanMentions === merged.autoReply?.humanMentions
    && old.description === merged.description && JSON.stringify(old.posting) === JSON.stringify(merged.posting)
    && JSON.stringify(old.channelAdmins) === JSON.stringify(merged.channelAdmins) && sameBotScopes(old.botScopes, merged.botScopes)) return old;
  return merged;
}
export function mergeDirectory(state: MessageProjection, incoming: TeamConversation[], replace: boolean): MessageProjection {
  const old = new Map(state.conversations.map(value => [value.id, value]));
  const values = replace ? new Map<string, TeamConversation>() : new Map(old);
  for (const value of incoming) values.set(value.id, mergeConversation(old.get(value.id), value));
  const conversations = [...values.values()].sort((a, b) => a.id.localeCompare(b.id, 'en'));
  if (conversations.length === state.conversations.length && conversations.every((value, index) => value === state.conversations[index])) return state;
  return { ...state, conversations };
}
/** Only the visible UI keeps a projection; SDK metadata-only events reuse its rows. */
export function applyMessageSnapshot(state: MessageProjection, snapshot: Extract<MessageSnapshot, { kind: 'messages' }>): MessageProjection {
  const old = state.threads[snapshot.conversationId];
  if (snapshot.replace === false && !snapshot.messages.length && !snapshot.removed?.length && old) return state;
  let messages = snapshot.messages;
  if (snapshot.replace === false) {
    const records = new Map((old?.messages ?? []).map(value => [value.id, value]));
    for (const id of snapshot.removed ?? []) records.delete(id);
    const updates = new Set(snapshot.updatedIds);
    for (const value of snapshot.messages) {
      const prior = records.get(value.id);
      if (updates.has(value.id) && !prior) continue;
      if (!prior || prior.changeSeq <= value.changeSeq) records.set(value.id, value);
    }
    messages = [...records.values()].sort((a, b) => a.seq - b.seq);
  }
  messages = snapshot.older ? messages.slice(0,200) : messages.slice(-200);
  return { ...state, threads: { ...state.threads, [snapshot.conversationId]: { messages } } };
}
function failedCommand<T extends CommandMetadata>(intent: T, error: unknown, permanent: boolean): T {
  if (permanent) return { ...intent, leaseUntil: undefined, error: error instanceof Error ? error.message : 'SERVICE_UNAVAILABLE' };
  const retryDelay = Math.min(30000, Math.max(500, intent.retryDelay ?? 500) * 2);
  return { ...intent, leaseUntil: Date.now() + retryDelay, retryDelay };
}
/** Server verdicts that end an answer to a Bot's question for good: the wait is over, is not the viewer's, or the task lost its authority. */
export const ANSWER_REFUSED = ['WAIT_CLOSED', 'WAIT_RECIPIENT_REQUIRED', 'WAIT_REQUIRED', 'TASK_REVOKED'];
const isRevokedAccessError = (error: unknown) => error instanceof Error && ['MEMBERSHIP_REQUIRED', 'STALE_MEMBERSHIP', 'INVALID_SESSION', 'SIGN_IN_REQUIRED', 'CONVERSATION_REQUIRED'].includes(error.message);
const isPermanentMessageError = (error: unknown) => error instanceof Error && ['MESSAGE_ID_CONFLICT', 'DATA_REFERENCE_UNAVAILABLE', 'DATA_ACCESS_REQUIRED', 'DATA_NOT_FOUND', 'MESSAGE_RECALLED', 'MESSAGE_AUTHOR_REQUIRED', 'REPLY_REQUIRED', 'CHANNEL_POSTING_DENIED', 'CHANNEL_POST_REQUIRED', 'CHANNEL_MODE_CHANGED', 'MENTION_TARGET_REQUIRED', 'INVALID_INPUT', 'BOT_USE_REQUIRED', 'BOT_PARTICIPANT_REQUIRED', 'INVALID_BOT_TARGET', 'HOST_UNAVAILABLE', 'REQUEST_QUOTA_EXCEEDED', 'CONVERSATION_POLICY_REQUIRED', 'INPUT_GRANT_REQUIRED', 'BOT_RETIRED', 'COMMAND_ID_CONFLICT', ...ANSWER_REFUSED].includes(error.message);
const PREFETCH_HOVER_MS = 80;
const PREFETCH_TIMEOUT_MS = 1500;
const PREFETCH_MIN_INTERVAL_MS = 750;
const PREFETCH_COOLDOWN_MS = 30_000;
/** Persist command identities before transport; Firestore owns message history. */
export class TeamMessaging {
  state: MessageProjection;
  selected = '';
  offline = true;
  connecting = true;
  error = '';
  private directoryCached = true;
  private messagesCached = true;
  private closed = false;
  private revoked = false;
  private pageRevision=0;
  private pageBusy=false;
  private historicalWindow=false;
  private running?: Promise<void>;
  private again = false;
  private stops: (() => void)[] = [];
  private callbacks = new Set<() => void>();
  private key: string;
  private retry?: ReturnType<typeof setTimeout>;
  private retryDelay = 1000;
  private stopWatch?: () => void;
  private watchVersion = 0;
  private prefetch?: { id: string; timer: ReturnType<typeof setTimeout>; stop?: () => void };
  private prefetchedAt = new Map<string, number>();
  private nextPrefetchAt = 0;
  private connectionTimeout?: ReturnType<typeof setTimeout>;
  private awaitingEcho: VisiblePendingMessage[] = [];
  private pendingView?: {
    selected: string; messages?: TeamMessage[]; outbox: PendingCommands['outbox'];
    awaitingEcho: VisiblePendingMessage[]; value: VisiblePendingMessage[];
  };
  constructor(private api: MessageApi, private storage: CommandStorage, readonly uid: string, readonly scope: MessageScope) {
    this.state = emptyProjection(uid, scope); this.key = projectionKey(uid, scope);
  }
  onChanged(callback: () => void) { this.callbacks.add(callback); return () => { this.callbacks.delete(callback); }; }
  get visiblePending(): VisiblePendingMessage[] {
    const { selected, awaitingEcho } = this;
    const { outbox, threads } = this.state;
    if (!selected || (!outbox.length && !awaitingEcho.length)) return EMPTY_PENDING;
    const messages = threads[selected]?.messages;
    const previous = this.pendingView;
    if (previous?.selected === selected && previous.messages === messages
      && previous.outbox === outbox && previous.awaitingEcho === awaitingEcho) return previous.value;
    const queued = new Set(outbox.map(intent => pendingPayload(intent).changeId));
    const confirmed = new Set(messages?.filter(row => row.author.kind === 'user' && row.author.uid === this.uid).map(row => row.id));
    const value = [...awaitingEcho.filter(intent => !queued.has(pendingPayload(intent).changeId)), ...outbox].filter(intent => {
      const payload = pendingPayload(intent);
      return payload.conversationId === selected && (intent.error || !createsMessage(intent) || !confirmed.has(payload.messageId));
    });
    this.pendingView = { selected, messages, outbox, awaitingEcho, value };
    return value;
  }
  private emit() { if (!this.closed) for (const callback of this.callbacks) callback(); }
  private async commit(reduce: (old: MessageProjection) => MessageProjection): Promise<void> {
    const value = await this.storage.update(this.key, old => pendingCommands(this.revoked ? emptyProjection(this.uid, this.scope) : reduce({ ...this.state, ...this.validPending(old) })));
    this.applyPending(value);
    this.emit();
  }
  private applyPending(value: PendingCommands): void {
    if (this.closed) return;
    const previousCreation = this.state.creation;
    this.state = { ...this.state, ...value };
    if (previousCreation && pendingPayload(previousCreation).conversationId === this.selected
      && (!value.creation || pendingPayload(value.creation).conversationId !== this.selected)) {
      // The command may settle in another window. Replace the watch to ignore directory results queued before settlement.
      this.watch('replace');
    }
  }
  private validPending(value: PendingCommands | undefined): PendingCommands {
    if (!value || value.version !== 2 || value.uid !== this.uid || value.teamId !== this.scope.teamId || value.generation !== this.scope.generation)
      return pendingCommands(emptyProjection(this.uid, this.scope));
    if (!Array.isArray(value.outbox) || !value.outbox.every(item => this.validCommand(item, 'outbox'))
      || value.creation && !this.validCommand(value.creation, 'creation')) throw new Error('STORAGE_INVALID_DATA');
    return pendingCommands(value);
  }
  private validCommand(value: PendingCommand<MessageCommand>, kind: 'outbox' | 'creation'): boolean {
    if (!value || !value.command) return false;
    const command = value.command;
    if (command.teamId !== this.scope.teamId || command.generation !== this.scope.generation) return false;
    if ('schemaVersion' in command) return isMemberExecutionCommand(command)
      && (kind === 'creation' ? command.action === 'createConversation' : command.action === 'submit' || command.action === 'answerWait');
    // Human-message commands retain the existing transport; no execution targets are accepted here.
    const fields = kind === 'creation' ? ['action', 'teamId', 'generation', 'conversationId', 'kind', 'title', 'participantUids']
      : ['action', 'teamId', 'generation', 'conversationId', 'messageId', 'changeId', 'operation', 'text', 'replyTo', 'mentionEntities', 'channelMode'];
    return command.action === (kind === 'creation' ? 'create' : 'mutate') && Object.keys(command).every(key => fields.includes(key));
  }
  private async reload(): Promise<void> {
    const value = await this.storage.load(this.key);
    if (!this.closed) this.applyPending(this.validPending(value));
  }
  private watch(mode: 'overlap' | 'replace' = 'overlap'): void {
    const previous = this.stopWatch;
    this.stopWatch = undefined;
    const version = ++this.watchVersion;
    clearTimeout(this.connectionTimeout);
    this.connecting = true;
    // A cached SDK snapshot does not establish that the server is unreachable.
    // Bound the initial confirmation period so an unavailable network still
    // settles to offline even when the SDK only produces cached snapshots.
    this.connectionTimeout = setTimeout(() => { this.connecting = false; this.emit(); }, 10000);
    try {
      if (mode === 'replace') previous?.();
      const stop = this.api.watchMessages(this.scope, this.selected, snapshot => {
        if (version === this.watchVersion) this.receive(snapshot);
      });
      if (this.closed || version !== this.watchVersion) stop();
      else this.stopWatch = stop;
    } finally {
      if (mode === 'overlap') previous?.();
    }
  }
  private finishConnecting(): void { clearTimeout(this.connectionTimeout); this.connectionTimeout = undefined; this.connecting = false; }
  private receive(snapshot: MessageSnapshot): void {
    if (this.closed || snapshot.uid !== this.uid || snapshot.teamId !== this.scope.teamId || snapshot.generation !== this.scope.generation) return;
    if (snapshot.kind === 'error') { this.cancelPrefetch(); this.finishConnecting(); this.error = snapshot.error; this.offline = true; void this.deny(new Error(snapshot.error)); this.emit(); return; }
    if (snapshot.kind === 'postStats') return;
    if (snapshot.kind === 'directory') {
      this.state = mergeDirectory(this.state, snapshot.conversations, true);
      const allowed = new Set(snapshot.conversations.map(value => value.id));
      if (this.prefetch && !allowed.has(this.prefetch.id)) this.cancelPrefetch();
      this.state = { ...this.state, threads: Object.fromEntries(Object.entries(this.state.threads).filter(([id]) => allowed.has(id))) };
      if (!snapshot.fromCache && this.selected && !allowed.has(this.selected) && (this.state.creation ? pendingPayload(this.state.creation).conversationId : undefined) !== this.selected) { this.selected = ''; this.awaitingEcho = []; this.watch(); }
      this.directoryCached = snapshot.fromCache;
    } else if (snapshot.kind === 'messages') {
      if (snapshot.conversationId !== this.selected) return;
      const previous=this.state.threads[this.selected]?.messages;
      if(snapshot.rebase && previous?.length && this.historicalWindow) {
        void this.reloadWindow(previous[previous.length-1]!.seq+1,previous.length);
      } else {
        const through=this.historicalWindow ? previous?.[previous.length-1]?.seq : undefined;
        this.state = applyMessageSnapshot(this.state, through===undefined ? snapshot : {...snapshot,replace:false,messages:snapshot.messages.filter(row=>row.seq<=through)});
      }
      if (this.awaitingEcho.length) {
        const confirmed = new Set(this.state.threads[this.selected]?.messages.filter(row => row.author.kind === 'user' && row.author.uid === this.uid).map(row => row.id));
        const remaining = this.awaitingEcho.filter(intent => !confirmed.has(pendingPayload(intent).messageId));
        if (remaining.length !== this.awaitingEcho.length) this.awaitingEcho = remaining;
      }
      this.messagesCached = snapshot.fromCache;
      if (!snapshot.fromCache && snapshot.replace) this.cancelPrefetch(snapshot.conversationId);
    } else if (snapshot.kind === 'read') {
      this.state = { ...this.state, conversations: this.state.conversations.map(value => value.id === snapshot.conversationId ? { ...value, lastReadSeq: Math.max(value.lastReadSeq, snapshot.lastReadSeq), ...(snapshot.unreadCount===undefined?{}:{unreadCount:snapshot.unreadCount}), ...(snapshot.failureRevision===undefined?{}:{failureRevision:snapshot.failureRevision}),...(snapshot.pendingActionRevision===undefined?{}:{pendingActionRevision:snapshot.pendingActionRevision}), ...(snapshot.manualUnreadRevision===undefined?{}:{manualUnreadRevision:snapshot.manualUnreadRevision}) } : value) };
    }
    this.offline = this.directoryCached || (!!this.selected && this.messagesCached);
    if (!this.offline) this.finishConnecting();
    if (!snapshot.fromCache) { this.error = ''; this.retryDelay = 1000; void this.refresh(); }
    this.emit();
  }
  private scheduleRetry(): void {
    clearTimeout(this.retry); this.retry = undefined;
    if (this.closed) return;
    const pending = [this.state.creation, ...this.state.outbox].filter(value => value && !value.error);
    if (!pending.length) { this.retryDelay = 1000; return; }
    const lease = Math.min(...pending.map(value => Math.max(0, (value!.leaseUntil ?? 0) - Date.now())));
    this.retry = setTimeout(() => { this.retry = undefined; void this.refresh(); }, Math.max(lease, this.retryDelay));
    this.retryDelay = Math.min(30000, this.retryDelay * 2);
  }
  async start() {
    if (this.closed) return;
    this.stops.push(this.api.onMessagesChanged(() => void this.refresh()));
    const stopCommands = this.storage.onChanged?.(this.key, () => void this.refresh());
    if (stopCommands) this.stops.push(stopCommands);
    this.watch();
    try { await this.reload(); }
    catch { this.finishConnecting(); this.error = 'STORAGE_FAILED'; this.emit(); return; }
    if (!this.closed) void this.refresh();
  }
  reconnect(): void { if (this.closed) return; this.error = ''; this.watch(); this.emit(); void this.refresh(); }
  private canPrefetch(id: string): boolean {
    return !this.closed && !this.offline && !this.error && id !== this.selected
      && !!this.api.prefetchMessages && this.state.conversations.some(row => row.id === id && row.lastSeq > 0);
  }
  prefetchConversation(id: string): void {
    if (!this.canPrefetch(id) || this.prefetch?.id === id) return;
    this.cancelPrefetch();
    const now = Date.now();
    for (const [key, at] of this.prefetchedAt) if (now - at >= PREFETCH_COOLDOWN_MS) this.prefetchedAt.delete(key);
    if (this.prefetchedAt.has(id)) return;
    this.prefetch = { id, timer: setTimeout(() => {
      const startedAt = Date.now();
      if (startedAt < this.nextPrefetchAt || !this.canPrefetch(id)) { this.cancelPrefetch(id); return; }
      try {
        const stop = this.api.prefetchMessages!(this.scope, id);
        this.prefetchedAt.set(id, startedAt);
        this.nextPrefetchAt = startedAt + PREFETCH_MIN_INTERVAL_MS;
        this.prefetch = { id, stop, timer: setTimeout(() => this.cancelPrefetch(id), PREFETCH_TIMEOUT_MS) };
      } catch { this.cancelPrefetch(id); }
    }, PREFETCH_HOVER_MS) };
  }
  cancelPrefetch(id?: string): void {
    const active = this.prefetch;
    if (!active || (id !== undefined && active.id !== id)) return;
    this.prefetch = undefined;
    clearTimeout(active.timer);
    active.stop?.();
  }
  select(id: string) {
    // An in-flight prefetch for the selected channel can share its SDK query with the new watch.
    if (this.prefetch?.id !== id || !this.prefetch.stop) this.cancelPrefetch();
    this.pageRevision++;
    this.pageBusy = false;
    this.historicalWindow = false;
    if (id !== this.selected) this.awaitingEcho = [];
    this.selected = id;
    this.messagesCached = true;
    this.offline = this.directoryCached || !!id;
    const thread = this.state.threads[id];
    this.state = { ...this.state, threads: thread ? { [id]: thread } : {} };
    this.watch();
    this.error = '';
    this.emit();
    void this.refresh();
  }
  async older(beforePrepend?: (firstId: string) => void):Promise<string|undefined> {
    const thread=this.state.threads[this.selected]?.messages, before=thread?.[0]?.seq;
    if(!this.api.historyPage || this.pageBusy || !before || before<=1 || this.closed) return;
    const id=this.selected, revision=this.pageRevision;
    this.pageBusy=true;
    try {
      const messages=await this.api.historyPage(this.scope,id,before);
      if(this.closed || revision!==this.pageRevision) return;
      if(!messages.length) return;
      beforePrepend?.(messages[0].id);
      this.historicalWindow=this.historicalWindow || (thread?.length??0)+messages.length>200;
      this.state=applyMessageSnapshot(this.state,{kind:'messages',uid:this.uid,...this.scope,conversationId:id,fromCache:false,messages,replace:false,older:true});
      this.emit();
      return messages[0]?.id;
    } finally {if(revision===this.pageRevision)this.pageBusy=false;}
  }
  private async reloadWindow(before:number,count:number) {
    if(!this.api.historyPage)return;
    const revision=++this.pageRevision,id=this.selected;
    this.pageBusy=true;
    try {
      const messages:TeamMessage[]=[];
      while(messages.length<count) {
        const page=await this.api.historyPage(this.scope,id,before);
        if(this.closed || revision!==this.pageRevision) return;
        messages.unshift(...page);
        if(page.length<50 || !page[0] || page[0].seq>=before) break;
        before=page[0].seq;
      }
      this.state=applyMessageSnapshot(this.state,{kind:'messages',uid:this.uid,...this.scope,conversationId:id,fromCache:false,messages,replace:true});
      this.emit();
    } catch(error) {if(revision===this.pageRevision){this.error=error instanceof Error?error.message:'MESSAGE_LISTENER_FAILED';this.emit();}}
    finally {if(revision===this.pageRevision)this.pageBusy=false;}
  }
  async visibleRead(id:string,through:number,revision:number,pendingRevision?:number,failureRevision?:number) {
    if(this.closed || this.selected!==id)return;
    if(this.api.readVisible) return this.api.readVisible(this.scope,id,through,revision,pendingRevision,failureRevision);
    await this.markRead(through);
  }
  async create(kind: TeamConversation['kind'], title: string, participantUids: string[], sharedBotIds: string[] = []) {
    if (this.closed) throw new Error('INVALID_SESSION');
    if (sharedBotIds.length > 8 || new Set(sharedBotIds).size !== sharedBotIds.length || (kind === 'dm' && sharedBotIds.length)
      || (kind === 'bot_dm' && (sharedBotIds.length !== 1 || participantUids.length !== 1))) throw new Error('INVALID_BOT_TARGET');
    const conversationId = crypto.randomUUID();
    const command: CreationCommand = sharedBotIds.length && kind !== 'dm'
      ? executionCommand(this.scope, 'createConversation', { conversationId, kind, title, participantUids: [...participantUids], sharedBotIds: [...sharedBotIds] })
      : { action: 'create', ...this.scope, conversationId, kind, title, participantUids: [...participantUids] };
    await this.commit(old => { if (old.creation) throw new Error('CREATION_PENDING'); return { ...old, creation: { command } }; });
    this.select(conversationId); await this.refresh();
    return conversationId;
  }
  /** `attachments`: checked uploads of this sender for the open conversation. A new message may be files alone. */
  async queue(operation: MessageMutation['operation'], text: string, messageId: string = crypto.randomUUID(), replyTo?: string, targets: string[] = [], attachments: string[] = [], dataRef?: DataRef, privateDataSource?: string, mentionEntities?: MentionEntity[]) {
    if (this.closed || !this.selected || (operation !== 'recall' && ((!text.trim() && !attachments.length) || new TextEncoder().encode(text).length > 8192))) throw new Error('INVALID_INPUT');
    if (operation !== 'create' && (targets.length || attachments.length)) throw new Error('INVALID_BOT_TARGET');
    if (attachments.length > ATTACHMENT_MESSAGE_FILES || new Set(attachments).size !== attachments.length) throw new Error('ATTACHMENT_LIMIT');
    if (targets.length > 8 || new Set(targets).size !== targets.length) throw new Error('INVALID_BOT_TARGET');
    const conversation = this.state.conversations.find(value => value.id === this.selected);
    const selectedTargets = targets.length ? targets : operation === 'create' && conversation?.kind === 'bot_dm'
      ? conversation.participants.filter(value => value.startsWith('shared_bot:')).map(value => value.slice(11)) : [];
    if (dataRef && (!parseDataRef(dataRef) || operation !== 'create' || selectedTargets.length || attachments.length)) throw new Error('INVALID_INPUT');
    if (privateDataSource && (!/^[A-Za-z0-9_-]{1,64}$/u.test(privateDataSource)
      || operation !== 'create' || replyTo || selectedTargets.length !== 1 || attachments.length || dataRef)) throw new Error('INVALID_INPUT');
    const changeId = crypto.randomUUID();
    const channelMode = conversation?.kind === 'channel' && operation === 'create' ? conversation.posting?.mode ?? 'chat' : undefined;
    const channel = channelMode ? { channelMode } : {};
    // Files alone carry no text: surrounding spaces are not a message.
    const command: OutboxCommand = operation === 'create' && (selectedTargets.length || attachments.length)
      ? executionCommand(this.scope, 'submit', { conversationId: this.selected, messageId, changeId, text: text.trim() ? text : '', targets: [...selectedTargets], ...channel,
        ...(replyTo ? { replyTo } : {}), ...(attachments.length ? { attachments: [...attachments] } : {}),
        ...(privateDataSource ? { privateDataSource } : {}), ...(mentionEntities?.length ? { mentionEntities } : {}) })
      : { action: 'mutate', ...this.scope, conversationId: this.selected, messageId, changeId, operation, text, ...channel, ...(replyTo ? { replyTo } : {}), ...(dataRef ? { dataRef } : {}), ...(mentionEntities !== undefined ? { mentionEntities } : {}) };
    const intent = { command, enqueuedAt: operation === 'create' ? new Date().toISOString() : undefined };
    await this.commit(old => ({ ...old, outbox: [...old.outbox, intent] }));
    void this.refresh();
  }
  /**
   * Answer a Bot's question. The command (with its fresh message and change ids) is stored before it is sent, so a retry after a
   * lost response repeats the same ids. A refused answer is dropped from the queue and thrown, so the caller keeps the text;
   * any other failure stays queued and is retried like a message.
   */
  async answer(waitId: string, text: string) {
    if (this.closed || !this.selected || !text.trim() || new TextEncoder().encode(text).length > 8192) throw new Error('INVALID_INPUT');
    const changeId = crypto.randomUUID();
    const command = executionCommand(this.scope, 'answerWait', { conversationId: this.selected, waitId, messageId: crypto.randomUUID(), changeId, text });
    const intent = { command, enqueuedAt: new Date().toISOString() };
    await this.commit(old => ({ ...old, outbox: [...old.outbox, intent] }));
    await this.refresh();
    const refused = this.state.outbox.find(item => pendingPayload(item).changeId === changeId)?.error;
    if (refused && ANSWER_REFUSED.includes(refused)) { await this.discard(changeId); throw new Error(refused); }
  }
  async discard(changeId: string) { await this.commit(old => { if (old.outbox.some(item => pendingPayload(item).changeId === changeId && ((item.attempted && !item.error) || (item.leaseUntil ?? 0) > Date.now()))) throw new Error('MESSAGE_IN_FLIGHT'); return { ...old, outbox: old.outbox.filter(item => pendingPayload(item).changeId !== changeId) }; }); }
  /** Retry an explicit host-unavailable verdict with the original idempotent command. */
  async retryHostUnavailable(changeId: string) {
    if (this.closed) throw new Error('INVALID_SESSION');
    await this.commit(old => ({ ...old, outbox: old.outbox.map(item => {
      if (pendingPayload(item).changeId !== changeId || item.error !== 'HOST_UNAVAILABLE' || (item.leaseUntil ?? 0) > Date.now()) return item;
      const { error: _error, leaseUntil: _lease, retryDelay: _delay, ...retry } = item;
      return retry;
    }) }));
    if (!this.closed) void this.refresh();
  }
  async discardCreation() { await this.commit(old => { if ((old.creation?.attempted && !old.creation.error) || (old.creation?.leaseUntil ?? 0) > Date.now()) throw new Error('MESSAGE_IN_FLIGHT'); return { ...old, creation: undefined }; }); }
  async markRead(through?:number) {
    const id = this.selected, thread = this.state.threads[id];
    if (!thread || this.closed || this.offline && !this.api.readVisible) return;
    const conversation = this.state.conversations.find(row => row.id === id);
    const seq = through ?? conversation?.lastSeq ?? thread.messages.at(-1)?.seq ?? 0;
    if (this.api.readVisible) {
      await this.api.readVisible(this.scope, id, seq, conversation?.manualUnreadRevision ?? 0,
        conversation?.pendingActionRevision, conversation?.failureRevision);
      return;
    }
    let value: { lastReadSeq: number };
    try { value = await this.api.messages<{ lastReadSeq: number }>({ action: 'read', ...this.scope, conversationId: id, lastReadSeq: seq }); } catch (error) { await this.deny(error); throw error; }
    if (this.closed) return;
    this.state = { ...this.state, conversations: this.state.conversations.map(c => c.id === id ? { ...c, lastReadSeq: Math.max(c.lastReadSeq, value.lastReadSeq) } : c) }; this.emit();
  }
  refresh(): Promise<void> {
    if (this.closed) return Promise.resolve();
    if (this.running) { this.again = true; return this.running; }
    this.running = (async () => {
      do { this.again = false; await this.cycle(); } while (this.again && !this.closed);
    })().finally(() => { this.running = undefined; });
    return this.running;
  }
  private async deny(error: unknown) {
    if (!isRevokedAccessError(error)) return;
    this.offline = true; this.error = (error as Error).message; this.revoked = true;
    this.state = emptyProjection(this.uid, this.scope); this.selected = ''; this.emit(); this.close();
    try { await this.storage.update(this.key, () => emptyProjection(this.uid, this.scope)); } catch { /* The view is cleared even if durable storage has failed. */ }
  }
  private async cycle() {
    try {
      await this.reload();
      let creation: MessageProjection['creation'];
      if (this.state.creation) await this.commit(old => {
        if (!old.creation || old.creation.error || (old.creation.leaseUntil ?? 0) > Date.now()) return old;
        creation = { ...old.creation, leaseUntil: Date.now() + 30000, attempted: true }; return { ...old, creation };
      });
      if (creation) {
        const command = creation.command;
        const conversationId = pendingPayload(creation).conversationId;
        const unselectedWatchVersion = this.selected ? undefined : this.watchVersion;
        try {
          await this.api.messages(command);
          if (this.closed) return;
          await this.commit(old => old.creation && pendingPayload(old.creation).conversationId === conversationId
            ? { ...old, creation: undefined } : old);
          if (!this.closed && unselectedWatchVersion === this.watchVersion) this.select(conversationId);
        } catch (error) {
          if (isRevokedAccessError(error)) throw error;
          const isPermanent = isPermanentMessageError(error) || error instanceof Error && ['CONVERSATION_REJECTED', 'PARTICIPANT_REQUIRED'].includes(error.message);
          await this.commit(old => ({ ...old, creation: old.creation ? failedCommand(old.creation, error, isPermanent) : undefined }));
          if (!isPermanent) throw error;
        }
      }
      // Reload the durable queue; another window/tab can append while this one waits.
      await this.reload();
      const blockedConversations = new Set<string>();
      for (const pending of this.state.outbox) {
        if (pending.error || this.closed || blockedConversations.has(pendingPayload(pending).conversationId)) continue;
        let intent: MessageProjection['outbox'][number] | undefined;
        await this.commit(old => ({ ...old, outbox: old.outbox.map(item => { if (pendingPayload(item).changeId !== pendingPayload(pending).changeId || item.error || (item.leaseUntil ?? 0) > Date.now()) return item; intent = { ...item, leaseUntil: Date.now() + 30000, attempted: true }; return intent; }) }));
        if (!intent || this.closed) { blockedConversations.add(pendingPayload(pending).conversationId); continue; }
        const changeId = pendingPayload(intent).changeId;
        try {
          const command = intent.command;
          await this.api.messages(command);
          if (this.closed) return;
          // Transport can settle before Firestore delivers the row. Retire the
          // durable command without removing the message from the open transcript.
          const payload = pendingPayload(intent);
          if (createsMessage(intent) && this.selected === payload.conversationId
            && !this.state.threads[this.selected]?.messages.some(row =>
              row.id === payload.messageId && row.author.kind === 'user' && row.author.uid === this.uid)
            && !this.awaitingEcho.some(value => pendingPayload(value).changeId === changeId)) {
            this.awaitingEcho = [...this.awaitingEcho, { ...intent, acknowledged: true }];
          }
          await this.commit(old => ({ ...old, outbox: old.outbox.filter(item => pendingPayload(item).changeId !== changeId) }));
        } catch (error) {
          await this.commit(old => ({ ...old, outbox: old.outbox.map(item => pendingPayload(item).changeId === changeId ? failedCommand(item, error, isPermanentMessageError(error)) : item) }));
          if (!isPermanentMessageError(error)) throw error;
        }
      }
    } catch (error) {
      if (this.closed) return;
      this.again = false;
      this.finishConnecting();
      this.offline = true; this.error = error instanceof Error ? error.message : 'SERVICE_UNAVAILABLE';
      await this.deny(error);
    }
    this.scheduleRetry(); this.emit();
  }
  close() { this.closed = true; this.cancelPrefetch(); this.prefetchedAt.clear(); this.awaitingEcho = []; this.pendingView = undefined; this.finishConnecting(); clearTimeout(this.retry); this.stopWatch?.(); this.stopWatch = undefined; this.stops.forEach(stop => stop()); this.stops = []; this.callbacks.clear(); }
}
