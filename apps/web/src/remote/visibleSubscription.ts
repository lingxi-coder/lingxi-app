interface VisibilitySource {
  readonly visibilityState: string;
  addEventListener(type: 'visibilitychange', listener: () => void): void;
  removeEventListener(type: 'visibilitychange', listener: () => void): void;
}

/** Presence is only useful while visible. Hidden tabs must not reserve an SSE
 * connection and starve a foreground tab's signaling requests. */
export function subscribeWhileVisible(
  visibility: VisibilitySource,
  start: () => { close(): void },
): () => void {
  let subscription: { close(): void } | undefined;
  let disposed = false;
  const sync = (): void => {
    if (disposed) return;
    if (visibility.visibilityState === 'visible') {
      subscription ??= start();
    } else {
      subscription?.close();
      subscription = undefined;
    }
  };
  visibility.addEventListener('visibilitychange', sync);
  sync();
  return () => {
    disposed = true;
    visibility.removeEventListener('visibilitychange', sync);
    subscription?.close();
    subscription = undefined;
  };
}
