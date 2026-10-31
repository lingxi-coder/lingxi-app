/**
 * The owner's last permission mode and Fast mode, as the hooks of the runtime
 * manager that read and write them. They are the owner's choices for the
 * owner's sessions. A Bot's sessions neither start from them nor change them:
 * a Bot starts in its own default mode (its settings say which), and a change
 * made inside one of its sessions lasts for that session only.
 */

import type { PermissionModeId } from '@lingxi/bridge-client';
import type { SessionRef } from '../bridgeTypes.js';

/** What of the settings store these hooks use. */
export interface OwnerPreferences {
  getLastPermissionMode(): PermissionModeId | undefined;
  setLastPermissionMode(mode: PermissionModeId): void;
  getLastFastMode(): boolean | undefined;
  setLastFastMode(enabled: boolean): void;
}

export function ownerPreferenceHooks(preferences: OwnerPreferences, isBotSession: (ref: SessionRef) => boolean) {
  return {
    getSavedPermissionMode: (ref: SessionRef): PermissionModeId | undefined => isBotSession(ref) ? undefined : preferences.getLastPermissionMode(),
    onPermissionModeSelected: (ref: SessionRef, mode: PermissionModeId): void => { if (!isBotSession(ref)) preferences.setLastPermissionMode(mode); },
    getSavedFastMode: (ref: SessionRef): boolean | undefined => isBotSession(ref) ? undefined : preferences.getLastFastMode(),
    onFastModeSelected: (ref: SessionRef, enabled: boolean): void => { if (!isBotSession(ref)) preferences.setLastFastMode(enabled); },
  };
}
