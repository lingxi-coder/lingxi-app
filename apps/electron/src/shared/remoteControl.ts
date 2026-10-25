/**
 * What the desktop window may know of and ask of Remote Control: its status,
 * the computers' own on/off switches, the approved devices, and which sessions
 * viewers may see. Plain data only; never an ID token, a key or a pairing
 * secret. The main process validates everything it is sent.
 */

import type { ImageRefDto } from '@lingxi/bridge-client';

/** Invoke → `RemoteControlUiStatus`. */
export const CH_REMOTE_STATUS_GET = 'lingxi:remote:status:get';
/** Main → renderer: the status changed. */
export const CH_REMOTE_STATUS_EVENT = 'lingxi:remote:status';
/** Invoke(`enabled: boolean`) → `RemoteControlUiStatus`. Turns Remote Control on or off for this computer. */
export const CH_REMOTE_ENABLED_SET = 'lingxi:remote:enabled:set';
/** Invoke → `boolean`: whether every new session starts with Remote Control on. */
export const CH_REMOTE_AT_STARTUP_GET = 'lingxi:remote:at-startup:get';
/** Invoke(`enabled: boolean`) → `boolean`. */
export const CH_REMOTE_AT_STARTUP_SET = 'lingxi:remote:at-startup:set';
/** Invoke → `RemoteDeviceUi[]`. */
export const CH_REMOTE_DEVICES_LIST = 'lingxi:remote:devices:list';
/** Invoke(`deviceId: string`) → `RemoteDeviceUi[]`. Forgets the device and ends its connections. */
export const CH_REMOTE_DEVICE_REVOKE = 'lingxi:remote:devices:revoke';
/** Invoke → `RemoteSessionsUi`. */
export const CH_REMOTE_SESSIONS_GET = 'lingxi:remote:sessions:get';
/**
 * Invoke(`sessionId: string, enabled: boolean`) → `RemoteSessionsUi`. Turning
 * a session on also turns Remote Control on for the computer, the first time.
 */
export const CH_REMOTE_SESSION_SET = 'lingxi:remote:sessions:set';
/** Main → renderer: a session was switched on or off, or viewers attached or left. */
export const CH_REMOTE_SESSIONS_EVENT = 'lingxi:remote:sessions';
/** Main → renderer: a viewer sent a prompt (`RemotePromptEchoUi`); the window shows it as a user row. */
export const CH_REMOTE_PROMPT_ECHO = 'lingxi:remote:prompt-echo';

export type RemoteControlUiState = 'off' | 'starting' | 'online' | 'error';

export interface RemoteConnectionUi {
  connId: string;
  deviceId: string;
  deviceName: string;
  platform: 'web' | 'android' | 'ios';
  transport: 'webrtc' | 'tunnel';
  /** False while the device waits for the user to allow it. */
  approved: boolean;
  /** For a direct connection: `host` and `srflx` are peer to peer, `relay` goes through a TURN server. */
  candidateType?: 'host' | 'srflx' | 'prflx' | 'relay';
}

export interface RemoteControlUiStatus {
  /** The computer's own switch (`remoteControlEnabled`). */
  enabled: boolean;
  /** Every new session starts with Remote Control on. */
  atStartup: boolean;
  /** The administrator forbids Remote Control; the switches do nothing. */
  managedDisabled: boolean;
  signedIn: boolean;
  state: RemoteControlUiState;
  /** What went wrong, when `state` is `error`. */
  error?: string;
  connections: RemoteConnectionUi[];
}

export interface RemoteDeviceUi {
  deviceId: string;
  name: string;
  platform: 'web' | 'android' | 'ios';
  /** Epoch milliseconds. */
  approvedAt: number;
  lastSeen: number;
}

export interface RemoteSessionUi {
  sessionId: string;
  /** Viewers attached to the session right now. */
  viewers: number;
}

/** The sessions that have Remote Control on. Any other session has it off. */
export interface RemoteSessionsUi {
  sessions: RemoteSessionUi[];
}

export interface RemotePromptEchoUi {
  sessionId: string;
  turnId: number;
  text: string;
  images: ImageRefDto[];
  origin: 'device';
  deviceName: string;
}

export interface RemoteControlApi {
  status(): Promise<RemoteControlUiStatus>;
  onStatus(callback: (status: RemoteControlUiStatus) => void): () => void;
  setEnabled(enabled: boolean): Promise<RemoteControlUiStatus>;
  atStartup(): Promise<boolean>;
  setAtStartup(enabled: boolean): Promise<boolean>;
  devices(): Promise<RemoteDeviceUi[]>;
  revokeDevice(deviceId: string): Promise<RemoteDeviceUi[]>;
  sessions(): Promise<RemoteSessionsUi>;
  setSession(sessionId: string, enabled: boolean): Promise<RemoteSessionsUi>;
  onSessions(callback: (sessions: RemoteSessionsUi) => void): () => void;
  onPromptEcho(callback: (echo: RemotePromptEchoUi) => void): () => void;
}
