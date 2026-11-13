import type { Firestore, DocumentData, Query, QuerySnapshot } from 'firebase/firestore';
import { ATTACHMENT_FILE_BYTES, ATTACHMENT_MESSAGE_FILES, ATTACHMENT_TYPES, type MessageAttachment } from './attachments.js';

export type ChatSourceAuthor = { kind: 'user'; uid: string; displayName: string }
  | { kind: 'shared_bot'; sharedBotId: string; displayName: string };
export interface ChatSourceMessage {
  messageId: string; seq: number; changeSeq: number; changeId: string; author: ChatSourceAuthor;
  createdAt: number; editedAt?: number; text: string; replyTo?: string; recalled?: boolean;
  sourceTaskId?: string; deliveryId?: string; artifactId?: string; waitId?: string;
  /** The files the message carries, as it names them. Their bytes are read through the service, one file at a time. */
  attachments?: MessageAttachment[];
}
export type ChatReadRequest = { teamId: string; generation: number; conversationId: string } & (
  | { mode: 'recent'; throughSeq: number }
  | { mode: 'range'; fromSeqExclusive: number; toSeqInclusive: number }
  | { mode: 'before'; beforeSeq: number; limit?: number }
  | { mode: 'byId'; messageId: string }
  | { mode: 'changes'; afterChangeSeq: number; throughMessageSeq: number }
);
export interface ChatReadResult { messages: ChatSourceMessage[]; lastSeq: number; lastChangeSeq: number; hasMore: boolean }
const id = (value: unknown): value is string => typeof value === 'string' && /^[A-Za-z0-9_-]{8,64}$/u.test(value);
const counter = (value: unknown, min = 0): value is number => typeof value === 'number' && Number.isSafeInteger(value) && value >= min;
const fail = (code: string): never => { throw new Error(code); };
function millis(value: unknown): number {
  if (!value || typeof value !== 'object' || !('toMillis' in value) || typeof value.toMillis !== 'function') return fail('INVALID_SOURCE_TIME');
  const result: unknown = value.toMillis();
  return counter(result) ? result : fail('INVALID_SOURCE_TIME');
}
function attachments(value: unknown): MessageAttachment[] {
  if (!Array.isArray(value) || !value.length || value.length > ATTACHMENT_MESSAGE_FILES) return fail('INVALID_DATA');
  return value.map((item: unknown) => {
    const file = item && typeof item === 'object' ? item as Record<string, unknown> : fail('INVALID_DATA');
    const { attachmentId, name, mime, byteLength, sha256, revoked } = file;
    if (!id(attachmentId) || typeof name !== 'string' || !name || name.length > 120 || /[\u0000-\u001f\u007f]/u.test(name)
      || !ATTACHMENT_TYPES.some(type => type === mime) || !counter(byteLength, 1) || byteLength > ATTACHMENT_FILE_BYTES
      || typeof sha256 !== 'string' || !/^[a-f0-9]{64}$/u.test(sha256) || (revoked !== undefined && typeof revoked !== 'boolean')) return fail('INVALID_DATA');
    return { attachmentId, name, mime: mime as MessageAttachment['mime'], byteLength, sha256, ...(revoked !== undefined ? { revoked } : {}) };
  });
}
export function chatSourceMessage(messageId: string, data: DocumentData): ChatSourceMessage {
  if (!id(messageId) || !counter(data.seq, 1) || !counter(data.changeSeq, 1) || !id(data.changeId)
    || typeof data.text !== 'string' || (data.replyTo !== undefined && !id(data.replyTo))
    || (data.recalled !== undefined && typeof data.recalled !== 'boolean') || (data.recalled === true && data.text !== '')) return fail('INVALID_DATA');
  const by: unknown = data.author;
  if (!by || typeof by !== 'object' || !('kind' in by)) return fail('INVALID_DATA');
  // Human-only Chats are retained business data. Their stable UID is always a
  // truthful display label when a source message has no captured display name.
  const label = 'displayName' in by ? by.displayName : by.kind === 'user' && 'uid' in by ? by.uid : undefined;
  if (typeof label !== 'string' || !label || label.length > 128) return fail('INVALID_DATA');
  let author: ChatSourceAuthor;
  if (by.kind === 'user' && 'uid' in by && typeof by.uid === 'string' && /^[A-Za-z0-9_-]{1,128}$/u.test(by.uid)) author = { kind: 'user', uid: by.uid, displayName: label.slice(0, 80) };
  else if (by.kind === 'shared_bot' && 'sharedBotId' in by && id(by.sharedBotId)) author = { kind: 'shared_bot', sharedBotId: by.sharedBotId, displayName: label.slice(0, 80) };
  else return fail('INVALID_DATA');
  return { messageId, seq: data.seq, changeSeq: data.changeSeq, changeId: data.changeId, text: data.text, author,
    createdAt: millis(data.createdAt), ...(data.editedAt !== undefined ? { editedAt: millis(data.editedAt) } : {}),
    ...(data.replyTo !== undefined ? { replyTo: data.replyTo } : {}), ...(data.recalled !== undefined ? { recalled: data.recalled } : {}),
    // A recalled message gave its files up with its text.
    ...(data.attachments !== undefined && data.recalled !== true ? { attachments: attachments(data.attachments) } : {}),
    // A Bot's message names its task as `taskId`; the human answer to a Bot's question stores it as `sourceTaskId`.
    ...(data.taskId !== undefined ? { sourceTaskId: id(data.taskId) ? data.taskId : fail('INVALID_DATA') }
      : data.sourceTaskId !== undefined ? { sourceTaskId: id(data.sourceTaskId) ? data.sourceTaskId : fail('INVALID_DATA') } : {}),
    ...(data.waitId !== undefined ? { waitId: id(data.waitId) ? data.waitId : fail('INVALID_DATA') } : {}),
    ...(data.deliveryId !== undefined ? { deliveryId: id(data.deliveryId) ? data.deliveryId : fail('INVALID_DATA') } : {}),
    ...(data.artifactId !== undefined ? { artifactId: id(data.artifactId) ? data.artifactId : fail('INVALID_DATA') } : {}) };
}

