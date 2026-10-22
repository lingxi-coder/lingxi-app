import { generateIdentityKey, idFromKey, type WebCryptoKey } from '@lingxi/bridge-client/remote';
import { deviceNameFromUserAgent } from './text';

/** A computer this browser has paired with. `host_key` is what every later answer must be signed with. */
export interface PinnedHost {
  host_id: string;
  /** b64u of the host's raw Ed25519 key, pinned when the user approved this browser on the desktop. */
  host_key: string;
  name: string;
  paired_at: number;
}

/** This browser's long-term identity: the private half never exists as JS bytes. */
export interface DeviceIdentity {
  /** `idFromKey(publicKey)`, the id the desktop knows this browser by. */
  id: string;
  /** Raw 32-byte Ed25519 public key. */
  publicKey: Uint8Array;
  /** A non-extractable `CryptoKey`; it can sign and nothing else. */
  privateKey: WebCryptoKey;
}

/** What is kept between visits. Implementations may throw; {@link DeviceKeys} copes. */
export interface DeviceStore {
  /** The stored identity key pair, if any. */
  loadIdentity(): Promise<StoredIdentity | undefined>;
  /** Store `identity` unless one is already there (another tab won the race); returns what is stored afterwards. */
  saveIdentityIfAbsent(identity: StoredIdentity): Promise<StoredIdentity>;
  deleteIdentity(): Promise<void>;
  listHosts(): Promise<PinnedHost[]>;
  putHost(host: PinnedHost): Promise<void>;
  deleteHost(hostId: string): Promise<void>;
}

export interface StoredIdentity {
  publicKey: Uint8Array;
  privateKey: WebCryptoKey;
}

/** Keeps everything in this tab only; the fallback when storage is unavailable, and the test double. */
export class MemoryDeviceStore implements DeviceStore {
  private identity: StoredIdentity | undefined;
  private readonly hosts = new Map<string, PinnedHost>();

  async loadIdentity(): Promise<StoredIdentity | undefined> {
    return this.identity;
  }

  async saveIdentityIfAbsent(identity: StoredIdentity): Promise<StoredIdentity> {
    this.identity ??= identity;
    return this.identity;
  }

  async deleteIdentity(): Promise<void> {
    this.identity = undefined;
  }

  async listHosts(): Promise<PinnedHost[]> {
    return [...this.hosts.values()];
  }

  async putHost(host: PinnedHost): Promise<void> {
    this.hosts.set(host.host_id, { ...host });
  }

  async deleteHost(hostId: string): Promise<void> {
    this.hosts.delete(hostId);
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// IndexedDB
// ─────────────────────────────────────────────────────────────────────────────

const DB_NAME = 'lingxi-remote';
const DB_VERSION = 1;
const IDENTITY_STORE = 'identity';
const HOSTS_STORE = 'hosts';
const IDENTITY_KEY = 'device';

function settled<T>(request: IDBRequest<T>): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error ?? new Error('IndexedDB request failed'));
  });
}

function finished(transaction: IDBTransaction): Promise<void> {
  return new Promise<void>((resolve, reject) => {
    transaction.oncomplete = () => resolve();
    transaction.onerror = () => reject(transaction.error ?? new Error('IndexedDB transaction failed'));
    transaction.onabort = () => reject(transaction.error ?? new Error('IndexedDB transaction aborted'));
  });
}

function openDatabase(factory: IDBFactory): Promise<IDBDatabase> {
  return new Promise<IDBDatabase>((resolve, reject) => {
    const request = factory.open(DB_NAME, DB_VERSION);
    request.onupgradeneeded = () => {
      const db = request.result;
      if (!db.objectStoreNames.contains(IDENTITY_STORE)) db.createObjectStore(IDENTITY_STORE);
      if (!db.objectStoreNames.contains(HOSTS_STORE)) db.createObjectStore(HOSTS_STORE, { keyPath: 'host_id' });
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error ?? new Error('IndexedDB open failed'));
    request.onblocked = () => reject(new Error('IndexedDB open blocked'));
  });
}

