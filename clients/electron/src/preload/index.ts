import { contextBridge, ipcRenderer, type IpcRendererEvent } from 'electron';
import type {
  ClientEvent,
  PermissionRequest,
  PermissionResponseDto,
} from '@lingxi/bridge-client';

// ── IPC channel names (mirror `src/main/bridge.ts`) ──────────────────────────

const CH_SEND_PROMPT = 'lingxi:sendPrompt';
const CH_APPROVE = 'lingxi:approve';
const CH_DENY = 'lingxi:deny';
const CH_CANCEL = 'lingxi:cancel';
const CH_CONNECTION_STATE = 'lingxi:connectionState';
const CH_EVENT = 'lingxi:event';
const CH_PERMISSION = 'lingxi:permission';
const CH_STATE_CHANGED = 'lingxi:connectionStateChanged';

/** Coarse lifecycle of the bridge connection (mirrors `ConnectionState` in the main process). */
export type ConnectionState =
  | { status: 'idle' }
  | { status: 'spawning' }
  | { status: 'connecting' }
  | { status: 'connected' }
  | { status: 'disconnected'; reason?: string }
  | { status: 'error'; message: string };

/** An unsubscribe handle returned by the `on*` registrations. */
export type Unsubscribe = () => void;

/**
 * The typed surface exposed to the renderer as `window.lingxi`. The renderer
 * drives turns / permissions through these and subscribes to the live engine
 * feed via {@link LingxiApi.onEvent} / {@link LingxiApi.onPermission}.
 */
export interface LingxiApi {
  /** Static: which platform the main process runs on. */
  platform: NodeJS.Platform;
  /** Static: a renderer can detect it is hosted by Electron. */
  isElectron: true;

  /** Submit a user prompt to drive a turn. */
  sendPrompt(text: string): Promise<void>;
  /** Approve a parked permission request (defaults to allow-once). */
  approve(requestId: number, response?: PermissionResponseDto): Promise<void>;
  /** Deny a parked permission request. */
  deny(requestId: number): Promise<void>;
  /** Cancel the in-flight turn (optionally a specific `turnId`). */
  cancel(turnId?: number): Promise<void>;
  /** Read the current connection state. */
  connectionState(): Promise<ConnectionState>;

  /** Subscribe to inbound engine {@link ClientEvent}s. Returns an unsubscribe. */
  onEvent(cb: (event: ClientEvent) => void): Unsubscribe;
  /** Subscribe to inbound {@link PermissionRequest}s. Returns an unsubscribe. */
  onPermission(cb: (request: PermissionRequest) => void): Unsubscribe;
  /** Subscribe to {@link ConnectionState} transitions. Returns an unsubscribe. */
  onConnectionStateChanged(cb: (state: ConnectionState) => void): Unsubscribe;
}

/** Wrap an `ipcRenderer.on` subscription so the renderer never touches the raw event. */
function subscribe<T>(channel: string, cb: (payload: T) => void): Unsubscribe {
  const listener = (_e: IpcRendererEvent, payload: T): void => cb(payload);
  ipcRenderer.on(channel, listener);
  return () => ipcRenderer.removeListener(channel, listener);
}

const api: LingxiApi = {
  platform: process.platform,
  isElectron: true,

  sendPrompt: (text) => ipcRenderer.invoke(CH_SEND_PROMPT, text),
  approve: (requestId, response) => ipcRenderer.invoke(CH_APPROVE, requestId, response),
  deny: (requestId) => ipcRenderer.invoke(CH_DENY, requestId),
  cancel: (turnId) => ipcRenderer.invoke(CH_CANCEL, turnId),
  connectionState: () => ipcRenderer.invoke(CH_CONNECTION_STATE) as Promise<ConnectionState>,

  onEvent: (cb) => subscribe<ClientEvent>(CH_EVENT, cb),
  onPermission: (cb) => subscribe<PermissionRequest>(CH_PERMISSION, cb),
  onConnectionStateChanged: (cb) => subscribe<ConnectionState>(CH_STATE_CHANGED, cb),
};

if (process.contextIsolated) {
  try {
    contextBridge.exposeInMainWorld('lingxi', api);
  } catch (error) {
    console.error(error);
  }
} else {
  // @ts-expect-error fallback when context isolation is disabled
  window.lingxi = api;
}
