import type { DocumentData, Firestore, FirestoreError } from 'firebase/firestore';
import { ATTACHMENT_TYPES, type MessageAttachment } from './attachments.js';
import type { BlockEntry, FriendCard, TeamOffer } from './friends.js';
import type { ListApproval, ListApprovalChange, MessageScope, MessageSnapshot, TeamConversation, TeamMessage } from './messages.js';

function timestamp(value: unknown): string | null {
  if (value && typeof value === 'object' && 'toDate' in value && typeof value.toDate === 'function') return value.toDate().toISOString();
  return null;
}

/** A stored document that breaks the contract the rules and the trusted backend enforce; the watcher reports it as `INVALID_DATA`. */
const invalidData = () => new Error('INVALID_DATA');
const record = (value: unknown): Record<string, unknown> => { if (value && typeof value === 'object' && !Array.isArray(value)) return value as Record<string, unknown>; throw invalidData(); };
const text = (value: unknown): string => { if (typeof value === 'string' && value) return value; throw invalidData(); };
type Pointer = NonNullable<TeamConversation['lastMessage']>;

function pointerAuthor(value: unknown): Pointer['author'] {
  const by = record(value);
  if (by.kind === 'user') return { kind: 'user', uid: text(by.uid) };
  if (by.kind === 'shared_bot') return { kind: 'shared_bot', sharedBotId: text(by.sharedBotId), displayName: text(by.displayName) };
  throw invalidData();
}

/**
 * Conversation `lastMessage` pointer -> list DTO. `null` is a conversation with no message yet. Any other value must be the whole pointer
 * `{ id, seq, at, author, preview, recalled? }` that the rules and the backend write, or this throws `INVALID_DATA`. `at` is `null` only
 * while the server has not yet stamped a write made on this device.
 */
export function firestoreLastMessage(value: unknown): TeamConversation['lastMessage'] {
  if (value === null) return undefined;
  const { id, seq, at, author, preview, recalled } = record(value);
  const time = at === null ? null : timestamp(at);
  if (typeof seq !== 'number' || !Number.isSafeInteger(seq) || (at !== null && time === null) || typeof preview !== 'string' || (recalled !== undefined && recalled !== true)) throw invalidData();
  return { id: text(id), seq, at: time, author: pointerAuthor(author), preview, ...(recalled ? { recalled: true } : {}) };
}

const count = (value: unknown): number | undefined => typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 ? value : undefined;
const nonEmpty = (value: unknown): string | undefined => typeof value === 'string' && value ? value : undefined;
const defined = <T extends object>(fields: T): T => Object.fromEntries(Object.entries(fields).filter(([, value]) => value !== undefined)) as T;
const millis = (value: unknown): number => { const iso = timestamp(value); return iso ? Date.parse(iso) : 0; };

/**
 * `users/{uid}/friends/{other}` -> card with plain values, or `undefined` for a document that breaks the contract: one bad card
 * never fails the list. A `lastMessage` that is not the whole pointer counts as absent; a counter that is not a count, as missing.
 */
export function firestoreFriendCard(uid: string, value: DocumentData): FriendCard | undefined {
  if (!['friends', 'incoming', 'outgoing'].includes(value.state) || typeof value.displayName !== 'string') return undefined;
  let lastMessage: TeamConversation['lastMessage'];
  try { lastMessage = firestoreLastMessage(value.lastMessage ?? null); } catch { /* Not the whole pointer: the card has no message. */ }
  return defined({ uid, state: value.state, displayName: value.displayName, handle: nonEmpty(value.handle), note: nonEmpty(value.note), since: millis(value.since),
    teamId: nonEmpty(value.teamId), generation: count(value.generation) || undefined, lastSeq: count(value.lastSeq), lastReadSeq: count(value.lastReadSeq),
    lastChangeSeq: count(value.lastChangeSeq), lastMessage });
}

/** `users/{uid}/blocks/{other}` -> entry. The name may be missing on an entry made before names were kept. */
export function firestoreBlock(uid: string, value: DocumentData): BlockEntry {
  return defined({ uid, at: millis(value.at), displayName: typeof value.displayName === 'string' ? value.displayName : '', handle: nonEmpty(value.handle) });
}

/**
 * `users/{uid}/teamOffers/{teamId}` -> offer with `expiresAt` in epoch milliseconds, or `undefined` for a document without the four
 * strings or a readable `expiresAt` (D27): one bad offer never fails the list. The document id is the team id.
 */
export function firestoreTeamOffer(id: string, value: DocumentData): TeamOffer | undefined {
  const { teamId, teamName, fromUid, fromName } = value, expires = timestamp(value.expiresAt);
  if (![teamId, teamName, fromUid, fromName].every(field => typeof field === 'string') || !expires) return undefined;
  return { teamId: id, teamName, fromUid, fromName, role: 'member', expiresAt: Date.parse(expires) };
}

/** `profiles/{uid}` -> the two fields a friend can change (D10), or `null` when the profile cannot be read. */
export function firestoreFriendName(value: DocumentData | undefined): { displayName?: string; handle?: string } | null {
  return value ? defined({ displayName: typeof value.displayName === 'string' ? value.displayName : undefined, handle: nonEmpty(value.handle) }) : null;
}

