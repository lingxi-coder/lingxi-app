import { isMemberExecutionCommand } from './execution-validation.js';
import { executionCommand, type MemberExecutionCommand } from './execution.js';
export type MessageAuthor = { kind: 'user'; uid: string; displayName?: string } | { kind: 'shared_bot'; sharedBotId: string; displayName: string };
export const authorIdentity = (a: MessageAuthor) => a.kind === 'user' ? `user:${a.uid}` : `shared_bot:${a.sharedBotId}`;
export interface TeamConversation {
  id: string; kind: 'dm' | 'group' | 'channel' | 'bot_dm'; title: string; participants: string[];
  lastSeq: number; lastChangeSeq: number; lastReadSeq: number;
  /** Newest message for conversation lists. Absent until the conversation has a message; then always the full pointer. */
  lastMessage?: { id: string; seq: number; at: string | null; author: MessageAuthor; preview: string; recalled?: true };
}
/** One change a Bot asks to make to its list. `item` is the text of the item it touches as it read when the Bot asked, `null` if it was gone. */
export interface ListApprovalChange { op: 'add' | 'update' | 'remove'; text?: string; itemId?: string; done?: boolean; item?: string | null }
export interface ListApproval { title: string; changes: ListApprovalChange[] }
export interface TeamMessage {
  id: string; seq: number; author: MessageAuthor;
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
}
export interface MessageScope { teamId: string; generation: number }
export type MessageMutation = MessageScope & { action: 'mutate'; conversationId: string; messageId: string; changeId: string;
  operation: 'create' | 'edit' | 'recall'; text: string; replyTo?: string };
export type MessageCommand = MessageMutation | MemberExecutionCommand
  | MessageScope & { action: 'create'; conversationId: string; kind: TeamConversation['kind']; title: string; participantUids: string[];  }
  | MessageScope & { action: 'read'; conversationId: string; lastReadSeq: number };
