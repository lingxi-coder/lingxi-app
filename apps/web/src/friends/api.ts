import { collection, doc, getDoc, onSnapshot, type Firestore } from 'firebase/firestore';
import { getAuth, type User } from 'firebase/auth';
import { friendsTransport, type BlockEntry, type FriendCard, type FriendProfile, type FriendsApi, type FriendsCommand, type TeamOffer } from '../../../../packages/teams/friends';
import { firestoreBlock, firestoreFriendCard, firestoreFriendName, firestoreTeamOffer } from '../../../../packages/teams/firestore.js';

/**
 * The Friends API of a signed-in Web account: HTTP with its ID token, the three owner-only listeners of `users/{uid}/friends`,
 * `users/{uid}/blocks` and `users/{uid}/teamOffers` on the page's Firestore, and one-shot reads of a friend's profile.
 * Nothing runs for another account: a call or snapshot after the account changed or `close()` is dropped.
 */
export function webFriendsApi(user: User, db: Firestore, currentUid: () => string | undefined = () => getAuth(db.app).currentUser?.uid): FriendsApi & { close(): void } {
  let closed = false;
  const stops = new Set<() => void>();
  const live = () => !closed && currentUid() === user.uid;
  const request = friendsTransport('', async () => { if (!live()) throw new Error('INVALID_SESSION'); const token = await user.getIdToken(); if (!live()) throw new Error('INVALID_SESSION'); return token; });
  const checked = async <T>(path = '', body?: unknown): Promise<T> => {
    if (!live()) throw new Error('INVALID_SESSION');
    const result = await request<T>(path, body);
    if (!live()) throw new Error('INVALID_SESSION');
    return result;
  };
  const watch = <T>(path: string, map: (uid: string, data: Record<string, unknown>) => T | undefined, next: (rows: T[]) => void, failed: (code: string) => void) => {
    if (!live()) throw new Error('INVALID_SESSION');
    const stop = onSnapshot(collection(db, 'users', user.uid, path), snapshot => {
      if (live()) next(snapshot.docs.flatMap(row => map(row.id, row.data()) ?? []));
    }, () => { if (live()) failed('SERVICE_UNAVAILABLE'); });
    stops.add(stop);
    return () => { stop(); stops.delete(stop); };
  };
  return {
    profile: () => checked<FriendProfile>('/profile'),
    execute: <C extends FriendsCommand>(command: C) => checked('', command),
    watchCards: (next, failed) => watch<FriendCard>('friends', firestoreFriendCard, next, failed),
    watchBlocks: (next, failed) => watch<BlockEntry>('blocks', firestoreBlock, next, failed),
    watchOffers: (next, failed) => watch<TeamOffer>('teamOffers', firestoreTeamOffer, next, failed),
    async readProfile(uid) {
      if (!live() || !/^[A-Za-z0-9_-]{1,128}$/u.test(uid)) return null;
      return firestoreFriendName((await getDoc(doc(db, 'profiles', uid))).data());
    },
    close() { closed = true; for (const stop of stops) stop(); stops.clear(); },
  };
}
