import type { Firestore } from 'firebase/firestore';

/** Each account has sixteen dirty stamps and sixteen badge shards. Rows are served through the authorized list endpoint. */
export function watchInbox(
  sdk: typeof import('firebase/firestore'),
  db: Firestore,
  uid: string,
  dirty: () => void,
  badge: (count: number) => void,
  error: (e: Error) => void,
): () => void {
  const source = (name: string) => sdk.query(sdk.collection(db, 'users', uid, name), sdk.limit(16));
  const fail = (value: { code: string }) => error(new Error(value.code));
  const stopSignals = sdk.onSnapshot(
    source('inboxSignals'),
    (snap) => {
      if (!snap.metadata.fromCache) dirty();
    },
    fail,
  );
  const stopBadges = sdk.onSnapshot(
    source('inboxBadges'),
    (snap) => {
      badge(snap.docs.reduce((sum, doc) => sum + Math.max(0, Number(doc.data().count) || 0), 0));
    },
    fail,
  );
  return () => {
    stopSignals();
    stopBadges();
  };
}
