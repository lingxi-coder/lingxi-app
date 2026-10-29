export interface TeamConversation {
  id: string; kind: 'dm' | 'group' | 'channel' | 'bot_dm'; title: string; participants: string[];
  lastSeq: number; lastChangeSeq: number; lastReadSeq: number;
}
export interface TeamMessage {
  id: string; seq: number; author: { kind: 'user' | 'bot'; uid: string; botKey?: string };
  kind: string; text: string; replyTo?: string; createdAt: string | null; editedAt?: string | null;
  recalled?: boolean; changeSeq: number; changeId: string;
}
export interface TeamChange { id: string; seq: number; messageId: string; operation: 'create' | 'edit' | 'recall' }
export interface MessagePage { conversation: TeamConversation; changes: TeamChange[]; messages: TeamMessage[]; hasMore: boolean }
export interface MessageScope { teamId: string; generation: number }
export type MessageMutation = MessageScope & { action: 'mutate'; conversationId: string; messageId: string; changeId: string;
  operation: 'create' | 'edit' | 'recall'; text: string; replyTo?: string };
export type MessageCommand = MessageMutation
  | MessageScope & { action: 'list'; after?: string }
  | MessageScope & { action: 'create'; conversationId: string; kind: 'dm' | 'group' | 'channel'; title: string; participantUids: string[] }
  | MessageScope & { action: 'sync'; conversationId: string; after: number }
  | MessageScope & { action: 'read'; conversationId: string; lastReadSeq: number };
export interface MessageApi {
  messages<T>(command: MessageCommand): Promise<T>;
  onMessagesChanged(callback: () => void): () => void;
}
export interface MessageProjection {
  version: 1; uid: string; teamId: string; generation: number;
  conversations: TeamConversation[]; threads: Record<string, { cursor: number; messages: TeamMessage[] }>;
  creation?: Extract<MessageCommand, { action: 'create' }> & { error?: string; leaseUntil?: number; attempted?: boolean };
  outbox: (MessageMutation & { error?: string; leaseUntil?: number; attempted?: boolean })[];
}
export interface MessageStorage {
  /** Reducer and the resulting record commit atomically, including across tabs. */
  update(key: string, reduce: (old: MessageProjection | undefined) => MessageProjection): Promise<MessageProjection>;
}
export const projectionKey = (uid: string, scope: MessageScope) => JSON.stringify([uid, scope.teamId, scope.generation]);
export const emptyProjection = (uid: string, scope: MessageScope): MessageProjection => ({ version: 1, uid, ...scope, conversations: [], threads: {}, outbox: [] });
export function mergeConversation(old: TeamConversation | undefined, value: TeamConversation): TeamConversation {
  return { ...value, lastSeq: Math.max(old?.lastSeq ?? 0, value.lastSeq), lastChangeSeq: Math.max(old?.lastChangeSeq ?? 0, value.lastChangeSeq), lastReadSeq: Math.max(old?.lastReadSeq ?? 0, value.lastReadSeq) };
}
export function applyMessagePage(state: MessageProjection, id: string, page: MessagePage): MessageProjection {
  const previous = state.threads[id] ?? { cursor: 0, messages: [] };
  let cursor = previous.cursor;
  const messages = new Map(previous.messages.map(value => [value.id, value]));
  for (const change of page.changes) {
    if (change.seq <= cursor) continue;
    if (change.seq !== cursor + 1) throw new Error('CHANGE_GAP');
    const value = page.messages.find(value => value.id === change.messageId);
    if (!value || value.changeSeq < change.seq) throw new Error('CHANGE_GAP');
    const old = messages.get(value.id);
    if (!old || old.changeSeq < value.changeSeq) messages.set(value.id, value);
    cursor = change.seq;
  }
  const current = state.conversations.find(value => value.id === id);
  const conversation = mergeConversation(current, page.conversation);
  return { ...state, conversations: [...state.conversations.filter(value => value.id !== id), conversation],
    threads: { ...state.threads, [id]: { cursor, messages: [...messages.values()].sort((a, b) => a.seq - b.seq) } } };
}
const revoked = (error: unknown) => error instanceof Error && ['MEMBERSHIP_REQUIRED', 'STALE_MEMBERSHIP', 'INVALID_SESSION', 'SIGN_IN_REQUIRED', 'CONVERSATION_REQUIRED'].includes(error.message);
const permanent = (error: unknown) => error instanceof Error && ['MESSAGE_ID_CONFLICT', 'MESSAGE_RECALLED', 'MESSAGE_AUTHOR_REQUIRED', 'REPLY_REQUIRED', 'INVALID_INPUT'].includes(error.message);