export function firestoreMessage(id: string, value: DocumentData): TeamMessage {
  return { id, seq: value.seq, author: pointerAuthor(value.author), kind: value.kind, text: value.text, changeSeq: value.changeSeq, changeId: value.changeId,
    createdAt: timestamp(value.createdAt), editedAt: timestamp(value.editedAt), ...(value.replyTo ? { replyTo: value.replyTo } : {}),
    ...(value.recalled ? { recalled: true } : {}), ...(value.mentions ? { mentions: value.mentions } : {}),
    ...(value.taskId ? { taskId: value.taskId } : {}), ...(value.taskIds ? { taskIds: value.taskIds } : {}), ...(value.artifactId ? { artifactId: value.artifactId } : {}),
    ...(value.waitId ? { waitId: value.waitId } : {}),
    ...(approvalOf(value.approval) ? { approval: approvalOf(value.approval)! } : {}),
    ...(value.decision === 'approve' || value.decision === 'deny' ? { decision: value.decision } : {}),
    ...(attachmentsOf(value.attachments).length ? { attachments: attachmentsOf(value.attachments) } : {}) };
}
/** The files of a message. An entry that does not read as a file is left out; the others stand. */
function attachmentsOf(value: unknown): MessageAttachment[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap(item => item && typeof item === 'object' && typeof item.attachmentId === 'string' && typeof item.name === 'string'
    && (ATTACHMENT_TYPES as readonly string[]).includes(item.mime) && Number.isSafeInteger(item.byteLength) && item.byteLength > 0 && typeof item.sha256 === 'string'
    ? [{ attachmentId: item.attachmentId, name: item.name, mime: item.mime, byteLength: item.byteLength, sha256: item.sha256, ...(item.revoked === true ? { revoked: true } : {}) }] : []);
}
/** The approval request on a Bot's message. A request that does not read as one is not shown as one. */
function approvalOf(value: unknown): ListApproval | undefined {
  if (!value || typeof value !== 'object') return undefined;
  const { title, changes } = value as { title?: unknown; changes?: unknown };
  if (typeof title !== 'string' || !Array.isArray(changes) || !changes.length) return undefined;
  const read: ListApprovalChange[] = [];
  for (const change of changes as Array<Record<string, unknown>>) {
    if (!change || typeof change !== 'object' || !['add', 'update', 'remove'].includes(change.op as string)) return undefined;
    if (change.op === 'add' && typeof change.text !== 'string') return undefined;
    read.push({ op: change.op as ListApprovalChange['op'], ...(typeof change.text === 'string' ? { text: change.text } : {}), ...(typeof change.itemId === 'string' ? { itemId: change.itemId } : {}),
      ...(typeof change.done === 'boolean' ? { done: change.done } : {}), ...(change.op === 'add' ? {} : { item: typeof change.item === 'string' ? change.item : null }) });
  }
  return { title, changes: read };
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
        try {
          emit({ kind: 'messages', conversationId, fromCache: page.metadata.fromCache, replace: firstSnapshot,
            messages: changes.filter(change => change.type !== 'removed').map(change => firestoreMessage(change.doc.id, change.doc.data())),
            removed: changes.filter(change => change.type === 'removed').map(change => change.doc.id) });
          firstSnapshot = false;
        } catch { emit({ kind: 'error', fromCache: page.metadata.fromCache, error: 'INVALID_DATA' }); close(); }
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
      let conversations: TeamConversation[];
      try {
        conversations = snapshot.docs.map(snapshot => {
          const value = snapshot.data(), lastMessage = firestoreLastMessage(value.lastMessage);
          if (!Array.isArray(value.participants) || !value.participants.every((principal: unknown) => typeof principal === 'string' && /^(user:[A-Za-z0-9_-]{1,128}|shared_bot:[A-Za-z0-9_-]{8,64})$/u.test(principal))) throw invalidData();
          if (value.kind === 'bot_dm' && value.participants.filter((principal: string) => principal.startsWith('shared_bot:')).length !== 1) throw invalidData();
          const readers = Array.isArray(value.memberUids) && value.memberUids.every((reader: unknown) => typeof reader === 'string') ? { readers: value.memberUids as string[] } : {};
          return { id: snapshot.id, kind: value.kind, title: value.title, participants: value.participants, ...readers, lastSeq: value.lastSeq, lastChangeSeq: value.lastChangeSeq, lastReadSeq: readSeq.get(snapshot.id) ?? 0, ...(lastMessage ? { lastMessage } : {}) };
        });
      } catch {
        // A conversation that breaks the stored contract is not shown half-parsed; the team's listeners stop with the error.
        emit({ kind: 'error', fromCache: snapshot.metadata.fromCache, error: 'INVALID_DATA' });
        close();
        return;
      }
      const allowed = new Set(snapshot.docs.map(value => value.id));
      for (const [id, stop] of reads) if (!allowed.has(id)) { stop(); reads.delete(id); readSeq.delete(id); }
      directory = { fromCache: snapshot.metadata.fromCache, conversations };
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
      if (authorized && !snapshot.metadata.fromCache && conversationId && snapshot.docChanges().some(change => change.doc.id === conversationId && change.doc.data().participants?.some((value: string) => value.startsWith('shared_bot:')))) {
        emit({ kind: 'signal', fromCache: false, signal: { conversationId } });
      }
    }, fail));
  // Subscribe only to sanitized invalidation stamps; task state uses authenticated commands.
  stops.push(onSnapshot(collection(db, ...root, 'signals'), { includeMetadataChanges: true }, snapshot => {
    if (authorized && !snapshot.metadata.fromCache) emit({ kind: 'signal', fromCache: false, signal: { bots: true } });
  }, fail));
  return close;
}