function isStoredIdentity(value: unknown): value is StoredIdentity {
  const record = value as Partial<StoredIdentity> | undefined;
  return record !== undefined && record !== null && record.publicKey instanceof Uint8Array && record.publicKey.length === 32 && record.privateKey !== undefined;
}

function isPinnedHost(value: unknown): value is PinnedHost {
  const record = value as Partial<PinnedHost> | undefined;
  return (
    typeof record === 'object' &&
    record !== null &&
    typeof record.host_id === 'string' &&
    typeof record.host_key === 'string' &&
    typeof record.name === 'string' &&
    typeof record.paired_at === 'number'
  );
}

/** The device key and the pinned computers in IndexedDB (`lingxi-remote`), which structured-clones a non-extractable `CryptoKey`. */
export class IndexedDbDeviceStore implements DeviceStore {
  private opened: Promise<IDBDatabase> | undefined;

  constructor(private readonly factory: IDBFactory) {}

  private database(): Promise<IDBDatabase> {
    this.opened ??= openDatabase(this.factory).catch((error: unknown) => {
      this.opened = undefined; // a later call may succeed, e.g. after the user allows storage
      throw error;
    });
    return this.opened;
  }

  async loadIdentity(): Promise<StoredIdentity | undefined> {
    const db = await this.database();
    const value: unknown = await settled(db.transaction(IDENTITY_STORE, 'readonly').objectStore(IDENTITY_STORE).get(IDENTITY_KEY));
    return isStoredIdentity(value) ? { publicKey: value.publicKey, privateKey: value.privateKey } : undefined;
  }

  async saveIdentityIfAbsent(identity: StoredIdentity): Promise<StoredIdentity> {
    const db = await this.database();
    const transaction = db.transaction(IDENTITY_STORE, 'readwrite');
    const store = transaction.objectStore(IDENTITY_STORE);
    // One transaction for the check and the write, so two tabs opening the page together end up with one key.
    const existing: unknown = await settled(store.get(IDENTITY_KEY));
    if (isStoredIdentity(existing)) {
      await finished(transaction);
      return { publicKey: existing.publicKey, privateKey: existing.privateKey };
    }
    store.put({ publicKey: identity.publicKey, privateKey: identity.privateKey }, IDENTITY_KEY);
    await finished(transaction);
    return identity;
  }

  async deleteIdentity(): Promise<void> {
    const db = await this.database();
    const transaction = db.transaction(IDENTITY_STORE, 'readwrite');
    transaction.objectStore(IDENTITY_STORE).delete(IDENTITY_KEY);
    await finished(transaction);
  }

  async listHosts(): Promise<PinnedHost[]> {
    const db = await this.database();
    const values: unknown[] = await settled(db.transaction(HOSTS_STORE, 'readonly').objectStore(HOSTS_STORE).getAll());
    return values.filter(isPinnedHost).map((host) => ({ ...host }));
  }

  async putHost(host: PinnedHost): Promise<void> {
    const db = await this.database();
    const transaction = db.transaction(HOSTS_STORE, 'readwrite');
    transaction.objectStore(HOSTS_STORE).put({ ...host });
    await finished(transaction);
  }

