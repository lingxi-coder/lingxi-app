import { execFile } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { lstatSync, mkdirSync, readFileSync, renameSync, writeFileSync, unlinkSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { promisify } from 'node:util';
import type { ComputerDevicePolicy, ComputerPermission, ComputerSettingsCommand, ComputerSettingsSnapshot } from '../shared/computerSettings.js';
import { validBundleId } from '../shared/applicationIcon.js';

const run = promisify(execFile);
const modes = new Set(['ask', 'allow', 'deny']);
const browserKeys = new Set(['history', 'siteTools', 'fullCdp', 'defaultBrowsing', 'defaultDownloads']);
const ownApplication = (id: string) => id === 'com.lingxi.code' || id.startsWith('com.lingxi.code.');
const mode = (value: unknown): ComputerPermission => {
  if (typeof value !== 'string' || !modes.has(value)) throw new Error('Invalid permission');
  return value as ComputerPermission;
};
export function siteOrigin(value: unknown): string {
  if (typeof value !== 'string' || value.length > 2048) throw new Error('Enter an HTTP or HTTPS origin');
  const url = new URL(value);
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash
      || url.pathname !== '/' || value.includes('*')) throw new Error('Use an exact HTTP or HTTPS origin without a path or wildcard');
  return url.origin;
}
function defaults(): ComputerDevicePolicy {
  return { revision: randomUUID(), enabled: true, apps: [], browser: {
    history: 'ask', siteTools: false, fullCdp: false, defaultBrowsing: 'ask', defaultDownloads: 'ask', sites: [],
  } };
}
function decode(value: unknown): ComputerDevicePolicy {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid computer settings');
  const raw = value as Record<string, unknown>;
  if (typeof raw.revision !== 'string' || !raw.revision || typeof raw.enabled !== 'boolean'
      || !Array.isArray(raw.apps) || raw.apps.length > 512 || !raw.browser || typeof raw.browser !== 'object') throw new Error('Invalid computer settings');
  const browser = raw.browser as Record<string, unknown>;
  if (typeof browser.siteTools !== 'boolean' || typeof browser.fullCdp !== 'boolean' || !Array.isArray(browser.sites) || browser.sites.length > 256) throw new Error('Invalid browser settings');
  const apps = raw.apps.map((value: unknown) => {
    if (!value || typeof value !== 'object') throw new Error('Invalid application');
    const app = value as Record<string, unknown>;
    if (!validBundleId(app.bundleId) || typeof app.displayName !== 'string' || !app.displayName || app.displayName.length > 512 || ownApplication(app.bundleId)) throw new Error('Invalid application');
    return { bundleId: app.bundleId, displayName: app.displayName, permission: mode(app.permission) };
  });
  const sites = browser.sites.map((value: unknown) => {
    if (!value || typeof value !== 'object') throw new Error('Invalid site');
    const site = value as Record<string, unknown>;
    return { origin: siteOrigin(site.origin), browsing: mode(site.browsing), downloads: mode(site.downloads) };
  });
  if (new Set(apps.map(app => app.bundleId)).size !== apps.length || new Set(sites.map(site => site.origin)).size !== sites.length) throw new Error('Duplicate permission entry');
  return { revision: raw.revision, enabled: raw.enabled, apps, browser: {
    history: mode(browser.history), siteTools: browser.siteTools, fullCdp: browser.fullCdp,
    defaultBrowsing: mode(browser.defaultBrowsing), defaultDownloads: mode(browser.defaultDownloads), sites,
  } };
}

interface BrowserService {
  status(): ComputerSettingsSnapshot['browserStatus'];
  retry(): Promise<ComputerSettingsSnapshot['browserStatus']>;
  install(assertCurrent?: () => void): Promise<ComputerSettingsSnapshot['browserStatus']>;
  remove(): Promise<ComputerSettingsSnapshot['browserStatus']>;
  policyChanged(): void;
}

