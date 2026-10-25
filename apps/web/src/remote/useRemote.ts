import { useEffect, useMemo, useState, useSyncExternalStore } from 'react';
import { onAuthStateChanged, signOut } from 'firebase/auth';
import {
  HostPresence,
  RtdbClient,
  RtdbMirror,
  hostsPath,
  parseHostRecord,
  systemClock,
  type HostRecord,
  type Rtdb,
} from '@lingxi/bridge-client/remote';
import { auth } from '../app/firebase';
import { browserDeviceStore, DeviceKeys, type PinnedHost } from './deviceKey';
import { browserStorage, forceTunnelRequested, remoteRtdbConfig } from './env';
import { browserPeerConnectionFactory } from './peerLink';
import { RemoteClient } from './remoteClient';
import { displayText } from './text';
import { TunnelMemory } from './tunnelMemory';
import { ViewerConnection, type ConnectionState } from './viewerConnection';
import { subscribeWhileVisible } from './visibleSubscription';

// ─────────────────────────────────────────────────────────────────────────────
// Process-wide pieces: one identity store and one tunnel memory per page load
// ─────────────────────────────────────────────────────────────────────────────

let deviceKeys: DeviceKeys | undefined;
let tunnelMemory: TunnelMemory | undefined;

export function getDeviceKeys(): DeviceKeys {
  deviceKeys ??= new DeviceKeys({ store: browserDeviceStore() });
  return deviceKeys;
}

function getTunnelMemory(): TunnelMemory {
  tunnelMemory ??= new TunnelMemory(systemClock, browserStorage('sessionStorage'));
  return tunnelMemory;
}