export interface MessageSignal { bots?: boolean; conversationId?: string }
export interface MessageApi {
  messages<T>(command: MessageCommand): Promise<T>;
  onMessagesChanged(callback: (signal?: MessageSignal) => void): () => void;
  watchMessages(scope: MessageScope, conversationId: string, next: (snapshot: MessageSnapshot) => void): () => void;
}
export type MessageSnapshot = MessageScope & { uid: string; fromCache: boolean } & (
  | { kind: 'directory'; conversations: TeamConversation[] }
  | { kind: 'messages'; conversationId: string; messages: TeamMessage[]; removed?: string[]; replace?: boolean }
  | { kind: 'read'; conversationId: string; lastReadSeq: number }
  | { kind: 'signal'; signal?: MessageSignal }
  | { kind: 'error'; error: string }
);
interface CommandMetadata { error?: string; leaseUntil?: number; attempted?: boolean; retryDelay?: number }
type CreationCommand = Extract<MessageCommand, { action: 'create' | 'createConversation' }>;
type OutboxCommand = MessageMutation | Extract<MemberExecutionCommand, { action: 'submit' | 'answerWait' }>;
export interface PendingCommand<C> extends CommandMetadata { command: C }
export interface PendingCommands {
  version: 2; uid: string; teamId: string; generation: number;
  creation?: PendingCommand<CreationCommand>;
  outbox: PendingCommand<OutboxCommand>[];
}
export function pendingPayload<C extends MessageCommand>(value: PendingCommand<C>): C extends { payload: infer P } ? P : C {
  return ('payload' in value.command ? value.command.payload : value.command) as C extends { payload: infer P } ? P : C;
}
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
    && old.lastSeq === merged.lastSeq && old.lastChangeSeq === merged.lastChangeSeq && old.lastReadSeq === merged.lastReadSeq
    && sameLastMessage(old.lastMessage, merged.lastMessage)
    && old.participants.length === merged.participants.length && old.participants.every((value, index) => value === merged.participants[index])) return old;
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
    for (const value of snapshot.messages) records.set(value.id, value);
    messages = [...records.values()].sort((a, b) => a.seq - b.seq);
  }
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
const isPermanentMessageError = (error: unknown) => error instanceof Error && ['MESSAGE_ID_CONFLICT', 'MESSAGE_RECALLED', 'MESSAGE_AUTHOR_REQUIRED', 'REPLY_REQUIRED', 'INVALID_INPUT', 'BOT_USE_REQUIRED', 'BOT_PARTICIPANT_REQUIRED', 'INVALID_BOT_TARGET', 'HOST_UNAVAILABLE', 'REQUEST_QUOTA_EXCEEDED', 'CONVERSATION_POLICY_REQUIRED', 'INPUT_GRANT_REQUIRED', 'BOT_RETIRED', 'COMMAND_ID_CONFLICT', ...ANSWER_REFUSED].includes(error.message);
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
  private running?: Promise<void>;
  private again = false;
  private stops: (() => void)[] = [];
  private callbacks = new Set<() => void>();
  private key: string;
  private retry?: ReturnType<typeof setTimeout>;
  private retryDelay = 1000;
  private stopWatch?: () => void;
  private connectionTimeout?: ReturnType<typeof setTimeout>;
  constructor(private api: MessageApi, private storage: CommandStorage, readonly uid: string, readonly scope: MessageScope) {
    this.state = emptyProjection(uid, scope); this.key = projectionKey(uid, scope);
  }
  onChanged(callback: () => void) { this.callbacks.add(callback); return () => { this.callbacks.delete(callback); }; }
  private emit() { if (!this.closed) for (const callback of this.callbacks) callback(); }
  private async commit(reduce: (old: MessageProjection) => MessageProjection): Promise<void> {
    const value = await this.storage.update(this.key, old => pendingCommands(this.revoked ? emptyProjection(this.uid, this.scope) : reduce({ ...this.state, ...this.validPending(old) })));
    if (!this.closed) { this.state = { ...this.state, ...value }; this.emit(); }
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
      : ['action', 'teamId', 'generation', 'conversationId', 'messageId', 'changeId', 'operation', 'text', 'replyTo'];
    return command.action === (kind === 'creation' ? 'create' : 'mutate') && Object.keys(command).every(key => fields.includes(key));
  }
  private async reload(): Promise<void> {
    const value = await this.storage.load(this.key);
    if (!this.closed) this.state = { ...this.state, ...this.validPending(value) };
  }
  private watch(): void {
    this.stopWatch?.();
    clearTimeout(this.connectionTimeout);
    this.connecting = true;
    // A cached SDK snapshot does not establish that the server is unreachable.
    // Bound the initial confirmation period so an unavailable network still
    // settles to offline even when the SDK only produces cached snapshots.
    this.connectionTimeout = setTimeout(() => { this.connecting = false; this.emit(); }, 10000);
    this.stopWatch = this.api.watchMessages(this.scope, this.selected, snapshot => this.receive(snapshot));
  }
  private finishConnecting(): void { clearTimeout(this.connectionTimeout); this.connectionTimeout = undefined; this.connecting = false; }
  private receive(snapshot: MessageSnapshot): void {
    if (this.closed || snapshot.uid !== this.uid || snapshot.teamId !== this.scope.teamId || snapshot.generation !== this.scope.generation) return;
    if (snapshot.kind === 'error') { this.finishConnecting(); this.error = snapshot.error; this.offline = true; void this.deny(new Error(snapshot.error)); this.emit(); return; }
    if (snapshot.kind === 'directory') {
      this.state = mergeDirectory(this.state, snapshot.conversations, true);
      const allowed = new Set(snapshot.conversations.map(value => value.id));
      this.state = { ...this.state, threads: Object.fromEntries(Object.entries(this.state.threads).filter(([id]) => allowed.has(id))) };
      if (!snapshot.fromCache && this.selected && !allowed.has(this.selected) && (this.state.creation ? pendingPayload(this.state.creation).conversationId : undefined) !== this.selected) { this.selected = ''; this.watch(); }
      this.directoryCached = snapshot.fromCache;
    } else if (snapshot.kind === 'messages') {
      if (snapshot.conversationId !== this.selected) return;
      this.state = applyMessageSnapshot(this.state, snapshot);
      this.messagesCached = snapshot.fromCache;
    } else if (snapshot.kind === 'read') {
      this.state = { ...this.state, conversations: this.state.conversations.map(value => value.id === snapshot.conversationId ? { ...value, lastReadSeq: Math.max(value.lastReadSeq, snapshot.lastReadSeq) } : value) };
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
  select(id: string) { this.selected = id; this.messagesCached = true; this.offline = this.directoryCached || !!id; this.state = { ...this.state, threads: this.state.threads[id] ? { [id]: this.state.threads[id]! } : {} }; this.watch(); this.error = ''; this.emit(); void this.refresh(); }
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
  }
  async queue(operation: MessageMutation['operation'], text: string, messageId: string = crypto.randomUUID(), replyTo?: string, targets: string[] = []) {
    if (this.closed || !this.selected || (operation !== 'recall' && (!text.trim() || new TextEncoder().encode(text).length > 8192))) throw new Error('INVALID_INPUT');
    if (operation !== 'create' && targets.length) throw new Error('INVALID_BOT_TARGET');
    if (targets.length > 8 || new Set(targets).size !== targets.length) throw new Error('INVALID_BOT_TARGET');
    const conversation = this.state.conversations.find(value => value.id === this.selected);
    const selectedTargets = targets.length ? targets : operation === 'create' && conversation?.kind === 'bot_dm'
      ? conversation.participants.filter(value => value.startsWith('shared_bot:')).map(value => value.slice(11)) : [];
    const changeId = crypto.randomUUID();
    const command: OutboxCommand = operation === 'create' && selectedTargets.length
      ? executionCommand(this.scope, 'submit', { conversationId: this.selected, messageId, changeId, text, targets: [...selectedTargets], ...(replyTo ? { replyTo } : {}) })
      : { action: 'mutate', ...this.scope, conversationId: this.selected, messageId, changeId, operation, text, ...(replyTo ? { replyTo } : {}) };
    await this.commit(old => ({ ...old, outbox: [...old.outbox, { command }] }));
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
    await this.commit(old => ({ ...old, outbox: [...old.outbox, { command }] }));
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
  async markRead() {
    const id = this.selected, thread = this.state.threads[id];
    if (!thread || this.offline || this.closed) return;
    let seq = 0;
    for (const message of thread.messages) seq = Math.max(seq, message.seq);
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
        try {
          await this.api.messages(command);
          if (this.closed) return;
          await this.commit(old => ({ ...old, creation: undefined }));
          if (!this.selected) this.selected = ('payload' in command ? command.payload.conversationId : command.conversationId);
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
  close() { this.closed = true; this.finishConnecting(); clearTimeout(this.retry); this.stopWatch?.(); this.stopWatch = undefined; this.stops.forEach(stop => stop()); this.stops = []; this.callbacks.clear(); }
}