/** No optimistic sequence numbers. Persist an intent before transport, and keep
 * that exact identity until acknowledged. Only complete change pages move cursor. */
export class TeamMessaging {
  state: MessageProjection;
  selected = '';
  offline = true;
  error = '';
  private closed = false;
  private revoked = false;
  private running?: Promise<void>;
  private again = false;
  private stops: (() => void)[] = [];
  private callbacks = new Set<() => void>();
  private key: string;
  constructor(private api: MessageApi, private storage: MessageStorage, readonly uid: string, readonly scope: MessageScope) {
    this.state = emptyProjection(uid, scope); this.key = projectionKey(uid, scope);
  }
  onChanged(callback: () => void) { this.callbacks.add(callback); return () => { this.callbacks.delete(callback); }; }
  private emit() { if (!this.closed) for (const callback of this.callbacks) callback(); }
  private async commit(reduce: (old: MessageProjection) => MessageProjection) {
    const value = await this.storage.update(this.key, old => this.revoked ? emptyProjection(this.uid, this.scope) : reduce(old?.version === 1 && old.uid === this.uid && old.teamId === this.scope.teamId && old.generation === this.scope.generation ? old : emptyProjection(this.uid, this.scope)));
    if (!this.closed) { this.state = value; this.emit(); }
  }
  async start() {
    await this.commit(old => old);
    if (this.closed) return;
    this.stops.push(this.api.onMessagesChanged(() => void this.refresh()));
    const timer = setInterval(() => void this.refresh(), 15000); this.stops.push(() => clearInterval(timer));
    void this.refresh();
  }
  select(id: string) { this.selected = id; this.error = ''; this.emit(); void this.refresh(); }
  async create(kind: 'dm' | 'group' | 'channel', title: string, participantUids: string[]) {
    if (this.closed) throw new Error('INVALID_SESSION');
    const command: Extract<MessageCommand, { action: 'create' }> = { action: 'create', ...this.scope, conversationId: crypto.randomUUID(), kind, title, participantUids };
    await this.commit(old => { if (old.creation) throw new Error('CREATION_PENDING'); return { ...old, creation: command }; });
    this.selected = command.conversationId; await this.refresh();
  }
  async queue(operation: MessageMutation['operation'], text: string, messageId: string = crypto.randomUUID(), replyTo?: string) {
    if (this.closed || !this.selected || (operation !== 'recall' && (!text.trim() || new TextEncoder().encode(text).length > 8192))) throw new Error('INVALID_INPUT');
    const intent: MessageMutation = { action: 'mutate', ...this.scope, conversationId: this.selected, messageId, changeId: crypto.randomUUID(), operation, text, ...(replyTo ? { replyTo } : {}) };
    await this.commit(old => ({ ...old, outbox: [...old.outbox, intent] }));
    void this.refresh();
  }
  async discard(changeId: string) { await this.commit(old => { if (old.outbox.some(item => item.changeId === changeId && ((item.attempted && !item.error) || (item.leaseUntil ?? 0) > Date.now()))) throw new Error('MESSAGE_IN_FLIGHT'); return { ...old, outbox: old.outbox.filter(item => item.changeId !== changeId) }; }); }
  async discardCreation() { await this.commit(old => { if ((old.creation?.attempted && !old.creation.error) || (old.creation?.leaseUntil ?? 0) > Date.now()) throw new Error('MESSAGE_IN_FLIGHT'); return { ...old, creation: undefined }; }); }
  async markRead() {
    const id = this.selected, thread = this.state.threads[id];
    if (!thread || this.offline || this.closed) return;
    const seq = Math.max(0, ...thread.messages.map(value => value.seq));
    let value: { lastReadSeq: number };
    try { value = await this.api.messages<{ lastReadSeq: number }>({ action: 'read', ...this.scope, conversationId: id, lastReadSeq: seq }); } catch (error) { await this.deny(error); throw error; }
    if (this.closed) return;
    await this.commit(old => ({ ...old, conversations: old.conversations.map(c => c.id === id ? { ...c, lastReadSeq: Math.max(c.lastReadSeq, value.lastReadSeq) } : c) }));
  }
  refresh(): Promise<void> {
    if (this.closed) return Promise.resolve();
    if (this.running) { this.again = true; return this.running; }
    this.running = this.cycle().finally(() => {
      this.running = undefined;
      if (this.again && !this.closed) { this.again = false; void this.refresh(); }
    });
    return this.running;
  }
  private async deny(error: unknown) {
    if (!revoked(error)) return;
    this.offline = true; this.error = (error as Error).message; this.revoked = true;
    this.state = emptyProjection(this.uid, this.scope); this.selected = ''; this.emit(); this.close();
    try { await this.storage.update(this.key, () => emptyProjection(this.uid, this.scope)); } catch { /* The view is cleared even if durable storage has failed. */ }
  }
  private async cycle() {
    try {
      await this.commit(old => old);
      let creation: MessageProjection['creation'];
      await this.commit(old => {
        if (!old.creation || old.creation.error || (old.creation.leaseUntil ?? 0) > Date.now()) return old;
        creation = { ...old.creation, leaseUntil: Date.now() + 30000, attempted: true }; return { ...old, creation };
      });
      if (creation) {
        const { error: _, leaseUntil: __, attempted: ___, ...command } = creation;
        try {
          await this.api.messages(command);
          if (this.closed) return;
          await this.commit(old => ({ ...old, creation: undefined }));
          if (!this.selected) this.selected = command.conversationId;
        } catch (error) {
          if (revoked(error)) throw error;
          const isPermanent = error instanceof Error && ['CONVERSATION_REJECTED', 'PARTICIPANT_REQUIRED', 'INVALID_INPUT'].includes(error.message);
          await this.commit(old => ({ ...old, creation: old.creation ? { ...old.creation, leaseUntil: undefined, ...(isPermanent ? { error: (error as Error).message } : {}) } : undefined }));
          if (!isPermanent) throw error;
        }
      }
      const conversations: TeamConversation[] = [];
      let after: string | undefined;
      do {
        const page = await this.api.messages<{ conversations: TeamConversation[]; next: string | null }>({ action: 'list', ...this.scope, ...(after ? { after } : {}) });
        if (this.closed) return;
        conversations.push(...page.conversations); after = page.next ?? undefined;
      } while (after);
      await this.commit(old => ({ ...old, conversations: conversations.map(value => mergeConversation(old.conversations.find(c => c.id === value.id), value)) }));
      // Reload the durable queue; another window/tab can append while this one waits.
      await this.commit(old => old);
      for (const pending of this.state.outbox) {
        if (pending.error || this.closed) continue;
        let intent: MessageProjection['outbox'][number] | undefined;
        await this.commit(old => ({ ...old, outbox: old.outbox.map(item => { if (item.changeId !== pending.changeId || item.error || (item.leaseUntil ?? 0) > Date.now()) return item; intent = { ...item, leaseUntil: Date.now() + 30000, attempted: true }; return intent; }) }));
        if (!intent || this.closed) continue;
        const changeId = intent.changeId;
        try {
          const { error: _, leaseUntil: __, attempted: ___, ...command } = intent;
          await this.api.messages(command);
          if (this.closed) return;
          await this.commit(old => ({ ...old, outbox: old.outbox.filter(item => item.changeId !== changeId) }));
        } catch (error) {
          await this.commit(old => ({ ...old, outbox: old.outbox.map(item => item.changeId === changeId ? { ...item, leaseUntil: undefined, ...(permanent(error) ? { error: (error as Error).message } : {}) } : item) }));
          if (!permanent(error)) throw error;
        }
      }
      const id = this.selected;
      if (id && !conversations.some(value => value.id === id)) { this.selected = ''; }
      if (id && conversations.some(value => value.id === id)) {
        let more: boolean;
        do {
          const page = await this.api.messages<MessagePage>({ action: 'sync', ...this.scope, conversationId: id, after: this.state.threads[id]?.cursor ?? 0 });
          if (this.closed) return;
          await this.commit(old => applyMessagePage(old, id, page)); more = page.hasMore;
        } while (more && !this.closed);
      }
      this.offline = false; this.error = '';
    } catch (error) {
      if (this.closed) return;
      this.offline = true; this.error = error instanceof Error ? error.message : 'SERVICE_UNAVAILABLE';
      await this.deny(error);
    }
    this.emit();
  }
  close() { this.closed = true; this.stops.forEach(stop => stop()); this.stops = []; this.callbacks.clear(); }
}
