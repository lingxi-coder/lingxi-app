import { mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';

import {
  defaultSettings,
  parseSettings,
  publicSettings,
  setWorkspaceTrust,
  withRecentWorkspace,
  workspaceTrust,
  type PersistedSettings,
  type PublicSettings,
} from './host-utils.js';

export class SettingsStore {
  readonly settingsPath: string;
  private settings: PersistedSettings;

  constructor(userData: string) {
    this.settingsPath = join(userData, 'settings.v1.json');
    this.settings = this.readSettings();
  }

  private readSettings(): PersistedSettings {
    try {
      const settings = parseSettings(JSON.parse(readFileSync(this.settingsPath, 'utf8')) as unknown);
      if (settings.apiBaseUrl) {
        try { settings.apiBaseUrl = validateApiBaseUrl(settings.apiBaseUrl); }
        catch { delete settings.apiBaseUrl; }
      }
      return settings;
    } catch {
      return defaultSettings();
    }
  }

  private persist(): void {
    mkdirSync(dirname(this.settingsPath), { recursive: true, mode: 0o700 });
    const temporary = `${this.settingsPath}.tmp`;
    writeFileSync(temporary, `${JSON.stringify(this.settings, null, 2)}\n`, { mode: 0o600 });
    renameSync(temporary, this.settingsPath);
  }

  getPublic(): PublicSettings {
    return publicSettings(this.settings);
  }

  getWorkspace(): string | undefined {
    return this.settings.lastWorkspace;
  }

  setWorkspace(canonical: string): void {
    this.settings = withRecentWorkspace(this.settings, canonical);
    this.persist();
  }

  isRecentWorkspace(canonical: string): boolean {
    return this.settings.recentWorkspaces.includes(canonical);
  }

  update(patch: { theme?: 'dark' | 'light'; model?: string | null; apiBaseUrl?: string | null }): PublicSettings {
    if ('theme' in patch) {
      if (patch.theme !== 'dark' && patch.theme !== 'light') throw new Error('invalid theme');
      this.settings.theme = patch.theme;
    }
    if ('model' in patch) {
      if (patch.model === null || patch.model === '') delete this.settings.model;
      else this.settings.model = validateString(patch.model, 'model', 256);
    }
    if ('apiBaseUrl' in patch) {
      if (patch.apiBaseUrl === null || patch.apiBaseUrl === '') delete this.settings.apiBaseUrl;
      else this.settings.apiBaseUrl = validateApiBaseUrl(patch.apiBaseUrl);
    }
    this.persist();
    return this.getPublic();
  }

  getTrust(workspace: string): { trusted: boolean; fingerprint: string } {
    return workspaceTrust(this.settings, workspace);
  }

  setTrust(workspace: string, trusted: boolean): { trusted: boolean; fingerprint: string } {
    this.settings = setWorkspaceTrust(this.settings, workspace, trusted);
    this.persist();
    return workspaceTrust(this.settings, workspace);
  }

  /** Whether the user has previously accepted Bypass Permissions mode. */
  getBypassPermissionsAccepted(): boolean {
    return this.settings.bypassPermissionsModeAccepted === true;
  }

  /** Persist the one-time Bypass Permissions acceptance (oracle
   * `bypassPermissionsModeAccepted`). */
  setBypassPermissionsAccepted(accepted: boolean): void {
    if (accepted) this.settings.bypassPermissionsModeAccepted = true;
    else delete this.settings.bypassPermissionsModeAccepted;
    this.persist();
  }

}

function validateString(value: unknown, name: string, max: number): string {
  if (typeof value !== 'string' || value.length === 0 || value.length > max || value.includes('\0')) {
    throw new Error(`invalid ${name}`);
  }
  return value;
}

function validateApiBaseUrl(value: unknown): string {
  const raw = validateString(value, 'API base URL', 2_048);
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    throw new Error('invalid API base URL');
  }
  const loopback = url.hostname === 'localhost' || url.hostname === '127.0.0.1' || url.hostname === '::1';
  if (url.protocol !== 'https:' && !(url.protocol === 'http:' && loopback)) {
    throw new Error('API base URL must use HTTPS (HTTP is allowed only for loopback)');
  }
  if (url.username || url.password || url.search || url.hash) {
    throw new Error('API base URL must not contain credentials, query parameters, or fragments');
  }
  return url.toString().replace(/\/$/, '');
}
