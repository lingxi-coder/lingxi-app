import type { Firestore } from 'firebase/firestore';
import { readFirestoreMessagePage } from '../../../../../packages/teams/firestore.js';
import type { MessageScope, TeamMessage } from '../../../../../packages/teams/messages.js';

export function validateHistoryPage(scope: MessageScope, conversationId: string, beforeSeq: number) {
  if (
    !scope ||
    ![scope.teamId, conversationId].every((id) => typeof id === 'string' && /^[A-Za-z0-9_-]{1,128}$/u.test(id)) ||
    !Number.isSafeInteger(scope.generation) ||
    scope.generation < 1 ||
    !Number.isSafeInteger(beforeSeq) ||
    beforeSeq < 1
  ) {
    throw new Error('INVALID_INPUT');
  }
}

/** Account-private data window. An offline SDK cache may only serve a recently verified membership. */
export function createDesktopHistoryReader(
  sdk: typeof import('firebase/firestore'),
  db: Firestore,
  uid: string,
  current: () => boolean,
  now = Date.now,
) {
  const leases = new Map<string, number>();
  const scopeKey = (scope: MessageScope, conversationId: string) =>
    JSON.stringify([scope.teamId, scope.generation, conversationId]);
  const check = () => {
    if (!current()) throw new Error('INVALID_SESSION');
  };
  function revoke(scope: MessageScope, conversationId: string) {
    leases.delete(scopeKey(scope, conversationId));
  }
  async function authorize(scope: MessageScope, conversationId: string) {
    check();
    const key = scopeKey(scope, conversationId);
    const refs = [
      sdk.doc(db, 'teams', scope.teamId),
      sdk.doc(db, 'teams', scope.teamId, 'members', uid),
      sdk.doc(db, 'teams', scope.teamId, 'conversations', conversationId),
    ];
    let server = true;
    let docs;
    try {
      docs = await Promise.all(refs.map((ref) => sdk.getDocFromServer(ref)));
    } catch (error) {
      check();
      const code = (error as { code?: string }).code;
      if (code === 'permission-denied' || code === 'unauthenticated') {
        leases.delete(key);
        throw new Error('SOURCE_ACCESS_REQUIRED');
      }
      if (code !== 'unavailable' || (leases.get(key) ?? 0) <= now()) throw error;
      server = false;
      docs = await Promise.all(refs.map((ref) => sdk.getDocFromCache(ref)));
    }
    check();
    if (docs[0]?.data()?.state !== 'active' || !docs[2]?.data()?.memberUids?.includes(uid)) {
      leases.delete(key);
      throw new Error('SOURCE_ACCESS_REQUIRED');
    }
    if (docs[1]?.data()?.generation !== scope.generation) {
      leases.delete(key);
      throw new Error('STALE_MEMBERSHIP');
    }
    if (server) {
      if (leases.size >= 32) leases.delete(leases.keys().next().value!);
      leases.set(key, now() + 300000);
    }
  }
  return {
    revoke,
    async read(scope: MessageScope, conversationId: string, beforeSeq: number): Promise<TeamMessage[]> {
      validateHistoryPage(scope, conversationId, beforeSeq);
      await authorize(scope, conversationId);
      const messages = await readFirestoreMessagePage(sdk, db, scope, conversationId, beforeSeq);
      // The query may have spent its entire lifetime offline, or the membership may have changed while it ran.
      await authorize(scope, conversationId);
      check();
      return messages;
    },
  };
}
