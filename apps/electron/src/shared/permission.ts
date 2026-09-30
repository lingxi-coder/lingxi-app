import type { PermissionRequest } from '@lingxi/bridge-client';

/** Electron-only broker ownership; SDK permission wire DTOs remain unchanged. */
export interface HostPermissionRequest extends PermissionRequest {
  backgroundOwned?: boolean;
}
