import { DEFAULT_RTDB_URL, RTDB_EMULATOR_NAMESPACE } from '@lingxi/bridge-client/remote';

/** Where the viewer reads and writes: the project's RTDB instance, or an emulator while developing. */
export interface RemoteRtdbConfig {
  databaseUrl: string;
  /** The emulator's `ns` query parameter; absent for the real instance. */
  namespace?: string;
}

/** The slice of `import.meta.env` this module reads, so tests need no Vite. */
export interface RemoteEnv {
  DEV?: boolean;
  VITE_REMOTE_RTDB_URL?: string;
  VITE_REMOTE_RTDB_NAMESPACE?: string;
}

/**
 * `VITE_REMOTE_RTDB_URL` (and `VITE_REMOTE_RTDB_NAMESPACE`, which defaults to
 * the emulator's name for the project) point the dev server at the RTDB
 * emulator. They are ignored in production builds, so a stray `.env` cannot
 * redirect a deployed page.
 */
export function remoteRtdbConfig(env: RemoteEnv | undefined = import.meta.env): RemoteRtdbConfig {
  const url = env?.DEV ? env.VITE_REMOTE_RTDB_URL?.trim() : undefined;
  if (!url) return { databaseUrl: DEFAULT_RTDB_URL };
  return { databaseUrl: url, namespace: env?.VITE_REMOTE_RTDB_NAMESPACE?.trim() || RTDB_EMULATOR_NAMESPACE };
}

declare const __LINGXI_WEB_VERSION__: string | undefined;

/** Build version sent in the offer's `device.app_version`; `dev` outside a Vite build. */
export function webAppVersion(): string {
  return typeof __LINGXI_WEB_VERSION__ === 'string' && __LINGXI_WEB_VERSION__ ? __LINGXI_WEB_VERSION__ : 'dev';
}

/** The slice of `Storage` the viewer uses; both `localStorage` and `sessionStorage` fit. */
export interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

/** A browser storage area, or `undefined` when the browser refuses access (private mode, blocked site data). */
export function browserStorage(kind: 'localStorage' | 'sessionStorage'): StorageLike | undefined {
  try {
    return typeof window === 'undefined' ? undefined : window[kind];
  } catch {
    return undefined;
  }
}

export const FORCE_TUNNEL_STORAGE_KEY = 'lingxi.remote.forceTunnel';

/**
 * The debug switch that makes every connection use the RTDB tunnel: `?tunnel=1`
 * in the address, or `localStorage['lingxi.remote.forceTunnel'] === '1'`.
 */
export function forceTunnelRequested(search: string, storage?: StorageLike): boolean {
  if (new URLSearchParams(search).get('tunnel') === '1') return true;
  try {
    return storage?.getItem(FORCE_TUNNEL_STORAGE_KEY) === '1';
  } catch {
    return false;
  }
}