export class ComputerSettingsStore {
  readonly policyPath: string;
  private readonly authorityPath: string;
  get executionPolicyPath(): string { return this.error ? `${this.authorityPath}.disabled-${this.current.revision}` : this.authorityPath; }
  private current: ComputerDevicePolicy;
  private error?: string;
  private browser?: BrowserService;
  private busy = false;
  private sourceDigest = '';
  private browserError?: string;
  constructor(userData: string, private readonly options: {
    onChanged?: () => void;
    resolveApplication?: (bundleId: string) => Promise<{ bundleId: string; displayName: string }>;
    showExtension?: (path: string) => Promise<void>;
  } = {}) {
    this.policyPath = join(userData, 'computer-use', 'policy.json');
    this.authorityPath = join(userData, 'computer-use', 'execution-policy.json');
    this.current = defaults();
    try {
      try { this.readSource(); }
      catch (error) {
        if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
        this.writeAtomic(this.policyPath, this.current);
      }
      const source = this.readSource();
      this.current = { ...source.policy, revision: randomUUID() };
      this.sourceDigest = source.digest;
      this.writeExecution();
    } catch (error) {
      this.failClosed(false);
    }
  }
  attachBrowser(browser: BrowserService): void { this.browser = browser; }
  browserUnavailable(message: string): void { this.browser = undefined; this.browserError = message; }
  private readSource(): { policy: ComputerDevicePolicy; digest: string } {
    const metadata = lstatSync(this.policyPath);
    if (!metadata.isFile() || metadata.size > 262144
        || process.platform !== 'win32' && ((metadata.mode & 0o077) !== 0 || metadata.uid !== process.geteuid?.())) throw new Error('Computer settings must be an owner-only regular file');
    const bytes = readFileSync(this.policyPath);
    if (bytes.length > 262144) throw new Error('Computer settings exceed the size limit');
    const contents = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
    return { policy: decode(JSON.parse(contents)), digest: createHash('sha256').update(bytes).digest('hex') };
  }
  private writeExecution(): void {
    this.writeAtomic(this.authorityPath, { ...this.current, ownerPid: process.pid, source: { path: this.policyPath, sha256: this.sourceDigest } });
  }
  private changed(): void { this.browser?.policyChanged(); this.options.onChanged?.(); }
  private failClosed(notify = true): void {
    this.error = 'Computer settings could not be read or saved. Restart Desktop after restoring the settings file.';
    this.current = { ...defaults(), enabled: false };
    // Even a restored source must not revive the previously issued authority.
    try { this.writeExecution(); } catch { /* Source digest verification rejects a stale execution file. */ }
    if (notify) this.changed();
  }
  private refreshSource(): void {
    if (this.error) return;
    try {
      const source = this.readSource();
      if (source.digest === this.sourceDigest) return;
      this.sourceDigest = source.digest;
      this.current = { ...source.policy, revision: randomUUID() };
      this.writeExecution();
    } catch { this.failClosed(); return; }
    this.changed();
  }
  policy(): ComputerDevicePolicy { this.refreshSource(); return structuredClone(this.current); }
  snapshot(): ComputerSettingsSnapshot {
    return { ...this.policy(), browserStatus: this.browser?.status() ?? {
      connected: false, installed: false, extensionDirectory: '', error: this.browserError ?? this.error ?? 'Browser integration is starting',
    }, capabilities: { computer: process.platform === 'darwin' && !this.error, browser: !!this.browser && !this.error, lockedUse: false, excel: false }, ...(this.error ? { error: this.error } : {}) };
  }
  private writeAtomic(path: string, value: unknown): void {
    const contents = `${JSON.stringify(value)}\n`;
    if (Buffer.byteLength(contents) > 262144) throw new Error('Computer settings exceed the size limit');
    const directory = dirname(path);
    mkdirSync(directory, { recursive: true, mode: 0o700 });
    const temporary = `${path}.${randomUUID()}.tmp`;
    try {
      writeFileSync(temporary, contents, { mode: 0o600, flag: 'wx' });
      renameSync(temporary, path);
    } finally { try { unlinkSync(temporary); } catch { /* renamed or not created */ } }
  }
  private persist(value: ComputerDevicePolicy, expectedRevision: string): void {
    this.refreshSource();
    if (this.error) throw new Error(this.error);
    if (this.current.revision !== expectedRevision) throw new Error('Computer settings changed. Reload and try again.');
    // Commit configuration first: any old authority now fails its source digest check.
    this.writeAtomic(this.policyPath, value);
    this.sourceDigest = createHash('sha256').update(`${JSON.stringify(value)}\n`).digest('hex');
    this.current = value;
    try { this.writeExecution(); }
    catch { this.failClosed(); throw new Error(this.error); }
  }
  async execute(input: unknown, assertCurrent: () => void = () => {}): Promise<ComputerSettingsSnapshot> {
    if (!input || typeof input !== 'object' || Array.isArray(input)) throw new Error('Invalid computer settings request');
    const command = input as ComputerSettingsCommand;
    if (command.action === 'get') return this.snapshot();
    this.refreshSource();
    if (this.error) throw new Error(this.error);
    if (this.busy) throw new Error('Computer settings are being updated. Try again.');
    this.busy = true;
    try {
      if (['retryBrowser', 'installBrowser', 'removeBrowser', 'showExtension'].includes(command.action)) {
        if (!this.browser) throw new Error('Browser integration is unavailable');
        assertCurrent();
        if (command.action === 'retryBrowser') await this.browser.retry();
        if (command.action === 'installBrowser') await this.browser.install(assertCurrent);
        if (command.action === 'removeBrowser') {
          // Also revoke approval cards whose HTTP nonce response has completed
          // and is now waiting in the trusted conversation permission UI.
          const next = { ...this.policy(), revision: randomUUID() };
          this.persist(next, this.current.revision);
          this.browser.policyChanged();
          this.options.onChanged?.();
          await this.browser.remove();
        }
        if (command.action === 'showExtension') {
          const path = this.browser.status().extensionDirectory;
          if (!path || !this.options.showExtension) throw new Error('Extension folder is unavailable');
          await this.options.showExtension(path);
        }
        this.options.onChanged?.();
        return this.snapshot();
      }
      if (!('revision' in command) || command.revision !== this.current.revision) throw new Error('Computer settings changed. Reload and try again.');
      const next = this.policy();
      const expectedRevision = next.revision;
      switch (command.action) {
        case 'setEnabled':
          if (typeof command.enabled !== 'boolean') throw new Error('Invalid enabled value');
          next.enabled = command.enabled;
          break;
        case 'setApplication': {
          if (!validBundleId(command.bundleId) || ownApplication(command.bundleId)) throw new Error('Select an installed application other than LingXi');
          const permission = mode(command.permission);
          const existing = next.apps.find(app => app.bundleId === command.bundleId);
          const app = existing && permission !== 'allow' ? existing : await (this.options.resolveApplication ?? resolveApplication)(command.bundleId);
          if (app.bundleId !== command.bundleId) throw new Error('Application identity changed');
          next.apps = [...next.apps.filter(entry => entry.bundleId !== app.bundleId), { ...app, permission }];
          break;
        }
        case 'removeApplication': {
          if (!validBundleId(command.bundleId)) throw new Error('Invalid application');
          // Removing permanent access also denies any active session grants.
          next.apps = next.apps.flatMap(app => app.bundleId !== command.bundleId ? [app] : app.permission === 'allow' ? [{ ...app, permission: 'deny' as const }] : []);
          break;
        }
        case 'setBrowser': {
          if (!browserKeys.has(command.key)) throw new Error('Invalid browser setting');
          if (command.key === 'siteTools' || command.key === 'fullCdp') {
            if (typeof command.value !== 'boolean') throw new Error('Invalid browser setting value');
            next.browser[command.key] = command.value;
          } else next.browser[command.key] = mode(command.value);
          break;
        }
        case 'setSite': {
          const origin = siteOrigin(command.origin);
          next.browser.sites = [...next.browser.sites.filter(site => site.origin !== origin), { origin, browsing: mode(command.browsing), downloads: mode(command.downloads) }];
          break;
        }
        case 'removeSite': next.browser.sites = next.browser.sites.filter(site => site.origin !== siteOrigin(command.origin)); break;
        default: throw new Error('Unknown computer settings request');
      }
      next.revision = randomUUID();
      const validated = decode(next);
      assertCurrent();
      this.persist(validated, expectedRevision);
      this.browser?.policyChanged();
      this.options.onChanged?.();
      return this.snapshot();
    } finally { this.busy = false; }
  }
}

