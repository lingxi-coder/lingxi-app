/**
 * The one vocabulary both processes use for "may this app record audio".
 *
 * It lives in `shared/` because the fact crosses the process boundary: only
 * the MAIN process can read it (`systemPreferences.getMediaAccessStatus`,
 * `main/microphoneAccess.ts`), and only the RENDERER displays it (the voice
 * settings page's 麦克风权限 row, via `renderer/audio/capabilities.ts`'s
 * `VoicePermissionStatus`, which is this type). A second declaration on each
 * side would drift exactly the way `bypassPermissionsModeAccepted` once did.
 *
 * Why this is not `navigator.permissions.query({name:'microphone'})`: that
 * API reports the PAGE permission, which in this app is decided by
 * `main/index.ts`'s `session.setPermissionCheckHandler` —
 * `permission === 'media' && isLocalRendererUrl(origin)`, i.e. always `true`
 * for the app's own renderer, computed with no reference to the OS grant.
 * Measured on real Electron 43 with this app's exact handlers, with macOS
 * microphone access never granted: the Permissions API said `granted` while
 * the OS said `not-determined`. Reporting the page permission as the
 * microphone permission is how the row came to say 已授权 while every
 * recording failed with `NotAllowedError`.
 */
export type MicrophonePermissionStatus = 'granted' | 'denied' | 'prompt' | 'unavailable';

const MICROPHONE_PERMISSION_STATUSES: readonly string[] = ['granted', 'denied', 'prompt', 'unavailable'];

/** Narrows an IPC-supplied value (i.e. one this process did not compute) to a real status. */
export function isMicrophonePermissionStatus(value: unknown): value is MicrophonePermissionStatus {
  return typeof value === 'string' && MICROPHONE_PERMISSION_STATUSES.includes(value);
}

/**
 * Maps Electron's `systemPreferences.getMediaAccessStatus('microphone')` to
 * the status the UI can speak about.
 *
 * `'not-determined'` is `'prompt'`, not `'denied'`: the OS has not asked yet,
 * so there is nothing for the 「打开系统设置」 button to fix — the next
 * `getUserMedia` raises the real prompt.
 *
 * `'restricted'` (MDM / parental controls) is `'denied'`: recording fails
 * exactly as it does for a refused grant, and the row has no honest sentence
 * for a fifth state. It is deliberately NOT `'unavailable'`, which claims
 * "cannot determine" about a state we determined perfectly well.
 *
 * Anything else — Electron's own `'unknown'`, a platform with no such API, a
 * value a future Electron adds — is `'unavailable'`. Never `'granted'`: an
 * unrecognised answer is not evidence of a grant, and treating it as one is
 * the whole defect.
 */
export function microphonePermissionFromMediaAccessStatus(raw: unknown): MicrophonePermissionStatus {
  switch (raw) {
    case 'granted': return 'granted';
    case 'denied': return 'denied';
    case 'restricted': return 'denied';
    case 'not-determined': return 'prompt';
    default: return 'unavailable';
  }
}
