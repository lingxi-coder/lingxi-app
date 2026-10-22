/**
 * The viewers this computer has approved, per Firebase account, in
 * `userData/remote-control/devices.json`. A device is approved by the user
 * through the pairing dialog and stays approved until revoked. Entries hold
 * public keys only. The file is written whole through a temporary file and a
 * rename, readable by the user alone.
 */

import { mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { dirname } from 'node:path';
import { isB64u, systemClock, type Clock, type DevicePlatform } from '@lingxi/bridge-client/remote';

import { describeError, noDiagnostics, type RemoteDiagnostics } from './diagnostics.js';

export interface RegisteredDevice {
  device_id: string;
  /** b64u of the 32-byte raw device key. */
  key: string;
  name: string;
  platform: DevicePlatform;
  approved_at: number;
  last_seen: number;
}

/** What approving a device needs to know; the registry stamps the times. */
export type ApprovedDeviceInput = Pick<RegisteredDevice, 'device_id' | 'key' | 'name' | 'platform'>;

/** The file operations the registry uses; `node:fs` in production. */
export interface RegistryFiles {
  mkdirSync(path: string, options: { recursive: true; mode: number }): unknown;
  readFileSync(path: string, encoding: 'utf8'): string;
  writeFileSync(path: string, data: string, options: { mode: number }): void;
  renameSync(from: string, to: string): void;
  rmSync(path: string, options: { force: true }): void;
}

export interface DeviceRegistryOptions {
  path: string;
  clock?: Clock;
  files?: RegistryFiles;
  diagnostics?: RemoteDiagnostics;
  /** How long a revoked device is remembered and answered `rejected/revoked`; default 10 minutes. */
  revokedMemoryMs?: number;
}

const FILE_VERSION = 1;
const DEFAULT_REVOKED_MEMORY_MS = 10 * 60_000;
// Bounds on what a damaged or hostile file may make the registry hold.
const MAX_ACCOUNTS = 64;
const MAX_DEVICES_PER_ACCOUNT = 256;
const MAX_FILE_BYTES = 4 * 1024 * 1024;
const ID = /^[A-Za-z0-9_-]{22}$/;
const PLATFORMS: readonly string[] = ['web', 'android', 'ios'];
const NAME_UNSAFE = /[\u0000-\u001f\u007f-\u009f\u2028\u2029]/;

type Json = Record<string, unknown>;

const isObject = (value: unknown): value is Json => typeof value === 'object' && value !== null && !Array.isArray(value);
const isTime = (value: unknown): value is number => typeof value === 'number' && Number.isFinite(value) && value >= 0;

function parseDevice(value: unknown): RegisteredDevice | undefined {
  if (!isObject(value)) return undefined;
  const { device_id, key, name, platform, approved_at, last_seen } = value;
  if (typeof device_id !== 'string' || !ID.test(device_id) || !isB64u(key, 32)) return undefined;
  if (typeof name !== 'string' || name.length < 1 || name.length > 128 || NAME_UNSAFE.test(name)) return undefined;
  if (typeof platform !== 'string' || !PLATFORMS.includes(platform) || !isTime(approved_at) || !isTime(last_seen)) return undefined;
  return { device_id, key, name, platform: platform as DevicePlatform, approved_at, last_seen };
}

export class DeviceRegistry {
  private readonly path: string;
  private readonly clock: Clock;
  private readonly files: RegistryFiles;
  private readonly diagnostics: RemoteDiagnostics;
  private readonly revokedMemoryMs: number;
  // account uid → device id → device. Maps keep hostile keys such as `__proto__` inert.
  private accounts = new Map<string, Map<string, RegisteredDevice>>();
  private readonly revoked = new Map<string, number>();
  private readonly listeners = new Set<(uid: string) => void>();

  constructor(options: DeviceRegistryOptions) {
    this.path = options.path;
    this.clock = options.clock ?? systemClock;
    this.files = options.files ?? { mkdirSync, readFileSync, writeFileSync, renameSync, rmSync };
    this.diagnostics = options.diagnostics ?? noDiagnostics;
    this.revokedMemoryMs = options.revokedMemoryMs ?? DEFAULT_REVOKED_MEMORY_MS;
    this.load();
  }

  private load(): void {
    let text: string;
    try {
      text = this.files.readFileSync(this.path, 'utf8');
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ENOENT') this.setAside(error);
      return;
    }
    try {
      if (text.length > MAX_FILE_BYTES) throw new Error('file is too large');
      const parsed: unknown = JSON.parse(text);
      if (!isObject(parsed) || parsed['version'] !== FILE_VERSION || !isObject(parsed['users'])) throw new Error('unrecognized format');
      for (const [uid, list] of Object.entries(parsed['users']).slice(0, MAX_ACCOUNTS)) {
        if (!Array.isArray(list)) continue;
        const devices = new Map<string, RegisteredDevice>();
        for (const entry of list.slice(0, MAX_DEVICES_PER_ACCOUNT)) {
          const device = parseDevice(entry);
          if (device) devices.set(device.device_id, device);
        }
        if (devices.size > 0) this.accounts.set(uid, devices);
      }
    } catch (error) {
      this.accounts.clear();
      this.setAside(error);
    }
  }

  /** Keep an unreadable file for inspection instead of overwriting it with the next change. */
  private setAside(error: unknown): void {
    this.diagnostics('warn', `devices.json is unreadable (${describeError(error)}); starting with no approved devices`);
    try { this.files.renameSync(this.path, `${this.path}.unreadable`); } catch { /* Nothing more to do for it. */ }
  }

  /** Write every account's devices, whole and atomically. */
  private persist(accounts: ReadonlyMap<string, ReadonlyMap<string, RegisteredDevice>>): void {
    const users = Object.fromEntries([...accounts].map(([uid, devices]) => [uid, [...devices.values()]]));
    const temporary = `${this.path}.tmp`;
    this.files.mkdirSync(dirname(this.path), { recursive: true, mode: 0o700 });
    try {
      this.files.writeFileSync(temporary, `${JSON.stringify({ version: FILE_VERSION, users }, null, 2)}\n`, { mode: 0o600 });
      this.files.renameSync(temporary, this.path);
    } catch (error) {
      try { this.files.rmSync(temporary, { force: true }); } catch { /* The next write replaces it. */ }
      throw error;
    }
  }

  private emit(uid: string): void {
    for (const listener of [...this.listeners]) {
      try { listener(uid); } catch { /* One listener cannot break the others. */ }
    }
  }

  private change(uid: string, edit: (devices: Map<string, RegisteredDevice>) => void): void {
    const devices = new Map(this.accounts.get(uid) ?? []);
    edit(devices);
    const next = new Map(this.accounts);
    if (devices.size > 0) next.set(uid, devices);
    else next.delete(uid);
    this.accounts = next;
    this.emit(uid);
    try {
      this.persist(next);
    } catch (error) {
      this.diagnostics('warn', `devices.json could not be saved (${describeError(error)}); the change holds until restart`);
      throw error;
    }
  }

  isApproved(uid: string, deviceId: string, key: string): boolean {
    const device = this.accounts.get(uid)?.get(deviceId);
    return device !== undefined && device.key === key;
  }

  /** Approve a device, or refresh its name if it is known. Clears a revocation. A failed save is reported, not thrown. */
  approve(uid: string, device: ApprovedDeviceInput): void {
    const now = this.clock.now();
    this.revoked.delete(`${uid}\0${device.device_id}`);
    try {
      this.change(uid, (devices) => {
        const known = devices.get(device.device_id);
        devices.set(device.device_id, { ...device, approved_at: known?.approved_at ?? now, last_seen: now });
      });
    } catch {
      // Already reported; the user said yes, so the device stays approved until the app restarts.
    }
  }

  /**
   * Forget a device and answer its next offers `rejected/revoked` for a while.
   * The revocation holds in memory even when the file cannot be written; that
   * failure is then thrown, so the caller can tell the user it will not last.
   * Returns whether the device was approved.
   */
  revoke(uid: string, deviceId: string): boolean {
    const known = this.accounts.get(uid)?.has(deviceId) === true;
    this.revoked.set(`${uid}\0${deviceId}`, this.clock.now() + this.revokedMemoryMs);
    if (known) this.change(uid, (devices) => void devices.delete(deviceId));
    return known;
  }

  /** Whether `deviceId` was revoked recently enough to be refused outright. */
  isRevoked(uid: string, deviceId: string): boolean {
    const key = `${uid}\0${deviceId}`;
    const until = this.revoked.get(key);
    if (until === undefined) return false;
    if (this.clock.now() < until) return true;
    this.revoked.delete(key);
    return false;
  }

  /** Approved devices, most recently seen first. */
  list(uid: string): RegisteredDevice[] {
    return [...(this.accounts.get(uid)?.values() ?? [])].map((device) => ({ ...device })).sort((a, b) => b.last_seen - a.last_seen);
  }

  /** Record that an approved device was just in use. Best effort. */
  touch(uid: string, deviceId: string): void {
    if (!this.accounts.get(uid)?.has(deviceId)) return;
    const now = this.clock.now();
    try {
      this.change(uid, (devices) => {
        const device = devices.get(deviceId);
        if (device) devices.set(deviceId, { ...device, last_seen: now });
      });
    } catch {
      // Already reported.
    }
  }

  /** Be told, with the account's uid, after the device list of an account changed. */
  onChange(listener: (uid: string) => void): () => void {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  }
}
