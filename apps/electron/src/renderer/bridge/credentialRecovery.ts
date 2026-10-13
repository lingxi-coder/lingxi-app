// Installed only while bootstrap reports a secure-storage error. A locked
// macOS data-protection keychain can become available without a bridge event.
export function watchCredentialRecovery(options: {
  windowTarget: EventTarget;
  documentTarget: EventTarget;
  isVisible: () => boolean;
  refresh: () => Promise<void>;
}): () => void {
  let disposed = false;
  let pending = false;
  const refresh = () => {
    if (disposed || pending || !options.isVisible()) return;
    pending = true;
    void options.refresh().catch(() => {
      // Keep the original storage error visible until a later retry succeeds.
    }).finally(() => { pending = false; });
  };
  options.windowTarget.addEventListener('focus', refresh);
  options.documentTarget.addEventListener('visibilitychange', refresh);
  const timer = setInterval(refresh, 5_000);
  return () => {
    disposed = true;
    clearInterval(timer);
    options.windowTarget.removeEventListener('focus', refresh);
    options.documentTarget.removeEventListener('visibilitychange', refresh);
  };
}
