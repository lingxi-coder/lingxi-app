import { collection, onSnapshot, orderBy, query, where,
  type DocumentData, type Firestore, type FirestoreError, type QuerySnapshot } from 'firebase/firestore';

/** Use the signed-in Web app's Firestore instance. Edits/recalls share this cursor. */
export function watchTeamChanges(db: Firestore, teamId: string, conversationId: string, after: number,
  next: (snapshot: QuerySnapshot<DocumentData>) => void, error: (error: FirestoreError) => void): () => void {
  if ([teamId, conversationId].some(id => !id || id.length > 128 || /[/\u0000-\u001f\u007f]/u.test(id))
    || !Number.isSafeInteger(after) || after < 0) throw new Error('Invalid Team change cursor');
  return onSnapshot(query(collection(db, 'teams', teamId, 'conversations', conversationId, 'changes'),
    where('seq', '>', after), orderBy('seq')), next, error);
}