async function resolveApplication(bundleId: string): Promise<{ bundleId: string; displayName: string }> {
  if (process.platform !== 'darwin') throw new Error('Application management is available on macOS');
  const source = `ObjC.import('AppKit'); function run(argv) {
    const url = $.NSWorkspace.sharedWorkspace.URLForApplicationWithBundleIdentifier(argv[0]);
    if (!url || url.isNil()) return '';
    const bundle = $.NSBundle.bundleWithURL(url);
    const name = bundle.objectForInfoDictionaryKey('CFBundleDisplayName') || bundle.objectForInfoDictionaryKey('CFBundleName');
    return JSON.stringify({bundleId: ObjC.unwrap(bundle.bundleIdentifier), displayName: name && !name.isNil() ? ObjC.unwrap(name) : argv[0]});
  }`;
  const { stdout } = await run('/usr/bin/osascript', ['-l', 'JavaScript', '-e', source, bundleId], { timeout: 3000, maxBuffer: 65536 });
  if (!stdout.trim()) throw new Error('Application is not installed');
  const app: unknown = JSON.parse(stdout);
  if (!app || typeof app !== 'object' || !('bundleId' in app) || app.bundleId !== bundleId || !('displayName' in app) || typeof app.displayName !== 'string') throw new Error('Application identity could not be verified');
  return { bundleId, displayName: app.displayName };
}
