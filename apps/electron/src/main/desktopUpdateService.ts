import type { DesktopUpdateRelease, DesktopUpdateState } from '../shared/desktopUpdates.js';

export const UPDATE_CHECK_INTERVAL_MS = 6 * 60 * 60 * 1_000;
export const UPDATE_REMINDER_DELAY_MS = 24 * 60 * 60 * 1_000;

interface Version { core: number[]; prerelease: string[] }

function parseVersion(value: unknown): Version | null {
  if (typeof value !== 'string' || value.length > 128) return null;
  const match = /^v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([\da-zA-Z-]+(?:\.[\da-zA-Z-]+)*))?(?:\+[\da-zA-Z-]+(?:\.[\da-zA-Z-]+)*)?$/.exec(value);
  if (!match) return null;
  const core = match.slice(1, 4).map(Number);
  const prerelease = match[4]?.split('.') ?? [];
  if (!core.every(Number.isSafeInteger) || prerelease.some(part => /^0\d+$/.test(part))) return null;
  return { core, prerelease };
}

/** SemVer precedence; build metadata never makes an installed version older. */
export function compareVersions(left: string, right: string): number | null {
  const a = parseVersion(left); const b = parseVersion(right);
  if (!a || !b) return null;
  for (let i = 0; i < 3; i++) {
    if (a.core[i] !== b.core[i]) return a.core[i] > b.core[i] ? 1 : -1;
  }
  if (!a.prerelease.length || !b.prerelease.length) {
    return Number(!a.prerelease.length) - Number(!b.prerelease.length);
  }
  for (let i = 0; i < Math.max(a.prerelease.length, b.prerelease.length); i++) {
    const x = a.prerelease[i]; const y = b.prerelease[i];
    if (x === undefined || y === undefined) return x === undefined ? -1 : 1;
    if (x === y) continue;
    const xn = /^\d+$/.test(x); const yn = /^\d+$/.test(y);
    if (xn && yn) return x.length !== y.length ? (x.length > y.length ? 1 : -1) : (x > y ? 1 : -1);
    if (xn !== yn) return xn ? -1 : 1;
    return x > y ? 1 : -1;
  }
  return 0;
}

export function isHttpsUpdateUrl(value: unknown): value is string {
  if (typeof value !== 'string' || value.length > 2_048) return false;
  try {
    const url = new URL(value);
    return url.protocol === 'https:' && !url.username && !url.password;
  } catch { return false; }
}

/** Only advertise published stable packages for this exact OS and architecture. */
export function selectDesktopUpdate(manifest: unknown, currentVersion: string, platform: string, arch: string): DesktopUpdateRelease | null {
  if (!manifest || typeof manifest !== 'object' || !('schemaVersion' in manifest) || manifest.schemaVersion !== 1
    || !('releases' in manifest) || !Array.isArray(manifest.releases)) throw new Error('Invalid Desktop update manifest');
  let latest: DesktopUpdateRelease | null = null;
  for (const entry of manifest.releases) {
    if (!entry || typeof entry !== 'object' || entry.platform !== platform || entry.arch !== arch) continue;
    const version = parseVersion(entry.version);
    if (!version || version.prerelease.length || !isHttpsUpdateUrl(entry.downloadUrl)) continue;
    if (compareVersions(entry.version, currentVersion) !== 1) continue;
    if (!latest || compareVersions(entry.version, latest.version) === 1) latest = { version: entry.version, downloadUrl: entry.downloadUrl };
  }
  return latest;
}

export interface UpdateSnooze { version: string; until: number }
interface UpdateServiceOptions {
  currentVersion: string;
  platform: string;
  arch: string;
  fetchManifest(): Promise<unknown>;
  openExternal(url: string): Promise<void>;
  readSnooze(): unknown;
  saveSnooze(value: UpdateSnooze): void;
  onChanged(state: DesktopUpdateState): void;
  onError(error: unknown): void;
  now?(): number;
}

export class DesktopUpdateService {
  private release: DesktopUpdateRelease | null = null;
  private snooze: UpdateSnooze | null = null;
  private inFlight: Promise<void> | null = null;
  private lastCheck: number | null = null;
  private disposed = false;
  private started = false;
  private checkTimer?: ReturnType<typeof setTimeout>;
  private reminderTimer?: ReturnType<typeof setTimeout>;

  constructor(private readonly options: UpdateServiceOptions) {
    try {
      const stored = options.readSnooze();
      if (stored && typeof stored === 'object' && 'version' in stored && typeof stored.version === 'string'
        && 'until' in stored && typeof stored.until === 'number' && Number.isFinite(stored.until)
        && stored.until > this.now() && stored.until <= this.now() + UPDATE_REMINDER_DELAY_MS) this.snooze = stored as UpdateSnooze;
    } catch { /* A missing or damaged reminder preference must not block startup. */ }
  }

  private now(): number { return this.options.now?.() ?? Date.now(); }

  getState(): DesktopUpdateState {
    const hidden = this.snooze !== null && this.release !== null
      && this.snooze.version === this.release.version && this.snooze.until > this.now();
    return { currentVersion: this.options.currentVersion, release: hidden ? null : this.release };
  }

  start(): void {
    if (this.disposed || this.started) return;
    this.started = true;
    this.scheduleCheck(20_000);
  }

  private scheduleCheck(delay: number): void {
    clearTimeout(this.checkTimer);
    this.checkTimer = setTimeout(() => { void this.check(); }, delay);
    this.checkTimer.unref();
  }

  /** Used on resume too; the interval also limits offline retries. */
  check(): Promise<void> {
    if (this.disposed) return Promise.resolve();
    if (this.inFlight) return this.inFlight;
    if (this.lastCheck !== null && this.now() - this.lastCheck < UPDATE_CHECK_INTERVAL_MS) {
      if (this.started) this.scheduleCheck(UPDATE_CHECK_INTERVAL_MS - (this.now() - this.lastCheck));
      return Promise.resolve();
    }
    this.lastCheck = this.now();
    this.inFlight = this.refresh().finally(() => {
      this.inFlight = null;
      if (this.started && !this.disposed) this.scheduleCheck(UPDATE_CHECK_INTERVAL_MS);
    });
    return this.inFlight;
  }

  private async refresh(): Promise<void> {
    try {
      const manifest = await this.options.fetchManifest();
      if (this.disposed) return;
      this.release = selectDesktopUpdate(manifest, this.options.currentVersion, this.options.platform, this.options.arch);
      this.publish();
    } catch (error) {
      // Keep an already discovered update visible across transient network errors.
      if (!this.disposed) this.options.onError(error);
    }
  }

  async download(): Promise<void> {
    if (this.disposed || !this.release) throw new Error('No Desktop update is available');
    // The renderer never supplies a URL or an executable path.
    await this.options.openExternal(this.release.downloadUrl);
  }

  remindLater(): void {
    if (this.disposed || !this.release) return;
    const snooze = { version: this.release.version, until: this.now() + UPDATE_REMINDER_DELAY_MS };
    this.options.saveSnooze(snooze);
    this.snooze = snooze;
    this.publish();
  }

  private publish(): void {
    clearTimeout(this.reminderTimer);
    this.options.onChanged(this.getState());
    if (this.release && this.snooze?.version === this.release.version && this.snooze.until > this.now()) {
      this.reminderTimer = setTimeout(() => { if (!this.disposed) this.publish(); }, this.snooze.until - this.now());
      this.reminderTimer.unref();
    }
  }

  dispose(): void {
    this.disposed = true;
    clearTimeout(this.checkTimer);
    clearTimeout(this.reminderTimer);
  }
}
