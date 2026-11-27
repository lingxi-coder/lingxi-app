import { parseMentionEntities } from './mentions.js';
import { isResourceRef } from './dataset-resource.js';
import { parseDataRef } from './lingxi-data.js';
import { parseVisualization } from './visualization.js';
import type { DocumentData, Firestore, FirestoreError } from 'firebase/firestore';
import { ATTACHMENT_TYPES, type MessageAttachment } from './attachments.js';
import { FRIEND_CHAT, type BlockEntry, type FriendCard, type TeamOffer } from './friends.js';
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
  if (by.kind === 'task') return { kind: 'task', taskId: text(by.taskId), logicalRun: text(by.logicalRun), displayName: text(by.displayName) };
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
  return defined({ uid, state: value.state, displayName: value.displayName, handle: nonEmpty(value.handle), note: nonEmpty(value.note), since: millis(value.since), version: nonEmpty(value.version),
    teamId: nonEmpty(value.teamId), generation: count(value.generation) || undefined, lastSeq: count(value.lastSeq), lastReadSeq: count(value.lastReadSeq),
    lastChangeSeq: count(value.lastChangeSeq),
    // Older counts may have come from a Bot DM. The chat's own sequence and read mark are the fallback.
    unreadCount: value.unreadConversationId === FRIEND_CHAT ? count(value.unreadCount) : undefined, lastMessage });
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
  return defined<TeamOffer>({ teamId: id, teamName, fromUid, fromName, role: 'member', expiresAt: Date.parse(expires), version: nonEmpty(value.version) });
}

/** `profiles/{uid}` -> the two fields a friend can change (D10), or `null` when the profile cannot be read. */
export function firestoreFriendName(value: DocumentData | undefined): { displayName?: string; handle?: string } | null {
  return value ? defined({ displayName: typeof value.displayName === 'string' ? value.displayName : undefined, handle: nonEmpty(value.handle) }) : null;
}

