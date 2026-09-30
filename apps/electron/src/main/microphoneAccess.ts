/**
 * The main process's read of the OS microphone grant.
 *
 * This is the ONLY authoritative source in the app: on macOS
 * `systemPreferences.getMediaAccessStatus('microphone')` returns the real TCC
 * grant, the same fact that decides whether the renderer's next
 * `getUserMedia` succeeds or rejects with `NotAllowedError`. The renderer
 * cannot read it — `navigator.permissions.query({name:'microphone'})` reports
 * the page permission this app grants itself (see
 * `shared/microphoneAccess.ts`'s header for the measurement) — so the value
 * is read here and handed over IPC.
 *
 * Kept in its own module, importing nothing but `shared/microphoneAccess.ts`
 * and `electron`, so the real-Electron test
 * (`test/microphone-permission-electron.test.mjs`) can load this exact
 * production code inside a real Electron main process and compare its answer
 * against `systemPreferences` directly.
 */
import { createRequire } from 'node:module';

import {
  microphonePermissionFromMediaAccessStatus,
  type MicrophonePermissionStatus,
} from '../shared/microphoneAccess.js';

const require = createRequire(import.meta.url);
const electronModule = require('electron');
/**
 * Absent outside Electron (where `require('electron')` is the binary PATH
 * string) and on Linux, where Electron exposes no `getMediaAccessStatus` at
 * all — both answered as `'unavailable'` below rather than as a fabricated
 * grant.
 */
const systemPreferences = typeof electronModule === 'string' ? undefined : electronModule.systemPreferences;

/** The one method of Electron's `systemPreferences` this app uses; injectable so a test can drive every OS answer. */
export interface MediaAccessReader {
  getMediaAccessStatus(mediaType: 'microphone'): string;
}

export interface MediaAccessRequester extends MediaAccessReader {
  askForMediaAccess(mediaType: 'microphone'): Promise<boolean>;
}

/**
 * Reads the OS microphone grant. Never throws: a platform without the API, or
 * an Electron that raises for an unsupported media type, is reported as
 * `'unavailable'` ("cannot determine"), which the 麦克风权限 row renders as
 * 无法确定 — an honest non-answer, never 已授权.
 */
export function readMicrophoneAccess(
  reader: MediaAccessReader | undefined = systemPreferences,
): MicrophonePermissionStatus {
  if (!reader || typeof reader.getMediaAccessStatus !== 'function') return 'unavailable';
  try {
    return microphonePermissionFromMediaAccessStatus(reader.getMediaAccessStatus('microphone'));
  } catch {
    return 'unavailable';
  }
}

/** Requests microphone access from the signed outer Electron application. */
export async function requestMicrophoneAccess(
  requester: MediaAccessRequester | undefined = systemPreferences,
): Promise<MicrophonePermissionStatus> {
  const current = readMicrophoneAccess(requester);
  if (current !== 'prompt') return current;
  if (!requester || typeof requester.askForMediaAccess !== 'function') return 'unavailable';
  try {
    await requester.askForMediaAccess('microphone');
    return readMicrophoneAccess(requester);
  } catch {
    return readMicrophoneAccess(requester);
  }
}
