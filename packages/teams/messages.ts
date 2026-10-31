import { canRecoverBot, isQuestionRecipient, type TeamBotAnswer, type TeamBotCommand, type TeamBotRecovery, type TeamBotState } from './bots.js';
export interface TeamConversation {
  id: string; kind: 'dm' | 'group' | 'channel' | 'bot_dm'; title: string; participants: string[];
  lastSeq: number; lastChangeSeq: number; lastReadSeq: number; botKey?: string;
}
export interface TeamMessage {
  id: string; seq: number; author: { kind: 'user' | 'bot'; uid: string; botKey?: string };
  kind: string; text: string; replyTo?: string; createdAt: string | null; editedAt?: string | null;
  recalled?: boolean; changeSeq: number; changeId: string;
  triggerId?: string; origin?: { uid: string; generation: number }; question?: { id: string; questions: string[] };
  mentions?: string[];
}
export interface MessageScope { teamId: string; generation: number }
export type MessageMutation = MessageScope & { action: 'mutate'; conversationId: string; messageId: string; changeId: string;
  operation: 'create' | 'edit' | 'recall'; text: string; replyTo?: string; targets?: string[] };
export type MessageCommand = MessageMutation | TeamBotCommand
  | MessageScope & { action: 'create'; conversationId: string; kind: TeamConversation['kind']; title: string; participantUids: string[]; botKeys?: string[] }
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
export interface PendingCommands {
  version: 1; uid: string; teamId: string; generation: number;
  creation?: Extract<MessageCommand, { action: 'create' }> & CommandMetadata;
  outbox: (MessageMutation & CommandMetadata)[];
  answerReceipts?: string[];
  answerOutbox?: (TeamBotAnswer & { question: NonNullable<TeamMessage['question']>; origin: NonNullable<TeamMessage['origin']> } & CommandMetadata)[];
  recoveryReceipts?: string[];
  recoveryOutbox?: (TeamBotRecovery & CommandMetadata)[];
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
  const { version, uid, teamId, generation, creation, outbox, answerReceipts, answerOutbox, recoveryReceipts, recoveryOutbox } = value;
  return { version, uid, teamId, generation, creation, outbox, answerReceipts, answerOutbox, recoveryReceipts, recoveryOutbox };
}
export const projectionKey = (uid: string, scope: MessageScope) => JSON.stringify([uid, scope.teamId, scope.generation]);
export const emptyProjection = (uid: string, scope: MessageScope): MessageProjection => ({ version: 1, uid, ...scope, conversations: [], threads: {}, outbox: [] });
export function mergeConversation(old: TeamConversation | undefined, value: TeamConversation): TeamConversation {
  const merged = { ...value, lastSeq: Math.max(old?.lastSeq ?? 0, value.lastSeq), lastChangeSeq: Math.max(old?.lastChangeSeq ?? 0, value.lastChangeSeq), lastReadSeq: Math.max(old?.lastReadSeq ?? 0, value.lastReadSeq) };
  if (old && old.kind === merged.kind && old.title === merged.title && old.botKey === merged.botKey
    && old.lastSeq === merged.lastSeq && old.lastChangeSeq === merged.lastChangeSeq && old.lastReadSeq === merged.lastReadSeq
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
const isRevokedAccessError = (error: unknown) => error instanceof Error && ['MEMBERSHIP_REQUIRED', 'STALE_MEMBERSHIP', 'INVALID_SESSION', 'SIGN_IN_REQUIRED', 'CONVERSATION_REQUIRED'].includes(error.message);
const isPermanentMessageError = (error: unknown) => error instanceof Error && ['MESSAGE_ID_CONFLICT', 'MESSAGE_RECALLED', 'MESSAGE_AUTHOR_REQUIRED', 'REPLY_REQUIRED', 'INVALID_INPUT', 'BOT_USE_REQUIRED', 'BOT_PARTICIPANT_REQUIRED', 'INVALID_BOT_TARGET', 'QUESTION_RECIPIENT_REQUIRED', 'INVALID_ANSWER', 'ANSWER_CONFLICT'].includes(error.message);
const isPermanentRecoveryError = (error: unknown) => isPermanentMessageError(error) || error instanceof Error && ['RECOVERY_NOT_REQUIRED', 'BOT_RESULT_CONFLICT', 'BOT_OWNER_REQUIRED', 'TRIGGER_REQUIRED'].includes(error.message);
export const recoveryKey = (conversationId: string, triggerId: string) => JSON.stringify([conversationId, triggerId]);

/** Persist command identities before transport; Firestore owns message history. */
export class TeamMessaging {
  state: MessageProjection;
  selected = '';
  offline = true;
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
    if (value?.version === 1 && value.uid === this.uid && value.teamId === this.scope.teamId && value.generation === this.scope.generation) return pendingCommands(value);
    return pendingCommands(emptyProjection(this.uid, this.scope));
  }
  private async reload(): Promise<void> {
    const value = await this.storage.load(this.key);
    if (!this.closed) this.state = { ...this.state, ...this.validPending(value) };
  }
  private watch(): void {
    this.stopWatch?.();
    this.stopWatch = this.api.watchMessages(this.scope, this.selected, snapshot => this.receive(snapshot));
  }
  private receive(snapshot: MessageSnapshot): void {
    if (this.closed || snapshot.uid !== this.uid || snapshot.teamId !== this.scope.teamId || snapshot.generation !== this.scope.generation) return;
    if (snapshot.kind === 'error') { this.error = snapshot.error; this.offline = true; void this.deny(new Error(snapshot.error)); this.emit(); return; }
    if (snapshot.kind === 'directory') {
      this.state = mergeDirectory(this.state, snapshot.conversations, true);
      const allowed = new Set(snapshot.conversations.map(value => value.id));
      this.state = { ...this.state, threads: Object.fromEntries(Object.entries(this.state.threads).filter(([id]) => allowed.has(id))) };
      if (!snapshot.fromCache && this.selected && !allowed.has(this.selected) && this.state.creation?.conversationId !== this.selected) { this.selected = ''; this.watch(); }
      this.directoryCached = snapshot.fromCache;
    } else if (snapshot.kind === 'messages') {
      if (snapshot.conversationId !== this.selected) return;
      this.state = applyMessageSnapshot(this.state, snapshot);
      this.messagesCached = snapshot.fromCache;
    } else if (snapshot.kind === 'read') {
      this.state = { ...this.state, conversations: this.state.conversations.map(value => value.id === snapshot.conversationId ? { ...value, lastReadSeq: Math.max(value.lastReadSeq, snapshot.lastReadSeq) } : value) };
    }
    this.offline = this.directoryCached || (!!this.selected && this.messagesCached);
    if (!snapshot.fromCache) { this.error = ''; this.retryDelay = 1000; void this.refresh(); }
    this.emit();
  }
  private scheduleRetry(): void {
    clearTimeout(this.retry); this.retry = undefined;
    if (this.closed) return;
    const pending = [this.state.creation, ...this.state.outbox, ...(this.state.answerOutbox ?? []), ...(this.state.recoveryOutbox ?? [])].filter(value => value && !value.error);
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
    catch { this.error = 'STORAGE_FAILED'; this.emit(); return; }
    if (!this.closed) void this.refresh();
  }
  reconnect(): void { if (this.closed) return; this.error = ''; this.watch(); void this.refresh(); }
  select(id: string) { this.selected = id; this.messagesCached = true; this.offline = this.directoryCached || !!id; this.state = { ...this.state, threads: this.state.threads[id] ? { [id]: this.state.threads[id]! } : {} }; this.watch(); this.error = ''; this.emit(); void this.refresh(); }
  async create(kind: TeamConversation['kind'], title: string, participantUids: string[], botKeys: string[] = []) {
    if (this.closed) throw new Error('INVALID_SESSION');
    if (botKeys.length > 8 || new Set(botKeys).size !== botKeys.length || (kind === 'dm' && botKeys.length)
      || (kind === 'bot_dm' && (botKeys.length !== 1 || participantUids.length !== 1))) throw new Error('INVALID_BOT_TARGET');
    const command: Extract<MessageCommand, { action: 'create' }> = { action: 'create', ...this.scope, conversationId: crypto.randomUUID(), kind, title, participantUids: [...participantUids], ...(botKeys.length ? { botKeys: [...botKeys] } : {}) };
    await this.commit(old => { if (old.creation) throw new Error('CREATION_PENDING'); return { ...old, creation: command }; });
    this.select(command.conversationId); await this.refresh();
  }
  async queue(operation: MessageMutation['operation'], text: string, messageId: string = crypto.randomUUID(), replyTo?: string, targets: string[] = []) {
    if (this.closed || !this.selected || (operation !== 'recall' && (!text.trim() || new TextEncoder().encode(text).length > 8192))) throw new Error('INVALID_INPUT');
    if (operation !== 'create' && targets.length) throw new Error('INVALID_BOT_TARGET');
    if (targets.length > 8 || new Set(targets).size !== targets.length) throw new Error('INVALID_BOT_TARGET');
    const intent: MessageMutation = { action: 'mutate', ...this.scope, conversationId: this.selected, messageId, changeId: crypto.randomUUID(), operation, text, ...(replyTo ? { replyTo } : {}), ...(targets.length ? { targets: [...targets] } : {}) };
    await this.commit(old => ({ ...old, outbox: [...old.outbox, intent] }));
    void this.refresh();
  }
  async answer(message: TeamMessage, answers: Record<string, string>) {
    if (this.closed || !this.selected || !isQuestionRecipient(message, this.uid, this.scope.generation)) throw new Error('QUESTION_RECIPIENT_REQUIRED');
    const questions = message.question!;
    if (Object.keys(answers).length !== questions.questions.length || questions.questions.some(q => !answers[q]?.trim() || answers[q]!.length > 2000)) throw new Error('INVALID_ANSWER');
    const intent = { action: 'botAnswer' as const, ...this.scope, conversationId: this.selected, triggerId: message.triggerId!, questionId: questions.id,
      answerId: crypto.randomUUID(), answers: { ...answers }, question: structuredClone(questions), origin: { ...message.origin! } };
    await this.commit(old => {
      if (old.answerReceipts?.includes(JSON.stringify([intent.conversationId, intent.questionId]))) throw new Error('ANSWER_CONFLICT');
      if (old.answerOutbox?.some(value => value.conversationId === intent.conversationId && value.questionId === intent.questionId)) throw new Error('ANSWER_PENDING');
      return { ...old, answerOutbox: [...(old.answerOutbox ?? []), intent] };
    });
    void this.refresh();
  }
  async discardAnswer(answerId: string) {
    await this.commit(old => {
      if (old.answerOutbox?.some(item => item.answerId === answerId && ((item.attempted && !item.error) || (item.leaseUntil ?? 0) > Date.now()))) throw new Error('MESSAGE_IN_FLIGHT');
      return { ...old, answerOutbox: old.answerOutbox?.filter(item => item.answerId !== answerId) };
    });
  }
  async recover(conversationId: string, triggerId: string, text: string) {
    if (this.closed || !text.trim() || new TextEncoder().encode(text).length > 8192) throw new Error('INVALID_INPUT');
    await this.reload();
    const key = recoveryKey(conversationId, triggerId);
    const pending = this.state.recoveryOutbox?.find(item => recoveryKey(item.conversationId, item.triggerId) === key);
    if (pending) {
      if (pending.text !== text) throw new Error('BOT_RESULT_CONFLICT');
      void this.refresh(); return;
    }
    if (this.state.recoveryReceipts?.includes(key)) throw new Error('RECOVERY_NOT_REQUIRED');
    // A new manual settlement needs a fresh owner/lease check. An existing durable
    // intent is retried unchanged even after its first response was lost.
    let current: TeamBotState;
    try { current = await this.api.messages<TeamBotState>({ action: 'botState', ...this.scope, conversationId, triggerId }); }
    catch (error) { await this.deny(error); throw error; }
    if (this.closed) throw new Error('INVALID_SESSION');
    if (!canRecoverBot(current, this.uid)) throw new Error('RECOVERY_NOT_REQUIRED');
    const intent: TeamBotRecovery = { action: 'recoverBot', ...this.scope, conversationId, triggerId, botKey: current.botKey, text };
    await this.commit(old => {
      if (old.recoveryReceipts?.includes(key)) throw new Error('RECOVERY_NOT_REQUIRED');
      const existing = old.recoveryOutbox?.find(item => recoveryKey(item.conversationId, item.triggerId) === key);
      if (existing) { if (existing.text !== text) throw new Error('BOT_RESULT_CONFLICT'); return old; }
      return { ...old, recoveryOutbox: [...(old.recoveryOutbox ?? []), intent] };
    });
    void this.refresh();
  }
  async discardRejectedRecovery(conversationId: string, triggerId: string) {
    const key = recoveryKey(conversationId, triggerId);
    await this.commit(old => {
      const item = old.recoveryOutbox?.find(value => recoveryKey(value.conversationId, value.triggerId) === key);
      if (item && (!item.error || (item.leaseUntil ?? 0) > Date.now())) throw new Error('MESSAGE_IN_FLIGHT');
      return { ...old, recoveryOutbox: old.recoveryOutbox?.filter(value => recoveryKey(value.conversationId, value.triggerId) !== key) };
    });
  }
  async discard(changeId: string) { await this.commit(old => { if (old.outbox.some(item => item.changeId === changeId && ((item.attempted && !item.error) || (item.leaseUntil ?? 0) > Date.now()))) throw new Error('MESSAGE_IN_FLIGHT'); return { ...old, outbox: old.outbox.filter(item => item.changeId !== changeId) }; }); }
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
        const { error: _error, leaseUntil: _leaseUntil, attempted: _attempted, retryDelay: _retryDelay, ...command } = creation;
        try {
          await this.api.messages(command);
          if (this.closed) return;
          await this.commit(old => ({ ...old, creation: undefined }));
          if (!this.selected) this.selected = command.conversationId;
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
        if (pending.error || this.closed || blockedConversations.has(pending.conversationId)) continue;
        let intent: MessageProjection['outbox'][number] | undefined;
        await this.commit(old => ({ ...old, outbox: old.outbox.map(item => { if (item.changeId !== pending.changeId || item.error || (item.leaseUntil ?? 0) > Date.now()) return item; intent = { ...item, leaseUntil: Date.now() + 30000, attempted: true }; return intent; }) }));
        if (!intent || this.closed) { blockedConversations.add(pending.conversationId); continue; }
        const changeId = intent.changeId;
        try {
          const { error: _error, leaseUntil: _leaseUntil, attempted: _attempted, retryDelay: _retryDelay, ...command } = intent;
          await this.api.messages(command);
          if (this.closed) return;
          await this.commit(old => ({ ...old, outbox: old.outbox.filter(item => item.changeId !== changeId) }));
        } catch (error) {
          await this.commit(old => ({ ...old, outbox: old.outbox.map(item => item.changeId === changeId ? failedCommand(item, error, isPermanentMessageError(error)) : item) }));
          if (!isPermanentMessageError(error)) throw error;
        }
      }
      for (const pending of this.state.answerOutbox ?? []) {
        if (pending.error || this.closed) continue;
        let intent: NonNullable<MessageProjection['answerOutbox']>[number] | undefined;
        await this.commit(old => ({ ...old, answerOutbox: old.answerOutbox?.map(item => {
          if (item.answerId !== pending.answerId || item.error || (item.leaseUntil ?? 0) > Date.now()) return item;
          intent = { ...item, leaseUntil: Date.now() + 30000, attempted: true }; return intent;
        }) }));
        if (!intent || this.closed) continue;
        const answerId = intent.answerId;
        try {
          if (pending.attempted) {
            // Observe this exact historical question, without replacing the active
            // question in the UI. answered=true is not an ACK: another device may
            // have answered differently, so the immutable command still goes to
            // the backend for its answerId/digest receipt check.
            await this.api.messages<TeamBotState>({ action: 'botState', ...this.scope, conversationId: intent.conversationId,
              triggerId: intent.triggerId, questionId: intent.questionId });
            if (this.closed) return;
          }
          const { error: _error, leaseUntil: _leaseUntil, attempted: _attempted, retryDelay: _retryDelay, question: _question, origin: _origin, ...command } = intent;
          await this.api.messages(command);
          if (this.closed) return;
          await this.commit(old => ({ ...old, answerOutbox: old.answerOutbox?.filter(item => item.answerId !== answerId), answerReceipts: [...new Set([...(old.answerReceipts ?? []), JSON.stringify([intent!.conversationId, intent!.questionId])])] }));
        } catch (error) {
          await this.commit(old => ({ ...old, answerOutbox: old.answerOutbox?.map(item => item.answerId === answerId ? failedCommand(item, error, isPermanentMessageError(error)) : item) }));
          if (!isPermanentMessageError(error)) throw error;
        }
      }
      for (const pending of this.state.recoveryOutbox ?? []) {
        if (pending.error || this.closed) continue;
        const key = recoveryKey(pending.conversationId, pending.triggerId);
        let intent: NonNullable<MessageProjection['recoveryOutbox']>[number] | undefined;
        await this.commit(old => ({ ...old, recoveryOutbox: old.recoveryOutbox?.map(item => {
          if (recoveryKey(item.conversationId, item.triggerId) !== key || item.error || (item.leaseUntil ?? 0) > Date.now()) return item;
          intent = { ...item, leaseUntil: Date.now() + 30000, attempted: true }; return intent;
        }) }));
        if (!intent || this.closed) continue;
        try {
          const { error: _error, leaseUntil: _leaseUntil, attempted: _attempted, retryDelay: _retryDelay, ...command } = intent;
          await this.api.messages(command);
          if (this.closed) return;
          await this.commit(old => ({ ...old, recoveryOutbox: old.recoveryOutbox?.filter(item => recoveryKey(item.conversationId, item.triggerId) !== key),
            recoveryReceipts: [...new Set([...(old.recoveryReceipts ?? []), key])] }));
        } catch (error) {
          await this.commit(old => ({ ...old, recoveryOutbox: old.recoveryOutbox?.map(item => recoveryKey(item.conversationId, item.triggerId) === key
            ? failedCommand(item, error, isPermanentRecoveryError(error)) : item) }));
          if (!isPermanentRecoveryError(error)) throw error;
        }
      }
    } catch (error) {
      if (this.closed) return;
      this.again = false;
      this.offline = true; this.error = error instanceof Error ? error.message : 'SERVICE_UNAVAILABLE';
      await this.deny(error);
    }
    this.scheduleRetry(); this.emit();
  }
  close() { this.closed = true; clearTimeout(this.retry); this.stopWatch?.(); this.stopWatch = undefined; this.stops.forEach(stop => stop()); this.stops = []; this.callbacks.clear(); }
}