export function firestoreMessage(id: string, value: DocumentData): TeamMessage {
  const mentionEntities = value.recalled ? [] : parseMentionEntities(value.text, value.mentionEntities);
  const dataRef = value.recalled ? undefined : parseDataRef(value.dataRef);
  const resourceRef = !value.recalled && isResourceRef(value.resourceRef) ? value.resourceRef : undefined;
  const visualization = value.kind === 'visualization' && !value.recalled ? parseVisualization(value.visualization, id) : undefined;
  return { id, seq: value.seq, author: pointerAuthor(value.author), kind: value.kind, text: value.text, changeSeq: value.changeSeq, changeId: value.changeId,
    createdAt: timestamp(value.createdAt), editedAt: timestamp(value.editedAt), ...(value.replyTo ? { replyTo: value.replyTo } : {}),
    ...(value.recalled ? { recalled: true } : {}), ...(value.mentions ? { mentions: value.mentions } : {}), ...(mentionEntities.length ? { mentionEntities } : {}),
    ...(value.taskId ? { taskId: value.taskId } : {}), ...(value.taskIds ? { taskIds: value.taskIds } : {}), ...(value.artifactId ? { artifactId: value.artifactId } : {}),
    ...(value.waitId ? { waitId: value.waitId } : {}),
    ...(approvalOf(value.approval) ? { approval: approvalOf(value.approval)! } : {}),
    ...(value.decision === 'approve' || value.decision === 'deny' ? { decision: value.decision } : {}),
    ...(attachmentsOf(value.attachments).length ? { attachments: attachmentsOf(value.attachments) } : {}),
    ...(dataRef ? { dataRef } : {}),
    ...(resourceRef ? { resourceRef, resourceGrant: nonEmpty(value.resourceGrant) } : {}),
    ...(visualization ? { visualization } : {}),
    ...(readFilesOf(value.readFiles).length ? { readFiles: readFilesOf(value.readFiles) } : {}) };
}
function readFilesOf(value: unknown): { attachmentId: string; name: string }[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap(item => item && typeof item === 'object' && typeof item.attachmentId === 'string' && typeof item.name === 'string' && item.name
    ? [{ attachmentId: item.attachmentId, name: item.name }] : []);
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

const AUTHORITY_LEASE_MS=5*60_000;
function membershipLease(uid:string,scope:MessageScope,verifiedAt?:number) {
  const key='lingxi.membershipLease.'+JSON.stringify([uid,scope.teamId,scope.generation]);
  try{
    if(verifiedAt!==undefined){if(verifiedAt)globalThis.localStorage?.setItem(key,String(verifiedAt));else globalThis.localStorage?.removeItem(key);}
    return Number(globalThis.localStorage?.getItem(key)??0);
  }catch{return 0;}
}

/** One history read. beforeSeq is exclusive; change-log snapshots independently handle edits to returned rows. */
export async function readFirestoreMessagePage(sdk:typeof import('firebase/firestore'),db:Firestore,scope:MessageScope,conversationId:string,beforeSeq:number,uid?:string):Promise<TeamMessage[]> {
  if(![scope.teamId,conversationId].every(id=>/^[A-Za-z0-9_-]{1,128}$/u.test(id)) || !Number.isSafeInteger(beforeSeq) || beforeSeq<=0) throw invalidData();
  const member=uid?sdk.doc(db,'teams',scope.teamId,'members',uid):undefined;
  const confirm=async()=>{
    if(!member || !uid)return;
    const snap=await sdk.getDoc(member);
    if(snap.data()?.generation!==scope.generation)throw new Error('STALE_MEMBERSHIP');
    if(snap.metadata.fromCache && Date.now()-membershipLease(uid,scope)>AUTHORITY_LEASE_MS)throw new Error('OFFLINE_AUTH_EXPIRED');
    if(!snap.metadata.fromCache)membershipLease(uid,scope,Date.now());
  };
  await confirm();
  const page=await sdk.getDocs(sdk.query(sdk.collection(db,'teams',scope.teamId,'conversations',conversationId,'messages'),
    sdk.where('seq','<',beforeSeq),sdk.orderBy('seq','desc'),sdk.limit(50)));
  await confirm();
  return page.docs.map(doc=>firestoreMessage(doc.id,doc.data())).reverse();
}

function recentMessagesQuery(sdk: typeof import('firebase/firestore'), db: Firestore, scope: MessageScope, id: string) {
  return sdk.query(sdk.collection(db, 'teams', scope.teamId, 'conversations', id, 'messages'), sdk.orderBy('seq', 'desc'), sdk.limit(50));
}

/** Warm only the SDK cache. The selected conversation watcher still owns message and read snapshots. */
export function prefetchFirestoreMessages(sdk: typeof import('firebase/firestore'), db: Firestore, scope: MessageScope, id: string): () => void {
  // The short-lived listener also lets a click reuse the same live query while it is still subscribed.
  return sdk.onSnapshot(recentMessagesQuery(sdk, db, scope, id), () => {}, () => {});
}

/** SDK snapshots are the only ordinary-message read path, including offline reads. */
export function watchFirestoreMessages(sdk: typeof import('firebase/firestore'), db: Firestore, uid: string, scope: MessageScope, conversationId: string,
  next: (snapshot: MessageSnapshot) => void, isCurrent: () => boolean): () => void {
  const { collection, doc, onSnapshot, orderBy, query, where } = sdk;
  let closed = false;
  const stops: (() => void)[] = [];
  const personal = new Map<string,{unreadCount:number;manualUnreadRevision:number;pendingActionRevision?:number;failureRevision?:number}>();
  const readSeq = new Map<string, number>();
  let authorized = false;
  let offlineExpiry:ReturnType<typeof setTimeout>|undefined;
  let verifiedAt=membershipLease(uid,scope);
  function expireCachedAuthority(){
    authorized=false;stopMessages?.();stopMessages=undefined;
    emit({kind:'directory',conversations:[],fromCache:true});
    if(conversationId)emit({kind:'messages',conversationId,fromCache:true,replace:true,messages:[]});
    emit({kind:'error',fromCache:true,error:'OFFLINE_AUTH_EXPIRED'});
  }
  let directory: { conversations: TeamConversation[]; fromCache: boolean } | undefined;
  let stopMessages: (() => void) | undefined;
  let selectedAllowed = false;
  const root = ['teams', scope.teamId] as const;
  const selectedPath = [...root, 'conversations', conversationId] as const;
  const head = conversationId ? recentMessagesQuery(sdk, db, scope, conversationId) : undefined;
  // Start the SDK cache read while the membership and directory listeners confirm access.
  // Only watchSelected may publish this result, after both checks have passed.
  let initialCache = head ? sdk.getDocsFromCache(head).catch(() => undefined) : undefined;
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
    if (directory) emit({ kind: 'directory', ...directory, conversations: directory.conversations.map(value => ({ ...value, ...personal.get(value.id),lastReadSeq: readSeq.get(value.id) ?? 0 })) });
    watchSelected();
  }
  function watchSelected(): void {
    if (!authorized || closed || !selectedAllowed || stopMessages || !head) return;
    let stopped=false, stopChanges:(()=>void)|undefined, version=0;
    const path = selectedPath, recentQuery = head;
    const current=()=>!closed && !stopped && selectedAllowed && isCurrent();
    let watermark=directory?.conversations.find(row=>row.id===conversationId)?.lastChangeSeq ?? 0;
    const read=sdk.onSnapshot(doc(db,...path,'reads',uid),{includeMetadataChanges:true},mark=>{
      if(!current()) return;
      const value=mark.data(), lastReadSeq=value?.generation===scope.generation ? value.lastReadSeq : 0;
      readSeq.set(conversationId,lastReadSeq);
      emit({kind:'read',conversationId,lastReadSeq,fromCache:mark.metadata.fromCache,...personal.get(conversationId)});
    },fail);
    const own=sdk.onSnapshot(doc(db,'users',uid,'inbox','team:'+scope.teamId+':'+conversationId),mark=>{
      const row=mark.data();
      if(!current() || row?.ref?.generation!==scope.generation) return;
      const value={unreadCount:count(row.unreadCount)??0,manualUnreadRevision:count(row.manualUnreadRevision)??0,pendingActionRevision:count(row.pendingActionRevision),failureRevision:count(row.failureRevision)};
      personal.set(conversationId,value);
      emit({kind:'read',conversationId,lastReadSeq:count(row.readThrough)??0,fromCache:mark.metadata.fromCache,...value});
    },()=>{/* Existing installations may only expose the source read mark. */});
    stopMessages=()=>{stopped=true;version++;stopChanges?.();read();own();};
    const recent = async (rebase = false) => {
      const ticket=++version;
      const pageRequest = sdk.getDocs(recentQuery);
      let publishedPage = false;
      if (!rebase && initialCache) {
        const cachedPage = initialCache;
        initialCache = undefined;
        void cachedPage.then(cached => {
          if (!publishedPage && current() && version === ticket && cached?.size) {
            emit({kind:'messages',conversationId,fromCache:true,replace:true,messages:cached.docs.map(d=>firestoreMessage(d.id,d.data())).reverse()});
          }
        }).catch(error => { if (current() && version === ticket) fail(error); });
      }
      const page=await pageRequest;
      publishedPage = true;
      if(!current() || ticket!==version) return;
      emit({kind:'messages',conversationId,fromCache:page.metadata.fromCache,replace:true,rebase,messages:page.docs.map(d=>firestoreMessage(d.id,d.data())).reverse()});
    };
    let chain=Promise.resolve();
    async function consume(page:import('firebase/firestore').QuerySnapshot) {
      if(!current()) return;
      emit({kind:'messages',conversationId,fromCache:page.metadata.fromCache,replace:false,messages:[]});
      if(page.empty || page.metadata.fromCache) return;
      // Ignore query removals: a newest-changes window moves on every create and edit.
      const docs=[...page.docs].reverse(), latest=docs[docs.length-1]!.data().seq;
      if(latest<=watermark) return;
      const first=docs.find(d=>d.data().seq>watermark);
      const needsRebase=latest-watermark>1000 || (!!first && millis(first.data().at)>0 && Date.now()-millis(first.data().at)>30*86400000);
      if(needsRebase) {await recent(true);watermark=latest;return;}
      let at=watermark;
      while(at<latest && current()){
        const changes=docs[0]!.data().seq<=at+1 ? docs.filter(d=>d.data().seq>at) :
          (await sdk.getDocs(sdk.query(collection(db,...path,'changes'),where('seq','>',at),where('seq','<=',latest),orderBy('seq'),sdk.limit(50)))).docs;
        if(!current())return;
        if(!changes.length || changes.some((d,i)=>d.data().seq!==at+i+1 || (millis(d.data().at)>0 && Date.now()-millis(d.data().at)>30*86400000))) {
          await recent(true);watermark=latest;return;
        }
        const operations=new Map<string,string>();
        for(const change of changes) {
          const {messageId,operation}=change.data();
          if(typeof messageId!=='string') throw invalidData();
          if(operation==='delete' || operations.get(messageId)!=='create') operations.set(messageId,operation);
        }
        const messages:TeamMessage[]=[],removed:string[]=[],updatedIds:string[]=[];
        const ids=[...operations.keys()];
        for(let i=0;i<ids.length;i+=8) await Promise.all(ids.slice(i,i+8).map(async id=>{
          const op=operations.get(id);
          if(op==='delete'){removed.push(id);return;}
          const saved=await sdk.getDocFromServer(doc(db,...path,'messages',id));
          if(!saved.exists()) return;
          messages.push(firestoreMessage(id,saved.data()));if(op!=='create')updatedIds.push(id);
        }));
        if(!current())return;
        emit({kind:'messages',conversationId,fromCache:page.metadata.fromCache,replace:false,messages,removed,updatedIds});
        at=changes[changes.length-1]!.data().seq;watermark=at;
      }
    }
    void recent().then(()=>{
      if(!current()) return;
      stopChanges=onSnapshot(sdk.query(collection(db,...path,'changes'),orderBy('seq','desc'),sdk.limit(50)),{includeMetadataChanges:true},page=>{
        chain=chain.then(()=>consume(page)).catch(e=>{if(current())fail(e);});
      },fail);
    }).catch(error=>{if(current())fail(error);});
  }
  function close(): void {
    if(closed)return;closed = true; clearTimeout(offlineExpiry);stops.forEach(stop => stop()); stopMessages?.();stopExact?.();
  }
  function decode(id:string,value:DocumentData):TeamConversation {
          const lastMessage = firestoreLastMessage(value.lastMessage);
          if (!Array.isArray(value.participants) || !value.participants.every((principal: unknown) => typeof principal === 'string' && /^(user:[A-Za-z0-9_-]{1,128}|shared_bot:[A-Za-z0-9_-]{8,64})$/u.test(principal))) throw invalidData();
          if (value.kind === 'bot_dm' && value.participants.filter((principal: string) => principal.startsWith('shared_bot:')).length !== 1) throw invalidData();
          const readers = Array.isArray(value.memberUids) && value.memberUids.every((reader: unknown) => typeof reader === 'string') ? { readers: value.memberUids as string[] } : {};
          const auto = value.autoReply, autoReply = auto && Number.isSafeInteger(auto.revision) && Array.isArray(auto.targets) && auto.targets.every((target: unknown) => typeof target === 'string')
            ? { autoReply: { revision: auto.revision as number, targets: auto.targets as string[] } } : {};
          return { id: id, kind: value.kind, title: value.title, participants: value.participants, ...readers, ...autoReply, lastSeq: value.lastSeq, lastChangeSeq: value.lastChangeSeq, lastReadSeq: readSeq.get(id) ?? 0, ...(!readSeq.has(id)?{unreadCount:0}:{}), ...(lastMessage ? { lastMessage } : {}) };
  }
  let sourceWindow:TeamConversation[]=[];
  let exact:TeamConversation|undefined, exactReady=!conversationId, stopExact:(()=>void)|undefined, windowCache=true;
  function updateDirectory() {
    if(!exactReady)return;
    const rows=sourceWindow.filter(row=>row.id!==conversationId || !stopExact);
    if(exact && !rows.some(r=>r.id===exact!.id))rows.push(exact);
    directory={fromCache:windowCache,conversations:rows};
    selectedAllowed=!!conversationId && rows.some(row=>row.id===conversationId);
    if(!selectedAllowed){stopMessages?.();stopMessages=undefined;}
    publish();
  }
  stops.push(onSnapshot(doc(db, ...root, 'members', uid), { includeMetadataChanges: true }, snapshot => {
    if (closed) return;
    clearTimeout(offlineExpiry);
    if (snapshot.data()?.generation === scope.generation) {
      if(!snapshot.metadata.fromCache){verifiedAt=Date.now();membershipLease(uid,scope,verifiedAt);}
      if(snapshot.metadata.fromCache){
        const remaining=verifiedAt+AUTHORITY_LEASE_MS-Date.now();
        if(remaining<=0){expireCachedAuthority();return;}
        offlineExpiry=setTimeout(expireCachedAuthority,remaining);
      }
      authorized=true;publish();
    } else if(authorized || !snapshot.metadata.fromCache){
      membershipLease(uid,scope,0);emit({kind:'error',fromCache:snapshot.metadata.fromCache,error:'STALE_MEMBERSHIP'});close();
    }
  }, fail));
  stops.push(onSnapshot(query(collection(db, ...root, 'conversations'), where('memberUids', 'array-contains', uid), sdk.limit(50)),
    { includeMetadataChanges: true }, snapshot => {
      if (closed) return;
      let conversations: TeamConversation[];
      try {
        conversations = snapshot.docs.map(snapshot => {
          return decode(snapshot.id,snapshot.data());
        });
      } catch {
        // A conversation that breaks the stored contract is not shown half-parsed; the team's listeners stop with the error.
        emit({ kind: 'error', fromCache: snapshot.metadata.fromCache, error: 'INVALID_DATA' });
        close();
        return;
      }
      sourceWindow=conversations;
      windowCache=snapshot.metadata.fromCache;
      if(!conversationId || conversations.some(row=>row.id===conversationId)) {
        stopExact?.();stopExact=undefined;exact=undefined;exactReady=true;
      } else if(!stopExact){
        exactReady=false;
        // A conversation selected from a later inbox page can lie outside this bounded source directory.
        stopExact=onSnapshot(doc(db,...root,'conversations',conversationId),{includeMetadataChanges:true},snap=>{
          if(closed)return;
          if(!snap.exists() && snap.metadata.fromCache)return;
          try {
            const data=snap.data();
            exact=data?.memberUids?.includes(uid)?decode(snap.id,data):undefined;
            exactReady=true;updateDirectory();
          } catch {emit({kind:'error',fromCache:snap.metadata.fromCache,error:'INVALID_DATA'});close();}
        },fail);
      }
      updateDirectory();
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