  async deleteHost(hostId: string): Promise<void> {
    const db = await this.database();
    const transaction = db.transaction(HOSTS_STORE, 'readwrite');
    transaction.objectStore(HOSTS_STORE).delete(hostId);
    await finished(transaction);
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// DeviceKeys
// ─────────────────────────────────────────────────────────────────────────────

export interface DeviceKeysOptions {
  /** The persistent store; omitted or `undefined` means this browser has none (e.g. IndexedDB is missing). */
  store?: DeviceStore;
  /** Defaults to a fresh non-extractable Ed25519 key. */
  generate?: () => Promise<StoredIdentity>;
}

async function generateDeviceKey(): Promise<StoredIdentity> {
  const key = await generateIdentityKey({ extractable: false });
  return { publicKey: key.publicKey, privateKey: key.privateKey };
}

/**
 * This browser's identity and the computers it trusts.
 *
 * Every access to the persistent store is guarded: if the store is missing or
 * throws (private browsing, blocked site data, a full disk) the object switches
 * to an in-memory store for the rest of the tab's life and tells its listeners
 * once, so the page can say that pairing will not survive closing the tab.
 */
export class DeviceKeys {
  private store: DeviceStore;
  private degraded: boolean;
  private identityPromise: Promise<DeviceIdentity> | undefined;
  private readonly listeners = new Set<() => void>();
  private readonly generate: () => Promise<StoredIdentity>;

  constructor(options: DeviceKeysOptions = {}) {
    this.degraded = options.store === undefined;
    this.store = options.store ?? new MemoryDeviceStore();
    this.generate = options.generate ?? generateDeviceKey;
  }

  /** `false` once pairing can no longer be kept across visits. */
  get persistent(): boolean {
    return !this.degraded;
  }

  /** Be told when {@link persistent} turns `false`. Returns the unsubscribe function. */
  onDegraded(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  /** Run a store operation; on failure fall back to memory and repeat it there. */
  private async guarded<T>(operation: (store: DeviceStore) => Promise<T>): Promise<T> {
    try {
      return await operation(this.store);
    } catch (error) {
      if (this.degraded) throw error; // the in-memory store does not fail
      this.degrade();
      return operation(this.store);
    }
  }

  private degrade(): void {
    this.degraded = true;
    this.store = new MemoryDeviceStore();
    for (const listener of [...this.listeners]) {
      try {
        listener();
      } catch {
        // Listeners are independent of each other.
      }
    }
  }

  /** The device identity, created on first use. Concurrent callers share one creation. */
  identity(): Promise<DeviceIdentity> {
    this.identityPromise ??= this.loadOrCreate().catch((error: unknown) => {
      this.identityPromise = undefined;
      throw error;
    });
    return this.identityPromise;
  }

  private async loadOrCreate(): Promise<DeviceIdentity> {
    let stored = await this.guarded((store) => store.loadIdentity());
    if (stored === undefined) {
      const fresh = await this.generate();
      stored = await this.guarded((store) => store.saveIdentityIfAbsent(fresh));
    }
    return { id: await idFromKey(stored.publicKey), publicKey: stored.publicKey, privateKey: stored.privateKey };
  }

  /**
   * Throw the device key away; the next {@link identity} makes a new one. This
   * is what "pair again" after a revocation does: the desktop remembers the
   * revoked id and would refuse the old key forever.
   */
  async resetIdentity(): Promise<void> {
    this.identityPromise = undefined;
    await this.guarded((store) => store.deleteIdentity());
  }

  async pinnedHosts(): Promise<PinnedHost[]> {
    return (await this.guarded((store) => store.listHosts())).sort((a, b) => b.paired_at - a.paired_at);
  }

  async pinnedHost(hostId: string): Promise<PinnedHost | undefined> {
    return (await this.pinnedHosts()).find((host) => host.host_id === hostId);
  }

  /** Remember a computer's key once the user has approved this browser on it. */
  async pin(host: PinnedHost): Promise<void> {
    await this.guarded((store) => store.putHost(host));
  }

  /** Forget a computer. The desktop still lists this browser until the user removes it there. */
  async unpair(hostId: string): Promise<void> {
    await this.guarded((store) => store.deleteHost(hostId));
  }
}

/** The browser's `indexedDB`, or `undefined` where reading it throws or it is missing. */
export function browserDeviceStore(): DeviceStore | undefined {
  try {
    return typeof indexedDB === 'undefined' ? undefined : new IndexedDbDeviceStore(indexedDB);
  } catch {
    return undefined;
  }
}

/** The name sent in the offer: browser and system, at most 64 characters. */
export function browserDeviceName(): string {
  if (typeof navigator === 'undefined') return 'Browser · Web';
  const hint = (navigator as Navigator & { userAgentData?: { platform?: string } }).userAgentData?.platform;
  return deviceNameFromUserAgent(navigator.userAgent, hint);
}
