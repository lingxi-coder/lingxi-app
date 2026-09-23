/** Structural subset of the window/document event targets used here. */
export interface GrantChangeTarget {
  addEventListener(type: string, listener: () => void): void;
  removeEventListener(type: string, listener: () => void): void;
}

/** Re-read the OS-backed microphone permission after the app returns to foreground. */
export function subscribeMicrophoneGrantChanges(
  onChange: () => void,
  targets: { window: GrantChangeTarget; document: GrantChangeTarget & { visibilityState?: string } },
): () => void {
  const onFocus = () => onChange();
  const onVisibilityChange = () => { if (targets.document.visibilityState !== 'hidden') onChange(); };
  targets.window.addEventListener('focus', onFocus);
  targets.document.addEventListener('visibilitychange', onVisibilityChange);
  return () => {
    targets.window.removeEventListener('focus', onFocus);
    targets.document.removeEventListener('visibilitychange', onVisibilityChange);
  };
}
