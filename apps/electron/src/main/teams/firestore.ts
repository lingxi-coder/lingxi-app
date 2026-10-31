import { randomUUID } from 'node:crypto';
import { deleteApp, initializeApp, type FirebaseOptions } from 'firebase/app';
import { connectAuthEmulator, initializeAuth, inMemoryPersistence, signInWithCustomToken, signOut, type Auth } from 'firebase/auth';
import { collection, connectFirestoreEmulator, getFirestore, onSnapshot, orderBy, query, terminate, where,
  type DocumentData, type Firestore, type FirestoreError, type QuerySnapshot } from 'firebase/firestore';

export interface TeamsFirestoreOptions {
  config: FirebaseOptions;
  /** The UID from AccountService, checked against the exchanged credential. */
  uid: string;
  customToken: string;
  /** Local spike/test only. An emulator session must use a demo project. */
  emulator?: { host: '127.0.0.1' | 'localhost'; authPort: number; firestorePort: number };
}
export interface TeamsFirestoreSession {
  /** Main-process only; never expose the SDK, credentials or snapshots over generic IPC. */
  readonly db: Firestore;
  watchChanges(teamId: string, conversationId: string, after: number,
    next: (snapshot: QuerySnapshot<DocumentData>) => void, error: (error: FirestoreError) => void): () => void;
  close(): Promise<void>;
}

/** A separate in-memory SDK session; AccountService remains the credential owner. */
export async function openTeamsFirestoreSession(options: TeamsFirestoreOptions): Promise<TeamsFirestoreSession> {
  if (!options.uid || !options.customToken) throw new Error('Team authentication required');
  if (options.emulator && (!options.config.projectId?.startsWith('demo-')
    || !['127.0.0.1', 'localhost'].includes(options.emulator.host))) throw new Error('Team emulator must be local and use a demo project');
  const app = initializeApp(options.config, `lingxi-teams-main-${randomUUID()}`);
  let auth: Auth | undefined;
  let db: Firestore | undefined;
  const subscriptions = new Set<() => void>();
  let closing: Promise<void> | undefined;
  function close(): Promise<void> {
    return closing ??= (async () => {
      for (const stop of subscriptions) stop();
      subscriptions.clear();
      try { if (db) await terminate(db); }
      finally { try { if (auth) await signOut(auth); } finally { await deleteApp(app); } }
    })();
  }
  try {
    auth = initializeAuth(app, { persistence: inMemoryPersistence });
    db = getFirestore(app);
    const firestore = db;
    if (options.emulator) {
      connectAuthEmulator(auth, `http://${options.emulator.host}:${options.emulator.authPort}`, { disableWarnings: true });
      connectFirestoreEmulator(db, options.emulator.host, options.emulator.firestorePort);
    }
    const credential = await signInWithCustomToken(auth, options.customToken);
    if (credential.user.uid !== options.uid) throw new Error('Team credential belongs to another account');
    return {
      db: firestore,
      close,
      watchChanges(teamId, conversationId, after, next, error) {
        if (closing) throw new Error('Team session is closed');
        if ([teamId, conversationId].some(id => !id || id.length > 128 || /[/\u0000-\u001f\u007f]/u.test(id))
          || !Number.isSafeInteger(after) || after < 0) throw new Error('Invalid Team change cursor');
        const changes = query(collection(firestore, 'teams', teamId, 'conversations', conversationId, 'changes'), where('seq', '>', after), orderBy('seq'));
        const unsubscribe = onSnapshot(changes, snapshot => { if (!closing) next(snapshot); }, failure => { if (!closing) error(failure); });
        const stop = () => { unsubscribe(); subscriptions.delete(stop); };
        subscriptions.add(stop);
        return stop;
      },
    };
  } catch (error) {
    await close();
    throw error;
  }
}

export { collection, doc, onSnapshot, query, where } from 'firebase/firestore';
