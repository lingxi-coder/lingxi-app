import type { PermissionModeId, PermissionRequest } from '@lingxi/bridge-client';

/** Narrow the engine's wire string to a mode this desktop understands. */
export function isPermissionModeId(value: unknown): value is PermissionModeId {
  return value === 'default' || value === 'acceptEdits' || value === 'plan'
    || value === 'auto' || value === 'dontAsk' || value === 'bypassPermissions';
}

/** Electron-only broker ownership; SDK permission wire DTOs remain unchanged. */
export interface HostPermissionRequest extends PermissionRequest {
  backgroundOwned?: boolean;
}
