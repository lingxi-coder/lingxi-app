/**
 * Ambient typing for the `window.lingxi` surface (M10 A1 — C3).
 *
 * The preload (`src/preload/index.ts`) exposes this API via `contextBridge`,
 * but the renderer's tsconfig only includes `src/renderer/**`, so the renderer
 * cannot import the preload's `LingxiApi` type directly. This declaration
 * mirrors that surface 1:1 (verified against `src/preload/index.ts`) so the
 * renderer is fully typed without reaching across the project boundary.
 *
 * The wire DTOs (`ClientEvent`, `PermissionRequest`, `PermissionResponseDto`)
 * come from the shared SDK — the single source of truth for the contract.
 */

import type {
  ClientEvent,
  PermissionRequest,
  PermissionResponseDto,
} from '@lingxi/bridge-client';

/** Coarse lifecycle of the bridge connection (mirrors the main process). */
export type ConnectionState =
  | { status: 'idle' }
  | { status: 'spawning' }
  | { status: 'connecting' }
  | { status: 'connected' }
  | { status: 'disconnected'; reason?: string }
  | { status: 'error'; message: string };

/** An unsubscribe handle returned by the `on*` registrations. */
export type Unsubscribe = () => void;

/** The typed surface exposed to the renderer as `window.lingxi`. */
export interface LingxiApi {
  platform: NodeJS.Platform;
  isElectron: true;
  sendPrompt(text: string): Promise<void>;
  approve(requestId: number, response?: PermissionResponseDto): Promise<void>;
  deny(requestId: number): Promise<void>;
  cancel(turnId?: number): Promise<void>;
  connectionState(): Promise<ConnectionState>;
  onEvent(cb: (event: ClientEvent) => void): Unsubscribe;
  onPermission(cb: (request: PermissionRequest) => void): Unsubscribe;
  onConnectionStateChanged(cb: (state: ConnectionState) => void): Unsubscribe;
}

declare global {
  interface Window {
    /** Present only when hosted by the Electron preload; `undefined` in a browser. */
    lingxi?: LingxiApi;
  }
}
