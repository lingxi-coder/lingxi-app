import { createDesktopHistoryReader } from '../main/teams/historyPage.js';
import * as firestore from 'firebase/firestore';
import { deleteApp, initializeApp } from 'firebase/app';
import { connectAuthEmulator, initializeAuth, browserLocalPersistence, signInWithCustomToken, signOut } from 'firebase/auth';
import { connectFirestoreEmulator, initializeFirestore, persistentLocalCache, persistentSingleTabManager, terminate } from 'firebase/firestore';
import { prefetchFirestoreMessages, watchFirestoreMessages } from '../../../../packages/teams/firestore.js';
import type { TeamsFirestoreOptions } from '../main/teams/firestore.js';
import type { MessageScope } from '../../../../packages/teams/messages.js';
import { createChatSourceReader, type ChatReadRequest } from '../../../../packages/teams/chat-source.js';

type SessionOptions = Omit<TeamsFirestoreOptions, 'customToken'> & { customToken?: string };

window.addEventListener('message', function receive(event: MessageEvent): void {
  if (event.source !== window || event.data !== 'lingxi:teams:data-port' || event.ports.length !== 1) return;
  window.removeEventListener('message', receive);
  const port = event.ports[0]!;
  let session: Awaited<ReturnType<typeof open>> | undefined;
  let reader: ReturnType<typeof createChatSourceReader> | undefined;
  let history: ReturnType<typeof createDesktopHistoryReader> | undefined;
  const stops = new Map<string, () => void>();
  port.onmessage = event => { void handle(event.data).catch(() => port.postMessage({ type: 'error' })); };
  port.start();
  async function open(options: SessionOptions): Promise<{ app: ReturnType<typeof initializeApp>; db: ReturnType<typeof initializeFirestore>; auth: ReturnType<typeof initializeAuth>; uid: string }> {
    if (session || !options.uid) throw new Error('INVALID_SESSION');
    const app = initializeApp(options.config, 'lingxi-teams-data');
    const auth = initializeAuth(app, { persistence: browserLocalPersistence });
    const db = initializeFirestore(app, { localCache: persistentLocalCache({ cacheSizeBytes: 80 * 1024 * 1024, tabManager: persistentSingleTabManager({}) }) });
    if (options.emulator) {
      if (!options.config.projectId?.startsWith('demo-') || !['127.0.0.1', 'localhost'].includes(options.emulator.host)) throw new Error('INVALID_SESSION');
      connectAuthEmulator(auth, `http://${options.emulator.host}:${options.emulator.authPort}`, { disableWarnings: true });
      connectFirestoreEmulator(db, options.emulator.host, options.emulator.firestorePort);
    }
    try {
      await auth.authStateReady();
      if (options.customToken) await signInWithCustomToken(auth, options.customToken);
      if (auth.currentUser?.uid !== options.uid) throw new Error('INVALID_SESSION');
      return { app, db, auth, uid: options.uid };
    } catch (error) { await terminate(db); await deleteApp(app); throw error; }
  }
  async function handle(message: { type: string; id: string; options: SessionOptions; scope: MessageScope; conversationId: string; prefetch?: boolean; clearAuthentication?: boolean; beforeSeq?:number; request?: ChatReadRequest }): Promise<void> {
    if (message.type === 'open') {
      session = await open(message.options);
      const current = session;
      history = createDesktopHistoryReader(firestore,current.db,current.uid,()=>session===current && current.auth.currentUser?.uid===current.uid);
      reader = createChatSourceReader(firestore, current.db, current.uid, () => session === current && current.auth.currentUser?.uid === current.uid);
      port.postMessage({ type: 'ready' }); return;
    }
    if (!session) throw new Error('INVALID_SESSION');
    if (message.type === 'close') {
      reader?.close(); reader = undefined; history=undefined;
      for (const stop of stops.values()) stop(); stops.clear();
      const current=session;session=undefined;
      if (message.clearAuthentication) await signOut(current.auth);
      await terminate(current.db); await deleteApp(current.app);
      port.postMessage({ type: 'closed' }); return;
    }
    if(message.type==='history-page'){
      const current=session;
      try {
        if(!history || message.beforeSeq===undefined)throw new Error('INVALID_INPUT');
        const messages=await history.read(message.scope,message.conversationId,message.beforeSeq);
        if(session!==current)throw new Error('INVALID_SESSION');
        port.postMessage({type:'history-result',id:message.id,uid:current.uid,result:messages});
      }catch(error){
        const code=error instanceof Error && /^[A-Z_]{3,50}$/u.test(error.message)?error.message:'MESSAGE_READ_FAILED';
        port.postMessage({type:'history-result',id:message.id,uid:current.uid,error:code});
      }
      return;
    }
    if (message.type === 'read') {
      const current = session;
      try {
        if (!message.request || !reader) throw new Error('INVALID_INPUT');
        const result = await reader.read(message.request);
        if (session !== current) throw new Error('INVALID_SESSION');
        port.postMessage({ type: 'read-result', id: message.id, uid: current.uid, result });
      } catch (error) {
        const code = error instanceof Error && /^(INVALID_INPUT|INVALID_SESSION|STALE_MEMBERSHIP|CONVERSATION_REQUIRED|INVALID_DATA|INVALID_SOURCE_TIME|INVALID_READ_CURSOR|MESSAGE_GAP|CHANGE_GAP|CHAT_CHANGED)$/u.test(error.message) ? error.message : 'MESSAGE_READ_FAILED';
        port.postMessage({ type: 'read-result', id: message.id, uid: current.uid, error: code });
      }
      return;
    }
    stops.get(message.id)?.(); stops.delete(message.id);
    if (message.type === 'watch' && message.prefetch) {
      if (session.auth.currentUser?.uid === session.uid) {
        stops.set(message.id, prefetchFirestoreMessages(firestore, session.db, message.scope, message.conversationId));
      }
      return;
    }
    if (message.type === 'watch') stops.set(message.id, watchFirestoreMessages(firestore, session.db, session.uid, message.scope, message.conversationId,
      snapshot => {
        if(snapshot.kind==='error' || snapshot.kind==='directory' && !snapshot.conversations.some(row=>row.id===message.conversationId))
          history?.revoke(message.scope,message.conversationId);
        port.postMessage({type:'snapshot',id:message.id,snapshot});
      }, () => !!session && session.auth.currentUser?.uid === session.uid));
  }
});
