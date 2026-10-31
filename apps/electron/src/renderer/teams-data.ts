import * as firestore from 'firebase/firestore';
import { deleteApp, initializeApp } from 'firebase/app';
import { connectAuthEmulator, initializeAuth, browserLocalPersistence, signInWithCustomToken, signOut } from 'firebase/auth';
import { connectFirestoreEmulator, initializeFirestore, persistentLocalCache, persistentSingleTabManager, terminate } from 'firebase/firestore';
import { watchFirestoreMessages } from '../../../../packages/teams/firestore.js';
import type { TeamsFirestoreOptions } from '../main/teams/firestore.js';
import type { MessageScope } from '../../../../packages/teams/messages.js';

type SessionOptions = Omit<TeamsFirestoreOptions, 'customToken'> & { customToken?: string };

window.addEventListener('message', function receive(event: MessageEvent): void {
  if (event.source !== window || event.data !== 'lingxi:teams:data-port' || event.ports.length !== 1) return;
  window.removeEventListener('message', receive);
  const port = event.ports[0]!;
  let session: Awaited<ReturnType<typeof open>> | undefined;
  const stops = new Map<string, () => void>();
  port.onmessage = event => { void handle(event.data).catch(() => port.postMessage({ type: 'error' })); };
  port.start();
  async function open(options: SessionOptions): Promise<{ app: ReturnType<typeof initializeApp>; db: ReturnType<typeof initializeFirestore>; auth: ReturnType<typeof initializeAuth>; uid: string }> {
    if (session || !options.uid) throw new Error('INVALID_SESSION');
    const app = initializeApp(options.config, 'lingxi-teams-data');
    const auth = initializeAuth(app, { persistence: browserLocalPersistence });
    const db = initializeFirestore(app, { localCache: persistentLocalCache({ tabManager: persistentSingleTabManager({}) }) });
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
  async function handle(message: { type: string; id: string; options: SessionOptions; scope: MessageScope; conversationId: string; clearAuthentication?: boolean }): Promise<void> {
    if (message.type === 'open') { session = await open(message.options); port.postMessage({ type: 'ready' }); return; }
    if (!session) throw new Error('INVALID_SESSION');
    if (message.type === 'close') {
      for (const stop of stops.values()) stop(); stops.clear();
      if (message.clearAuthentication) await signOut(session.auth);
      await terminate(session.db); await deleteApp(session.app); session = undefined;
      port.postMessage({ type: 'closed' }); return;
    }
    stops.get(message.id)?.(); stops.delete(message.id);
    if (message.type === 'watch') stops.set(message.id, watchFirestoreMessages(firestore, session.db, session.uid, message.scope, message.conversationId,
      snapshot => port.postMessage({ type: 'snapshot', id: message.id, snapshot }), () => !!session && session.auth.currentUser?.uid === session.uid));
  }
});
