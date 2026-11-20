import { useSyncExternalStore } from 'react';
import { onAuthStateChanged, type User } from 'firebase/auth';
import { auth } from './firebase';

// One Firebase auth subscription for every workspace page, kept for the page's
// life: moving between workspace pages never drops back to "not yet known".
// `undefined` until Firebase has restored (or not) the saved session, then the
// user or `null`.
let current: User | null | undefined;
const listeners = new Set<() => void>();
let started = false;

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  if (!started) {
    started = true;
    onAuthStateChanged(auth, (user) => {
      current = user;
      for (const notify of [...listeners]) notify();
    });
  }
  return () => { listeners.delete(listener); };
}

export function useAuthUser(): User | null | undefined {
  return useSyncExternalStore(subscribe, () => current, () => undefined);
}
