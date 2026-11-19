import { useSyncExternalStore } from 'react';
import { onAuthStateChanged, type User } from 'firebase/auth';
import { auth } from './firebase';
import { setSessionHint } from './sessionHint';

// One Firebase auth subscription for every workspace page. `undefined` until
// Firebase has restored (or not) the saved session, then the user or `null`.
let current: User | null | undefined;
const listeners = new Set<() => void>();
let unsubscribe: (() => void) | undefined;

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  unsubscribe ??= onAuthStateChanged(auth, (user) => {
    current = user;
    setSessionHint(!!user);
    for (const notify of [...listeners]) notify();
  });
  return () => {
    listeners.delete(listener);
    if (!listeners.size && unsubscribe) { unsubscribe(); unsubscribe = undefined; current = undefined; }
  };
}

export function useAuthUser(): User | null | undefined {
  return useSyncExternalStore(subscribe, () => current, () => undefined);
}
