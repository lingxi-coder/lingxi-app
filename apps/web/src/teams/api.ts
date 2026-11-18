import { botMarketLink } from './market-link';
import * as firestore from 'firebase/firestore';
import { collection, onSnapshot, type Firestore } from 'firebase/firestore';
import { teamsTransport, type TeamsApi, type TeamCommand, type TeamDetail, type TeamResult, type TeamSummary } from '../../../../packages/teams';
import { InboxController } from '../../../../packages/inbox/controller';
import { createInboxRequest } from '../../../../packages/inbox/client';
import { readFirestoreMessagePage, watchFirestoreMessages } from '../../../../packages/teams/firestore.js';
import type { MessageSignal, MessageCommand } from '../../../../packages/teams/messages';
import { getAuth, type User } from 'firebase/auth';

export function webTeamsApi(user: User, db: Firestore, currentUid: () => string | undefined = () => getAuth(db.app).currentUser?.uid): TeamsApi & { close(): void } {
  let closed = false;
  const request = teamsTransport('', async () => { if (currentUid() !== user.uid) throw new Error('INVALID_SESSION'); const token = await user.getIdToken(); if (closed || currentUid() !== user.uid) throw new Error('INVALID_SESSION'); return token; });
  const inboxRequest=createInboxRequest('/v1/inbox',async()=>{
    if(closed || currentUid()!==user.uid) throw new Error('INVALID_SESSION');
    const token=await user.getIdToken();if(closed || currentUid()!==user.uid) throw new Error('INVALID_SESSION');
    return token;
  });
  const inbox = new InboxController({uid:user.uid,request:inboxRequest});
  const callbacks = new Set<(event: { invalidated?: boolean }) => void>();
  const messageCallbacks = new Set<(signal?: MessageSignal) => void>();
  const messageStops = new Map<string, () => void>();
  const stops = new Map<string, () => void>();
  const failed = new Map<string, number>();
  const emit = (invalidated = false) => { if (!closed) for (const callback of callbacks) callback({ invalidated }); };
  const watch = (key: string, path: string[], generation = 0) => {
    if (stops.has(key)) return;
    if (failed.get(key) === generation) return;
    stops.set(key, onSnapshot(collection(db, path[0]!, ...path.slice(1)), { includeMetadataChanges: true },
      snapshot => emit(snapshot.metadata.fromCache), () => { stops.get(key)?.(); stops.delete(key); failed.set(key, generation); emit(true); }));
  };
  const checked = async <T>(path = '', body?: unknown): Promise<T> => {
    if (closed || currentUid() !== user.uid) throw new Error('INVALID_SESSION');
    const result = await request<T>(path, body);
    if (closed || currentUid() !== user.uid) throw new Error('INVALID_SESSION');
    return result;
  };
  return {
    async openBotMarket() {
      if (closed || currentUid() !== user.uid) throw new Error('INVALID_SESSION');
      window.open(botMarketLink(), '_blank', 'noopener,noreferrer');
    },
    async list() { watch('index', ['users', user.uid, 'teams']); return checked<{ teams: TeamSummary[] }>(); },
    async detail(teamId: string) {
      if (!/^[A-Za-z0-9_-]{8,64}$/u.test(teamId)) throw new Error('INVALID_INPUT');
      const current = await checked<TeamDetail>(`/${teamId}`);
      watch(teamId, ['teams', teamId, 'members'], current.team.generation); return current;
    },
    execute: (command: TeamCommand) => checked<TeamResult>('', command),
    messages: <T>(command: MessageCommand) => checked<T>('/messages', command),
    async historyPage(scope,conversationId,beforeSeq) {
      if(closed || currentUid()!==user.uid) throw new Error('INVALID_SESSION');
      const rows=await readFirestoreMessagePage(firestore,db,scope,conversationId,beforeSeq,user.uid);
      if(closed || currentUid()!==user.uid) throw new Error('INVALID_SESSION');
      return rows;
    },
    peerReceipt:(scope,id)=>inboxRequest({action:'peerReceipt',key:'team:'+scope.teamId+':'+id,generation:scope.generation}),
    readVisible:(scope,id,through,manualUnreadRevision,observedPendingActionRevision,observedFailureRevision)=>inbox.mutate({action:'markRead',key:'team:'+scope.teamId+':'+id,generation:scope.generation,through,manualUnreadRevision,...(observedPendingActionRevision===undefined?{}:{observedPendingActionRevision}),...(observedFailureRevision===undefined?{}:{observedFailureRevision})}),
    watchMessages(scope, conversationId, next) {
      if (closed || currentUid() !== user.uid) throw new Error('INVALID_SESSION');
      const key = crypto.randomUUID();
      const stop = watchFirestoreMessages(firestore, db, user.uid, scope, conversationId, snapshot => {
        if (closed) return;
        next(snapshot);
        if (snapshot.kind === 'signal' || snapshot.kind === 'error') for (const callback of messageCallbacks) callback(snapshot.kind === 'signal' ? snapshot.signal : undefined);
      }, () => !closed && currentUid() === user.uid);
      messageStops.set(key, stop);
      return () => { stop(); messageStops.delete(key); };
    },
    onMessagesChanged(callback) { messageCallbacks.add(callback); return () => { messageCallbacks.delete(callback); if (!messageCallbacks.size) { for (const stop of messageStops.values()) stop(); messageStops.clear(); } }; },
    onChanged(callback) { callbacks.add(callback); return () => { callbacks.delete(callback); }; },
    close() { closed = true; inbox.close(); for (const stop of stops.values()) stop(); stops.clear(); callbacks.clear(); for (const stop of messageStops.values()) stop(); messageStops.clear(); messageCallbacks.clear(); },
  };
}
