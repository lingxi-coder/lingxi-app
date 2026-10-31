import type { DocumentData, Firestore, FirestoreError } from 'firebase/firestore';
import type { MessageScope, MessageSnapshot, TeamConversation, TeamMessage } from './messages.js';

function timestamp(value: unknown): string | null {
  if (value && typeof value === 'object' && 'toDate' in value && typeof value.toDate === 'function') return value.toDate().toISOString();
  return null;
}

export function firestoreMessage(id: string, value: DocumentData): TeamMessage {
  return { id, seq: value.seq, author: value.author, kind: value.kind, text: value.text, changeSeq: value.changeSeq, changeId: value.changeId,
    createdAt: timestamp(value.createdAt), editedAt: timestamp(value.editedAt), ...(value.replyTo ? { replyTo: value.replyTo } : {}),
    ...(value.recalled ? { recalled: true } : {}), ...(value.mentions ? { mentions: value.mentions } : {}),
    ...(value.triggerId ? { triggerId: value.triggerId } : {}), ...(value.origin ? { origin: value.origin } : {}), ...(value.question ? { question: value.question } : {}) };
}

/** SDK snapshots are the only ordinary-message read path, including offline reads. */
export function watchFirestoreMessages(sdk: typeof import('firebase/firestore'), db: Firestore, uid: string, scope: MessageScope, conversationId: string,
  next: (snapshot: MessageSnapshot) => void, isCurrent: () => boolean): () => void {
  const { collection, doc, onSnapshot, orderBy, query, where } = sdk;
  let closed = false;
  const stops: (() => void)[] = [];
  const reads = new Map<string, () => void>();
  const readSeq = new Map<string, number>();
  let authorized = false;
  let directory: { conversations: TeamConversation[]; fromCache: boolean } | undefined;
  let stopMessages: (() => void) | undefined;
  let selectedAllowed = false;
  const root = ['teams', scope.teamId] as const;
  type SnapshotValue = MessageSnapshot extends infer Value ? Value extends MessageSnapshot ? Omit<Value, keyof MessageScope | 'uid'> : never : never;
  function emit(value: SnapshotValue): void {
    if (closed) return;
    if (!isCurrent()) { next({ uid, ...scope, kind: 'error', fromCache: true, error: 'INVALID_SESSION' }); close(); return; }
    next({ uid, ...scope, ...value } as MessageSnapshot);
  }
  function fail(error: FirestoreError): void {
    if (closed) return;
    emit({ kind: 'error', fromCache: true, error: error.code === 'permission-denied' || error.code === 'unauthenticated' ? 'MEMBERSHIP_REQUIRED' : 'MESSAGE_LISTENER_FAILED' });
    close();
  }
  function publish(): void {
    if (!authorized || closed) return;
    if (directory) emit({ kind: 'directory', ...directory, conversations: directory.conversations.map(value => ({ ...value, lastReadSeq: readSeq.get(value.id) ?? 0 })) });
    watchSelected();
  }
  function watchSelected(): void {
    if (!authorized || closed || !selectedAllowed || stopMessages) return;
    let firstSnapshot = true;
    stopMessages = onSnapshot(query(collection(db, ...root, 'conversations', conversationId, 'messages'), orderBy('seq')),
      { includeMetadataChanges: true }, page => {
        if (closed || !selectedAllowed) return;
        const changes = page.docChanges();
        emit({ kind: 'messages', conversationId, fromCache: page.metadata.fromCache, replace: firstSnapshot,
          messages: changes.filter(change => change.type !== 'removed').map(change => firestoreMessage(change.doc.id, change.doc.data())),
          removed: changes.filter(change => change.type === 'removed').map(change => change.doc.id) });
        firstSnapshot = false;
      }, fail);
  }
  function close(): void {
    closed = true; stops.forEach(stop => stop()); reads.forEach(stop => stop()); reads.clear(); stopMessages?.();
  }
  stops.push(onSnapshot(doc(db, ...root, 'members', uid), { includeMetadataChanges: true }, snapshot => {
    if (closed) return;
    if (snapshot.data()?.generation === scope.generation) { authorized = true; publish(); }
    else if (authorized || !snapshot.metadata.fromCache) { emit({ kind: 'error', fromCache: snapshot.metadata.fromCache, error: 'STALE_MEMBERSHIP' }); close(); }
  }, fail));
  stops.push(onSnapshot(query(collection(db, ...root, 'conversations'), where('memberUids', 'array-contains', uid)),
    { includeMetadataChanges: true }, snapshot => {
      if (closed) return;
      const allowed = new Set(snapshot.docs.map(value => value.id));
      for (const [id, stop] of reads) if (!allowed.has(id)) { stop(); reads.delete(id); readSeq.delete(id); }
      directory = { fromCache: snapshot.metadata.fromCache, conversations: snapshot.docs.map(snapshot => {
        const value = snapshot.data();
        return { id: snapshot.id, kind: value.kind, title: value.title, participants: value.participants, lastSeq: value.lastSeq, lastChangeSeq: value.lastChangeSeq, lastReadSeq: readSeq.get(snapshot.id) ?? 0, ...(value.botKey ? { botKey: value.botKey } : {}) };
      }) };
      for (const id of allowed) if (!reads.has(id)) reads.set(id, onSnapshot(doc(db, ...root, 'conversations', id, 'reads', uid),
        { includeMetadataChanges: true }, mark => {
          if (closed) return;
          const value = mark.data(), lastReadSeq = value?.generation === scope.generation ? value.lastReadSeq : 0;
          if ((readSeq.get(id) ?? 0) === lastReadSeq) return;
          readSeq.set(id, lastReadSeq);
          if (authorized) emit({ kind: 'read', fromCache: mark.metadata.fromCache, conversationId: id, lastReadSeq });
        }, fail));
      const wasAllowed = selectedAllowed;
      selectedAllowed = !!conversationId && allowed.has(conversationId);
      if (!selectedAllowed) { stopMessages?.(); stopMessages = undefined; }
      if (!wasAllowed) watchSelected();
      publish();
      if (authorized && !snapshot.metadata.fromCache && conversationId && snapshot.docChanges().some(change => change.doc.id === conversationId && change.doc.data().participants?.some((value: string) => value.startsWith('bot:')))) {
        emit({ kind: 'signal', fromCache: false, signal: { conversationId } });
      }
    }, fail));
  // Sanitized invalidation stamps only. Private Bot run/question documents stay denied.
  stops.push(onSnapshot(collection(db, ...root, 'signals'), { includeMetadataChanges: true }, snapshot => {
    if (authorized && !snapshot.metadata.fromCache) emit({ kind: 'signal', fromCache: false, signal: { bots: true } });
  }, fail));
  return close;
}
