import { mkdirSync, readFileSync, renameSync, unlinkSync, writeFileSync } from 'node:fs';
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

export interface EncryptionProvider {
  isEncryptionAvailable(): boolean;
  encryptString(plainText: string): Buffer;
  decryptString(encrypted: Buffer): string;
}

export interface CredentialKeychain {
  readonly available: boolean;
  has(providerId: string): boolean;
  read(providerId: string): string | undefined;
  write(providerId: string, value: string): void;
  clear(providerId: string): void;
}

export interface CredentialMetadata {
  configured: boolean;
  encryptionAvailable: boolean;
  /** The credential exists only in main-process memory and is lost on app exit. */
  sessionOnly?: true;
  /** The running engine received a credential from an external runtime source. */
  runtimeOnly?: true;
}

export interface ProviderCredentialMetadata extends CredentialMetadata {
  providerId: string;
}

export class SettingsStore {
  readonly settingsPath: string;
  readonly credentialPath: string;
  private settings: PersistedSettings;
  /** Plaintext exists only for the lifetime of this main-process instance. */
  private readonly sessionCredentials = new Map<string, string>();
  /** Providers that have no persisted ciphertext or Keychain item. */
  private readonly sessionOnlyCredentials = new Set<string>();

  constructor(
    userData: string,
    private readonly encryption: EncryptionProvider,
    private readonly keychain?: CredentialKeychain,
  ) {
    this.settingsPath = join(userData, 'settings.v1.json');
    this.credentialPath = join(userData, 'credential.bin');
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

  credentialMetadata(): CredentialMetadata {
    const { providerId: _providerId, ...metadata } = this.providerCredentialMetadata('anthropic');
    return metadata;
  }

  setCredential(value: string): CredentialMetadata {
    this.setProviderCredential('anthropic', value);
    return this.credentialMetadata();
  }

  clearCredential(): CredentialMetadata {
    this.clearProviderCredential('anthropic');
    return this.credentialMetadata();
  }

  /** Read only in the main process immediately before spawning the bridge. */
  readCredential(): string | undefined {
    return this.readProviderCredential('anthropic');
  }

  providerCredentialMetadata(providerId: string): ProviderCredentialMetadata {
    const normalized = validateProviderId(providerId);
    if (this.sessionOnlyCredentials.has(normalized)) {
      return { providerId: normalized, configured: true, encryptionAvailable: false, sessionOnly: true };
    }
    const path = this.providerCredentialPath(providerId);
    let configured = Boolean(this.keychain?.available && this.keychain.has(normalized));
    try {
      readFileSync(path);
      configured = true;
    } catch {
      // Absence and unreadability are both reported as not configured; no secret leaks.
    }
    // Metadata must stay non-blocking. Checking the generic-password item does
    // not read its value; legacy Safe Storage decryption is deferred until the
    // launch path needs a real credential and can migrate it.
    return {
      providerId: normalized,
      configured,
      encryptionAvailable: this.keychain ? this.keychain.available : true,
    };
  }

  providerCredentialMetadataFor(providerIds: readonly string[]): ProviderCredentialMetadata[] {
    return providerIds.map((providerId) => this.providerCredentialMetadata(providerId));
  }

  setProviderCredential(providerId: string, value: string): ProviderCredentialMetadata {
    const credential = validateString(value, 'credential', 16_384);
    const normalized = validateProviderId(providerId);
    if (this.keychain) {
      if (this.keychain.available) try {
        this.keychain.write(normalized, credential);
        if (this.keychain.read(normalized) !== credential) throw new Error('keychain verification failed');
        this.removeLegacyCredential(normalized);
        this.sessionCredentials.set(normalized, credential);
        this.sessionOnlyCredentials.delete(normalized);
        return this.providerCredentialMetadata(normalized);
      } catch {
        // A temporarily unavailable secure store must not prevent the current
        // desktop session from starting. Never persist a plaintext fallback.
      }
      this.sessionCredentials.set(normalized, credential);
      this.sessionOnlyCredentials.add(normalized);
      return this.providerCredentialMetadata(normalized);
    }
    const path = this.providerCredentialPath(providerId);
    let encrypted: Buffer;
    try {
      encrypted = this.encryption.encryptString(credential);
    } catch {
      throw new Error('secure credential storage is unavailable');
    }
    mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
    const temporary = `${path}.tmp`;
    writeFileSync(temporary, encrypted, { mode: 0o600 });
    renameSync(temporary, path);
    this.sessionCredentials.set(normalized, credential);
    this.sessionOnlyCredentials.delete(normalized);
    return this.providerCredentialMetadata(normalized);
  }

  clearProviderCredential(providerId: string): ProviderCredentialMetadata {
    const normalized = validateProviderId(providerId);
    if (this.keychain) {
      if (this.keychain.available) this.keychain.clear(normalized);
      this.removeLegacyCredential(normalized);
    } else {
      const path = this.providerCredentialPath(normalized);
      try {
        unlinkSync(path);
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
      }
    }
    this.sessionCredentials.delete(normalized);
    this.sessionOnlyCredentials.delete(normalized);
    return this.providerCredentialMetadata(normalized);
  }

  /** Decrypt provider keys only in the main process immediately before spawning the bridge. */
  readProviderCredential(providerId: string): string | undefined {
    const normalized = validateProviderId(providerId);
    const sessionValue = this.sessionCredentials.get(normalized);
    if (sessionValue !== undefined) return sessionValue;
    // Query item metadata before requesting its secret. On macOS, reading a
    // generic-password value can trigger Keychain authorization; probing every
    // supported provider during startup would serialize those prompts/timeouts
    // even though nearly every legacy item is absent.
    if (this.keychain?.available && this.keychain.has(normalized)) {
      const value = this.keychain.read(normalized);
      if (value !== undefined) {
        this.sessionCredentials.set(normalized, value);
        return value;
      }
    }
    const legacy = this.readLegacyCredential(normalized);
    // Do not write this value back into Electron's compatibility Keychain.
    // BridgeManager migrates it directly into the engine-owned store after the
    // connection is ready and clears this file only after that write succeeds.
    return legacy;
  }

  private readLegacyCredential(providerId: string): string | undefined {
    let encrypted: Buffer;
    try {
      encrypted = readFileSync(this.providerCredentialPath(providerId));
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') return undefined;
      throw error;
    }
    try {
      return this.encryption.decryptString(encrypted);
    } catch {
      // A stale or unavailable keychain must not prevent the desktop shell
      // from starting. The provider will be shown as configured and can be
      // replaced from Settings once secure storage is available again.
      return undefined;
    }
  }

  private removeLegacyCredential(providerId: string): void {
    try {
      unlinkSync(this.providerCredentialPath(providerId));
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
    }
  }

  readProviderCredentials(providerIds: readonly string[]): Record<string, string> {
    const credentials: Record<string, string> = {};
    for (const providerId of providerIds) {
      const value = this.readProviderCredential(providerId);
      if (value !== undefined) credentials[providerId] = value;
    }
    return credentials;
  }

  private providerCredentialPath(providerId: string): string {
    const normalized = validateProviderId(providerId);
    // Preserve the original credential.bin location for Anthropic so existing
    // beta installs migrate without a prompt and old IPC remains compatible.
    return normalized === 'anthropic'
      ? this.credentialPath
      : join(dirname(this.credentialPath), 'provider-credentials', `${normalized}.bin`);
  }
}

function validateString(value: unknown, name: string, max: number): string {
  if (typeof value !== 'string' || value.length === 0 || value.length > max || value.includes('\0')) {
    throw new Error(`invalid ${name}`);
  }
  return value;
}

function validateProviderId(value: unknown): string {
  if (typeof value !== 'string' || !/^[a-z0-9][a-z0-9._-]{0,63}$/.test(value)) {
    throw new Error('invalid provider id');
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