/** One instance per existing UID/project SDK session, shared by UI and all Bots.
 * Stores only in-flight promises and verification watermarks; Firestore owns the
 * message cache. Current server ACL is required even for a cache hit. */
export function createChatSourceReader(sdk: typeof import('firebase/firestore'), db: Firestore, uid: string, isCurrent: () => boolean) {
  const inflight = new Map<string, Promise<ChatReadResult>>();
  const verified = new Map<string, { watermark: number; count: number }>();
  let closed = false;
  const assertCurrent = () => { if (closed || !isCurrent()) fail('INVALID_SESSION'); };
  async function authorize(request: ChatReadRequest) {
    assertCurrent();
    const root = ['teams', request.teamId] as const;
    const [member, chat] = await Promise.all([
      sdk.getDocFromServer(sdk.doc(db, ...root, 'members', uid)),
      sdk.getDocFromServer(sdk.doc(db, ...root, 'conversations', request.conversationId)),
    ]);
    assertCurrent();
    if (member.data()?.generation !== request.generation) fail('STALE_MEMBERSHIP');
    const value = chat.data();
    if (!value || !Array.isArray(value.memberUids) || !value.memberUids.includes(uid)) return fail('CONVERSATION_REQUIRED');
    if (!counter(value.lastSeq) || !counter(value.lastChangeSeq)) fail('INVALID_DATA');
    return value as { lastSeq: number; lastChangeSeq: number };
  }
  async function queryPage(query: Query<DocumentData>, key: string, watermark: number): Promise<QuerySnapshot<DocumentData>> {
    const proof = verified.get(key);
    if (proof?.watermark === watermark) {
      try { const cached = await sdk.getDocsFromCache(query); assertCurrent(); if (cached.size === proof.count) return cached; } catch { assertCurrent(); }
    }
    const result = await sdk.getDocsFromServer(query); assertCurrent();
    verified.set(key, { watermark, count: result.size });
    // This is a bounded proof cache, never a source-message cache.
    if (verified.size > 1024) verified.delete(verified.keys().next().value!);
    return result;
  }
  async function perform(request: ChatReadRequest): Promise<ChatReadResult> {
    const before = await authorize(request), path = ['teams', request.teamId, 'conversations', request.conversationId, 'messages'] as const;
    let messages: ChatSourceMessage[] = [], hasMore = false;
    if (request.mode === 'changes') {
      if (request.afterChangeSeq > before.lastChangeSeq || request.throughMessageSeq > before.lastSeq) fail('INVALID_READ_CURSOR');
      const ids = new Set<string>();
      let changeCursor = request.afterChangeSeq;
      while (changeCursor < before.lastChangeSeq) {
        const size = Math.min(200, before.lastChangeSeq - changeCursor);
        const query = sdk.query(sdk.collection(db, 'teams', request.teamId, 'conversations', request.conversationId, 'changes'),
          sdk.where('seq', '>', changeCursor), sdk.where('seq', '<=', before.lastChangeSeq), sdk.orderBy('seq'), sdk.limit(size));
        const key = JSON.stringify([request, changeCursor, size]);
        let page = await queryPage(query, key, before.lastChangeSeq);
        if (page.size !== size || page.docs.some((doc, index) => doc.data().seq !== changeCursor + index + 1)) {
          verified.delete(key); page = await sdk.getDocsFromServer(query); assertCurrent();
        }
        if (page.size !== size || page.docs.some((doc, index) => doc.data().seq !== changeCursor + index + 1)) fail('CHANGE_GAP');
        for (const doc of page.docs) { if (!id(doc.data().messageId)) fail('INVALID_DATA'); ids.add(doc.data().messageId); }
        changeCursor += size;
      }
      const pending = [...ids]; let next = 0;
      const values = await Promise.all(Array.from({ length: Math.min(8, pending.length) }, async () => {
        const loaded: ChatSourceMessage[] = [];
        while (next < pending.length) {
          const messageId = pending[next++]!;
          const doc = await sdk.getDocFromServer(sdk.doc(db, ...path, messageId)); assertCurrent();
          if (!doc.exists()) fail('MESSAGE_GAP');
          const message = chatSourceMessage(doc.id, doc.data()!);
          if (message.seq <= request.throughMessageSeq) loaded.push(message);
        }
        return loaded;
      }));
      messages = values.flat().sort((a, b) => a.seq - b.seq);
    } else if (request.mode === 'byId') {
      const doc = await sdk.getDocFromServer(sdk.doc(db, ...path, request.messageId)); assertCurrent();
      if (doc.exists()) messages = [chatSourceMessage(doc.id, doc.data())];
    } else if (request.mode === 'before') {
      const size = request.limit ?? 200;
      const query = sdk.query(sdk.collection(db, ...path), sdk.where('seq', '<', request.beforeSeq), sdk.orderBy('seq', 'desc'), sdk.limit(size + 1));
      const page = await queryPage(query, JSON.stringify([request, 'before']), before.lastChangeSeq);
      hasMore = page.size > size;
      messages = page.docs.slice(0, size).map(doc => chatSourceMessage(doc.id, doc.data())).reverse();
    } else {
      const to = request.mode === 'recent' ? request.throughSeq : request.toSeqInclusive;
      const from = request.mode === 'recent' ? Math.max(0, to - 200) : request.fromSeqExclusive;
      if (to > before.lastSeq || to <= from) fail('INVALID_READ_CURSOR');
      let cursor = from;
      while (cursor < to) {
        assertCurrent();
        const size = Math.min(200, to - cursor);
        const query = sdk.query(sdk.collection(db, ...path), sdk.where('seq', '>', cursor), sdk.where('seq', '<=', to), sdk.orderBy('seq'), sdk.limit(size));
        const key = JSON.stringify([request.teamId, request.conversationId, cursor, to, size]);
        let page = await queryPage(query, key, before.lastChangeSeq);
        // Eviction may leave a partial cache page. Exact sequence coverage is
        // required, so refill through the same SDK instead of accepting a gap.
        if (page.size !== size || page.docs.some((doc, index) => doc.data().seq !== cursor + index + 1)) {
          verified.delete(key); page = await sdk.getDocsFromServer(query); assertCurrent();
        }
        const values = page.docs.map(doc => chatSourceMessage(doc.id, doc.data()));
        if (values.length !== size || values.some((message, index) => message.seq !== cursor + index + 1)) fail('MESSAGE_GAP');
        messages.push(...values); cursor = values.at(-1)!.seq;
      }
      hasMore = from > 0;
    }
    const after = await authorize(request);
    // Every accepted create advances both counters; edits and recalls advance
    // only changeSeq. Later appends cannot change this captured input window.
    const appended = after.lastSeq - before.lastSeq;
    if (after.lastChangeSeq !== before.lastChangeSeq
      && !(appended > 0 && after.lastChangeSeq - before.lastChangeSeq === appended)) fail('CHAT_CHANGED');
    if (messages.some(message => message.changeSeq > before.lastChangeSeq)) fail('INVALID_DATA');
    return { messages, lastSeq: before.lastSeq, lastChangeSeq: before.lastChangeSeq, hasMore };
  }
  return {
    read(request: ChatReadRequest): Promise<ChatReadResult> {
      assertCurrent();
      if (!id(request.teamId) || !id(request.conversationId) || !counter(request.generation, 1)
        || (request.mode === 'recent' && !counter(request.throughSeq, 1))
        || (request.mode === 'range' && (!counter(request.fromSeqExclusive) || !counter(request.toSeqInclusive, 1)))
        || (request.mode === 'before' && (!counter(request.beforeSeq, 1) || (request.limit !== undefined && (!counter(request.limit, 1) || request.limit > 200))))
        || (request.mode === 'changes' && (!counter(request.afterChangeSeq) || !counter(request.throughMessageSeq, 1)))
        || (request.mode === 'byId' && !id(request.messageId)) || !['recent', 'range', 'before', 'byId', 'changes'].includes(request.mode)) return Promise.reject(new Error('INVALID_INPUT'));
      const key = JSON.stringify(request), pending = inflight.get(key);
      if (pending) return pending.then(value => structuredClone(value));
      const promise = perform(request).finally(() => { inflight.delete(key); });
      inflight.set(key, promise);
      return promise.then(value => structuredClone(value));
    },
    close() { closed = true; verified.clear(); inflight.clear(); },
  };
}
