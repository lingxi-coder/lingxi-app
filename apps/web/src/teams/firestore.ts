import { collection, onSnapshot, orderBy, query, where,
  type DocumentData, type Firestore, type FirestoreError, type QuerySnapshot } from 'firebase/firestore';
import { connectFirestoreEmulator, getFirestore } from 'firebase/firestore';
import type { FirebaseApp } from 'firebase/app';

const connected = new WeakSet<Firestore>();
export function teamsDatabase(app: FirebaseApp): Firestore {
  const db = getFirestore(app);
  const emulator = import.meta.env.DEV ? import.meta.env.VITE_FIRESTORE_EMULATOR_HOST : undefined;
  if (emulator && !connected.has(db)) {
    const match = /^(127\.0\.0\.1|localhost):(\d{4,5})$/u.exec(emulator);
    if (!app.options.projectId?.startsWith('demo-') || !match) throw new Error('Teams emulator must be local and use a demo project');
    connectFirestoreEmulator(db, match[1]!, Number(match[2])); connected.add(db);
  }
  return db;
}

/** Use the signed-in Web app's Firestore instance. Edits/recalls share this cursor. */
export function watchTeamChanges(db: Firestore, teamId: string, conversationId: string, after: number,
  next: (snapshot: QuerySnapshot<DocumentData>) => void, error: (error: FirestoreError) => void): () => void {
  if ([teamId, conversationId].some(id => !id || id.length > 128 || /[/\u0000-\u001f\u007f]/u.test(id))
    || !Number.isSafeInteger(after) || after < 0) throw new Error('Invalid Team change cursor');
  return onSnapshot(query(collection(db, 'teams', teamId, 'conversations', conversationId, 'changes'),
    where('seq', '>', after), orderBy('seq')), next, error);
}
