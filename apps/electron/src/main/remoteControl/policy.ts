/**
 * Who may be shown what: the managed `disableRemoteControl` veto, which
 * sessions have Remote Control on, and the two host settings that shape it
 * (`remoteControlAtStartup`, and whether a viewer may answer "always allow").
 *
 * These keys live in the Electron settings file (`settings.v1.json`) and in
 * the administrator's `managed-settings.json`, not in the engine's settings:
 * the engine validates its settings against a closed schema, the host must
 * decide before any engine runs, and a session list must be answerable while
 * every runtime is asleep.
 */

import { readFileSync } from 'node:fs';
import { join } from 'node:path';

import type { SessionRef } from '../bridgeTypes.js';
import { productEnvironmentKey, productNamespace } from '../productNamespace.js';

/** The slice of `SettingsStore` the policy uses. */
export interface RemotePolicySettings {
  getRemoteSessions(): SessionRef[];
  setRemoteSession(ref: SessionRef, enabled: boolean): void;
  isSessionArchived(ref: SessionRef): boolean;
  getRemoteControlAtStartup(): boolean;
  setRemoteControlAtStartup(enabled: boolean): void;
  getRemoteAllowAlwaysAllow(): boolean;
}

export interface RemotePolicyOptions {
  settings: RemotePolicySettings;
  /** The directory that holds `managed-settings.json`; `undefined` when there is none to read. */
  managedDirectory?: string | undefined;
  /** For tests. */
  readFile?: (path: string) => string;
  now?: () => number;
}

/** How long a read of the managed file is trusted. */
const MANAGED_CACHE_MS = 5_000;
const MANAGED_FILE_MAX_BYTES = 1024 * 1024;

/**
 * Where the engine looks for the administrator's settings (the `MANAGED_DIR_*`
 * constants of the `branding` crate), unless `LINGXI_MANAGED_DIR` moves it.
 */
export function managedSettingsDirectory(env: Readonly<Record<string, string | undefined>>, platform: NodeJS.Platform): string {
  const override = env[productEnvironmentKey('MANAGED_DIR')];
  if (override) return override;
  if (platform === 'darwin') return productNamespace.managedDirMacOS;
  if (platform === 'win32') return productNamespace.managedDirWindows;
  return productNamespace.managedDirUnix;
}

export class RemotePolicy {
  private readonly listeners = new Set<() => void>();
  private readonly everEnabled = new Set<string>();
  private readonly readFile: (path: string) => string;
  private readonly now: () => number;
  private managed: { at: number; disabled: boolean } | undefined;

  constructor(private readonly options: RemotePolicyOptions) {
    this.readFile = options.readFile ?? ((path) => readFileSync(path, 'utf8'));
    this.now = options.now ?? Date.now;
    for (const ref of options.settings.getRemoteSessions()) this.everEnabled.add(ref.sessionId);
  }

  /** The administrator's veto: `{"disableRemoteControl": true}` in `managed-settings.json`. */
  managedDisabled(): boolean {
    const at = this.now();
    if (this.managed !== undefined && at - this.managed.at < MANAGED_CACHE_MS) return this.managed.disabled;
    this.managed = { at, disabled: this.readManaged() };
    return this.managed.disabled;
  }

  /** Whether Remote Control may run at all. */
  allows(): boolean {
    return !this.managedDisabled();
  }

  /** Read the managed file again; true if the veto changed. The listeners hear of a change. */
  recheck(): boolean {
    const before = this.managed?.disabled ?? false;
    this.managed = undefined;
    const after = this.managedDisabled();
    if (after !== before) this.emit();
    return after !== before;
  }

  private readManaged(): boolean {
    const directory = this.options.managedDirectory;
    if (directory === undefined) return false;
    let text: string;
    try {
      text = this.readFile(join(directory, 'managed-settings.json'));
    } catch {
      return false;
    }
    if (text.length > MANAGED_FILE_MAX_BYTES) return false;
    try {
      const parsed: unknown = JSON.parse(text);
      return typeof parsed === 'object' && parsed !== null && !Array.isArray(parsed)
        && (parsed as Record<string, unknown>)['disableRemoteControl'] === true;
    } catch {
      return false;
    }
  }

  // ───────────────────────────────────────────────────────────────────────────
  // Sessions
  // ───────────────────────────────────────────────────────────────────────────

  /** The sessions viewers may see: the ones with Remote Control on, minus archived ones. */
  enabledSessions(): SessionRef[] {
    return this.options.settings.getRemoteSessions().filter((ref) => !this.options.settings.isSessionArchived(ref));
  }

  /** The session with this id, if Remote Control is on for it. */
  find(sessionId: string): SessionRef | undefined {
    return this.enabledSessions().find((ref) => ref.sessionId === sessionId);
  }

  isEnabled(sessionId: string): boolean {
    return this.find(sessionId) !== undefined;
  }

  /** Whether Remote Control was on for this session at some point since the app started (the difference between "unknown" and "switched off"). */
  wasEnabled(sessionId: string): boolean {
    return this.everEnabled.has(sessionId);
  }

  /** Switch one session on or off. Throws if the setting could not be saved; the listeners then hear nothing. */
  setSessionEnabled(ref: SessionRef, enabled: boolean): void {
    if (this.isEnabled(ref.sessionId) === enabled) return;
    this.options.settings.setRemoteSession(ref, enabled);
    this.everEnabled.add(ref.sessionId);
    this.emit();
  }

  /** A session was created (its first prompt was sent): switch it on if `remoteControlAtStartup` asks for that. */
  sessionCreated(ref: SessionRef): void {
    if (this.options.settings.getRemoteControlAtStartup()) this.setSessionEnabled(ref, true);
  }

  getAtStartup(): boolean {
    return this.options.settings.getRemoteControlAtStartup();
  }

  setAtStartup(enabled: boolean): void {
    this.options.settings.setRemoteControlAtStartup(enabled);
  }

  /** Whether a viewer may answer a permission prompt with "always allow" (and "allow and switch to auto"). */
  allowAlwaysAllow(): boolean {
    return this.options.settings.getRemoteAllowAlwaysAllow();
  }

  /** Be told when the veto or the set of enabled sessions changes. */
  onChange(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  }

  /** Sessions were archived, removed or restored by someone else: let the listeners re-read. */
  notifySessionsChanged(): void {
    this.emit();
  }

  private emit(): void {
    for (const listener of [...this.listeners]) {
      try { listener(); } catch { /* One listener cannot break the others. */ }
    }
  }
}
