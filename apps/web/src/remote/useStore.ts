import { useSyncExternalStore } from 'react';

/**
 * The state of a store the pages read: the client's lists, one session, a Bot's
 * chats. Its own module, apart from `useRemote.ts`, so that a component that
 * only reads a store does not pull in Firebase (and a test can render it).
 */
export function useStore<T>(store: { subscribe(listener: () => void): () => void; getState(): T } | null): T | null {
  return useSyncExternalStore(
    (listener) => (store === null ? () => undefined : store.subscribe(listener)),
    () => (store === null ? null : store.getState()),
  );
}