/** Whether this browser can do the cryptography the protocol is built on. */
export async function supportsRemoteCrypto(): Promise<boolean> {
  try {
    if (typeof crypto === 'undefined' || crypto.subtle === undefined) return false;
    await crypto.subtle.generateKey({ name: 'Ed25519' }, false, ['sign', 'verify']);
    await crypto.subtle.generateKey({ name: 'X25519' }, false, ['deriveBits']);
    return true;
  } catch {
    return false;
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// Account
// ─────────────────────────────────────────────────────────────────────────────

export type AuthState = { status: 'loading' } | { status: 'signedOut' } | { status: 'signedIn'; uid: string; email: string | null };

export function useAuthState(): AuthState {
  const [state, setState] = useState<AuthState>({ status: 'loading' });
  useEffect(
    () =>
      onAuthStateChanged(auth, (user) => setState(user === null ? { status: 'signedOut' } : { status: 'signedIn', uid: user.uid, email: user.email })),
    [],
  );
  return state;
}

export function signOutUser(): Promise<void> {
  return signOut(auth);
}

/** The database client for one account; its token comes from the signed-in Firebase user. */
export function useRtdb(uid: string): Rtdb {
  return useMemo(() => {
    const { databaseUrl, namespace } = remoteRtdbConfig();
    return new RtdbClient({
      databaseUrl,
      ...(namespace === undefined ? {} : { namespace }),
      getIdToken: async (forceRefresh) => {
        const user = auth.currentUser;
        if (user === null) throw new Error('signed out');
        return user.getIdToken(forceRefresh);
      },
    });
  }, [uid]);
}

// ─────────────────────────────────────────────────────────────────────────────
// Computers
// ─────────────────────────────────────────────────────────────────────────────

export interface HostEntry {
  hostId: string;
  name: string;
  platform: string;
  appVersion: string;
  online: boolean;
}

export interface HostsState {
  status: 'loading' | 'ready' | 'error';
  hosts: HostEntry[];
  pins: PinnedHost[];
}

const PRESENCE_TICK_MS = 10_000;

/** The user's computers, live, with an online flag judged by when the heartbeat last changed on this machine. */
export function useHosts(uid: string, rtdb: Rtdb): HostsState {
  const [state, setState] = useState<HostsState>({ status: 'loading', hosts: [], pins: [] });
  useEffect(() => {
    const mirror = new RtdbMirror();
    const presence = new HostPresence(systemClock);
    const records = new Map<string, HostRecord>();
    let failed = false;
    const publish = (): void => {
      const hosts = [...records].map(([hostId, record]): HostEntry => ({
        hostId,
        name: displayText(record.name, 64) || hostId,
        platform: displayText(record.platform, 32),
        appVersion: displayText(record.app_version, 32),
        online: presence.isOnline(hostId),
      }));
      hosts.sort((a, b) => Number(b.online) - Number(a.online) || a.name.localeCompare(b.name));
      setState((previous) => ({ ...previous, status: failed ? 'error' : 'ready', hosts }));
    };
    const unsubscribe = subscribeWhileVisible(document, () => rtdb.listen(
      hostsPath(uid),
      (event) => {
        const changes = mirror.apply(event);
        if (changes.empty && records.size > 0) return;
        const tree = mirror.get('/');
        records.clear();
        if (typeof tree === 'object' && tree !== null) {
          for (const [hostId, raw] of Object.entries(tree as Record<string, unknown>)) {
            const parsed = parseHostRecord(raw);
            if (!parsed.ok) continue;
            presence.observe(hostId, parsed.value);
            records.set(hostId, parsed.value);
          }
        }
        failed = false;
        publish();
      },
      () => {
        failed = true;
        publish();
      },
    ));
    // A host that died produces no events, so re-judge on a timer.
    const timer = window.setInterval(publish, PRESENCE_TICK_MS);
    const keys = getDeviceKeys();
    const refreshPins = (): void => {
      void keys.pinnedHosts().then((pins) => setState((previous) => ({ ...previous, pins })), () => undefined);
    };
    refreshPins();
    return () => {
      unsubscribe();
      window.clearInterval(timer);
    };
  }, [uid, rtdb]);
  return state;
}

/** Re-read the pinned computers after a pairing changed them. */
export function usePins(version: unknown): PinnedHost[] {
  const [pins, setPins] = useState<PinnedHost[]>([]);
  useEffect(() => {
    let live = true;
    void getDeviceKeys().pinnedHosts().then((list) => live && setPins(list), () => undefined);
    return () => {
      live = false;
    };
  }, [version]);
  return pins;
}

/** Whether storage works, and a re-render when it stops (private browsing). */
export function usePersistentPairing(): boolean {
  const keys = getDeviceKeys();
  return useSyncExternalStore(
    (listener) => keys.onDegraded(listener),
    () => keys.persistent,
  );
}

// ─────────────────────────────────────────────────────────────────────────────
// One connection
// ─────────────────────────────────────────────────────────────────────────────

export interface RemoteHost {
  connection: ViewerConnection;
  client: RemoteClient;
}

/**
 * Dial `hostId` for as long as the component is mounted. The connection
 * re-dials by itself; coming back to the page or regaining the network skips
 * the rest of its wait.
 */
export function useRemoteHost(uid: string, rtdb: Rtdb, hostId: string | null): RemoteHost | null {
  const [host, setHost] = useState<RemoteHost | null>(null);
  useEffect(() => {
    if (hostId === null) {
      setHost(null);
      return undefined;
    }
    const connection = new ViewerConnection({
      uid,
      hostId,
      rtdb,
      keys: getDeviceKeys(),
      ...(browserPeerConnectionFactory() === undefined ? {} : { peerConnections: browserPeerConnectionFactory()! }),
      forceTunnel: forceTunnelRequested(window.location.search, browserStorage('localStorage')),
      tunnelMemory: getTunnelMemory(),
    });
    const client = new RemoteClient(connection);
    const nudge = (): void => {
      if (document.visibilityState === 'visible') connection.nudge();
    };
    document.addEventListener('visibilitychange', nudge);
    window.addEventListener('online', nudge);
    connection.start();
    setHost({ connection, client });
    return () => {
      document.removeEventListener('visibilitychange', nudge);
      window.removeEventListener('online', nudge);
      client.dispose();
      connection.stop();
      setHost(null);
    };
  }, [uid, rtdb, hostId]);
  return host;
}

export function useConnectionState(connection: ViewerConnection | null): ConnectionState | null {
  return useSyncExternalStore(
    (listener) => (connection === null ? () => undefined : connection.subscribe(listener)),
    () => (connection === null ? null : connection.state),
  );
}

export function useStore<T>(store: { subscribe(listener: () => void): () => void; getState(): T } | null): T | null {
  return useSyncExternalStore(
    (listener) => (store === null ? () => undefined : store.subscribe(listener)),
    () => (store === null ? null : store.getState()),
  );
}
