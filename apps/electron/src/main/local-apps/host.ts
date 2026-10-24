import { BrowserWindow, dialog, session, net, shell, clipboard, Notification, ipcMain, systemPreferences, type IpcMainInvokeEvent } from 'electron';
import { createServer, type Server } from 'node:http';
import { randomBytes, randomUUID } from 'node:crypto';
import { promises as fs } from 'node:fs';
import { dirname, join, relative, sep } from 'node:path';
import { pathToFileURL } from 'node:url';
import type { DesktopAppManifest, DesktopLocalApp, LocalAppCapability } from '../../shared/localApps.js';
import { CH_LOCAL_APPS, CH_LOCAL_APPS_CHANGED, LOCAL_APP_SCHEME } from '../../shared/localApps.js';
import { CH_LOCAL_APP_NATIVE } from '../../shared/localAppNative.js';
import { appId, atomicJson, containedFile } from './store.js';
import { CoreLocalAppStore } from './coreStore.js';
import type { LocalAppCoreTransport } from './coreClient.js';
import { LocalAppMarketClient, type MarketPublication } from './marketClient.js';
import { LocalAppServiceBroker } from './serviceBroker.js';
import { LocalAppRuntime, type RuntimeOptions } from './runtime.js';
import { relativePath } from './manifest.js';
import { isAppUrl, permissionCapabilities } from './permissions.js';

function appEntryUrl(id: string, entry: DesktopAppManifest['entry']): string {
  const path = entry.type === 'server' && entry.path ? entry.path.replace(/^\/+/, '').split('/').map(encodeURIComponent).join('/') : '';
  return new URL(`${LOCAL_APP_SCHEME}://${id}/${path}`).href;
}

export interface LocalAppHostOptions extends RuntimeOptions {
  configHome: string;
  core: LocalAppCoreTransport;
  accountToken?: () => Promise<string>;
  accountId?: () => string | undefined;
  resolveAuthoring?: (row: DesktopLocalApp) => Promise<DesktopLocalApp>;
  onRunEnded?: (appId: string, runId: string, reason: string) => Promise<void> | void;
  preload: string;
  mainWindow: () => BrowserWindow | undefined;
  edit: (row: DesktopLocalApp) => Promise<void>;
}
export class LocalAppHost {
  readonly store: CoreLocalAppStore;
  readonly services = new LocalAppServiceBroker();
  readonly market: LocalAppMarketClient;
  readonly runtime: LocalAppRuntime;
  private server?: Server;
  private readonly token = randomBytes(32).toString('hex');
  private readonly windows = new Map<string, BrowserWindow>();
  private readonly windowRuns = new WeakMap<BrowserWindow, string>();
  private readonly opening = new Map<string, { promise: Promise<void>; controller: AbortController }>();
  private readonly configured = new Set<string>();
  private readonly trustedWindows = new Map<number, string>();
  private changeTimer?: ReturnType<typeof setTimeout>;
  private shuttingDown = false;
  private unavailable?: Error;
  constructor(private readonly options: LocalAppHostOptions) {
    this.store = new CoreLocalAppStore(join(options.configHome, 'local-apps', 'v2'), options.core);
    this.market = new LocalAppMarketClient(this.store, options.accountToken ?? (() => Promise.reject(new Error('Sign in before publishing to the market'))), undefined, undefined, options.accountId);
    this.runtime = new LocalAppRuntime(this.store, () => this.changed(), { ...options, onRunEnded: async (appId, runId, reason) => { this.services.revoke(appId, runId); await options.onRunEnded?.(appId, runId, reason); } });
    options.core.onDisconnected?.(error => {
      this.unavailable = error;
      this.launchGrants.clear();
      for (const opening of this.opening.values()) opening.controller.abort();
      for (const window of this.windows.values()) window.destroy();
      void this.services.dispose();
      // No automatic bridge reconnect: native cleanup must finish before a new
      // exclusive owner can recover historical run metadata.
      void this.runtime.dispose().catch(() => undefined);
      this.changed();
    });
  }
  registerMainWindow(window: BrowserWindow, url: string): void {
    const id = window.webContents.id;
    this.trustedWindows.set(id, url);
    window.once('closed', () => this.trustedWindows.delete(id));
  }
  private assertMain(event: IpcMainInvokeEvent): void {
    const expected = this.trustedWindows.get(event.sender.id);
    if (!expected || event.senderFrame !== event.sender.mainFrame || !event.senderFrame || event.senderFrame.url.split('#')[0] !== expected.split('#')[0]) throw new Error('Unauthorized application library caller');
  }
  changed(): void {
    if (this.changeTimer || this.shuttingDown) return;
    this.changeTimer = setTimeout(() => {
      this.changeTimer = undefined;
      for (const window of BrowserWindow.getAllWindows()) if (this.trustedWindows.has(window.webContents.id)) window.webContents.send(CH_LOCAL_APPS_CHANGED);
    }, 80);
  }
  async initialize(): Promise<void> {
    await fs.mkdir(this.store.importsRoot, { recursive: true, mode: 0o700 });
    ipcMain.handle(CH_LOCAL_APPS, (event, input) => { this.assertMain(event); return this.request(input); });
    ipcMain.handle(CH_LOCAL_APP_NATIVE, (event, input) => this.native(event, input));
    // Model tools use this authenticated local-only endpoint. No account or
    // provider credentials are passed to it or to application children.
    this.server = createServer(async (request, response) => {
      if (request.method !== 'POST' || request.url !== '/local-app' || request.headers.authorization !== `Bearer ${this.token}` || request.headers.origin) {
        response.writeHead(403).end(); return;
      }
      const controller = new AbortController();
      response.once('close', () => { if (!response.writableFinished) controller.abort(); });
      try {
        const chunks: Buffer[] = []; let size = 0;
        for await (const chunk of request) { size += chunk.length; if (size > 64 * 1024 * 1024) throw new Error('Local App request is too large'); chunks.push(chunk); }
        const input = JSON.parse(Buffer.concat(chunks).toString('utf8'));
        if (!['publish', 'list', 'read', 'start', 'stop', 'status', 'logs'].includes(input.action)) throw new Error('Unsupported LocalApp tool action');
        controller.signal.throwIfAborted();
        const result = await this.request(input, controller.signal);
        response.writeHead(200, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' }).end(JSON.stringify(result ?? { ok: true }));
      } catch (error) { response.writeHead(400, { 'Content-Type': 'application/json' }).end(JSON.stringify({ error: error instanceof Error ? error.message : String(error) })); }
    });
    await new Promise<void>((resolve, reject) => { this.server!.once('error', reject); this.server!.listen(0, '127.0.0.1', resolve); });
    const address = this.server.address();
    if (!address || typeof address === 'string') throw new Error('Local App host could not listen');
    await atomicJson(join(this.options.configHome, 'desktop-local-app-host.json'), { port: address.port, token: this.token, pid: process.pid });
  }
  async list(): Promise<DesktopLocalApp[]> { return (await this.store.list()).map(row => this.runtime.status(row)); }
  async request(input: Record<string, unknown>, signal?: AbortSignal): Promise<unknown> {
    if (this.unavailable) throw this.unavailable;
    if (!input || typeof input !== 'object') throw new Error('Invalid Local App request');
    const action = String(input.action);
    if (action === 'marketList') return this.market.list(String(input.query ?? ''), input.owned === true);
    if (action === 'marketManage') { await shell.openExternal(new URL('/mine', this.market.base).href); return; }
    if (action === 'legacyEntries') return this.store.core.request({ op: 'legacyEntries' });
    if (action === 'importLegacy') {
      if (typeof input.legacyId !== 'string') throw new Error('Choose an archived application');
      const sourcePath = await this.authoringCopyPath('Imported app');
      const result = await this.store.core.request<{ record: { appId: string } }>({ op: 'importLegacy', legacyId: input.legacyId, sourcePath, authoring: { sourceDirectory: sourcePath } });
      this.changed(); await this.options.edit(await this.store.get(result.record.appId)); return;
    }
    if (action === 'marketInstall') {
      if (typeof input.remoteId !== 'string') throw new Error('Choose a market application');
      const existing = input.id ? await this.store.get(appId(input.id)) : undefined;
      if (existing && existing.revision !== input.expectedRevision) throw new Error('Application changed; refresh before updating');
      const row = await this.market.install(input.remoteId, existing); this.changed(); return row;
    }
    if (action === 'list') return { action, apps: await this.list() };
    if (action === 'publish') {
      const key = input.id === undefined ? '__publish_new__' : appId(input.id);
      const row = await this.runtime.exclusive(key, () => this.store.publish(input));
      this.changed();
      return { action, id: row.id, revision: row.revision, name: row.name, summary: row.summary, runtime: 'desktop', warnings: [], opensFrom: 'chat card or Apps library' };
    }
    const id = appId(input.id);
    if (action === 'versions') return this.store.core.request({ op: 'releases', appId: id });
    if (action === 'useVersion') {
      if (!Number.isSafeInteger(input.revision) || Number(input.revision) < 1 || !Number.isSafeInteger(input.expectedCurrentRevision) || Number(input.expectedCurrentRevision) < 1) throw new Error('Choose a version from the current release list');
      await this.cancelOpening(id);
      await this.open(id, false, true, signal, Number(input.revision), Number(input.expectedCurrentRevision));
      this.changed(); return;
    }
    if (action === 'openData') {
      await this.store.get(id);
      const error = await shell.openPath(this.store.dataPath(id));
      if (error) throw new Error(error);
      return;
    }
    if (action === 'restoreSource' || action === 'remix') {
      const row = await this.store.get(id), sourcePath = await this.authoringCopyPath(row.name);
      const result = await this.store.core.request<{ appId?: string; record?: { appId: string } }>({ op: action === 'remix' ? 'remix' : 'restoreAuthoring', appId: id, revision: row.revision, sourcePath, authoring: { sourceDirectory: sourcePath } });
      this.changed(); await this.options.edit(await this.store.get(result.record?.appId ?? result.appId ?? id)); return;
    }
    if (action === 'marketPublish') {
      const metadata = input.metadata as MarketPublication;
      if (!metadata || typeof metadata.name !== 'string' || typeof metadata.summary !== 'string' || !metadata.summary.trim()) throw new Error('Enter a name and summary before publishing');
      const prepared = await this.market.preparePublication(id, Number(input.expectedRevision));
      try {
        const options = { type: 'question' as const, message: `Publish ${metadata.name} source to the market?`, detail: `${prepared.files.length} explicitly included files:\n${prepared.files.map(file => file.path).join('\n')}\n\nThe source will be uploaded to your account. Services and new capabilities may require review. Local permissions and data are not included.`, buttons: ['Cancel', 'Upload source'], defaultId: 0, cancelId: 0 };
        const parent = this.options.mainWindow();
        const choice = parent ? await dialog.showMessageBox(parent, options) : await dialog.showMessageBox(options);
        if (choice.response !== 1) return;
        return await this.market.publish(prepared, metadata);
      } finally { await fs.rm(prepared.directory, { recursive: true, force: true }); }
    }
    if (action === 'read' || action === 'status') {
      const row = this.runtime.status(await this.store.get(id));
      const sourceSnapshotPath = this.store.revisionPath(id, row.revision);
      let sourcePath = row.sourcePath;
      if (sourcePath) { try { await fs.access(sourcePath); } catch { sourcePath = undefined; } }
      return { action, ...row, sourcePath: row.sourcePath, sourceMissing: Boolean(row.sourcePath && !sourcePath), sourceSnapshotPath, expectedRevision: row.revision };
    }
    if (action === 'logs') return { action, id, logs: this.runtime.logs(id) };
    if (action === 'open' || action === 'start') { await this.open(id, false, false, signal); return { action, id, phase: this.runtime.status(await this.store.get(id)).phase }; }
    if (action === 'stop') {
      const opening = this.cancelOpening(id);
      await this.runtime.stop(id);
      await opening;
      this.windows.get(id)?.close();
    } else if (action === 'restart' || action === 'repair') {
      const opening = this.cancelOpening(id);
      await opening;
      await this.open(id, action === 'repair', true);
    } else if (action === 'background') {
      if (typeof input.enabled !== 'boolean') throw new Error('Invalid background preference');
      await this.runtime.exclusive(id, async () => { const row = await this.store.get(id); row.background = input.enabled as boolean; await this.store.save(row); });
      if (!input.enabled && !this.windows.has(id) && !this.opening.has(id)) await this.runtime.stop(id);
    } else if (action === 'revoke') {
      const opening = this.cancelOpening(id);
      this.launchGrants.delete(id);
      await this.runtime.stop(id);
      await opening;
      this.windows.get(id)?.close();
      await this.runtime.exclusive(id, async () => { await this.store.revoke(id); await fs.rm(join(this.store.directory(id), 'file-grants.json'), { force: true }); });
    } else if (action === 'remove') {
      const parent = this.options.mainWindow();
      const options = { type: 'question' as const, message: `Remove ${(await this.store.get(id)).name}?`, detail: 'This removes the application from your library. Published versions and saved data are retained for recovery.', buttons: ['Cancel', 'Remove'], defaultId: 0, cancelId: 0 };
      const choice = parent ? await dialog.showMessageBox(parent, options) : await dialog.showMessageBox(options);
      if (choice.response !== 1) return;
      const opening = this.cancelOpening(id);
      this.windows.get(id)?.destroy();
      await this.runtime.remove(id);
      await opening;
      session.fromPartition(`persist:local-app-${id}`).flushStorageData();
    } else if (action === 'edit') { const row = await this.store.get(id); if (row.marketOrigin && !row.sourcePath) throw new Error('Create a permitted custom copy before developing a market application'); await this.options.edit(this.options.resolveAuthoring ? await this.options.resolveAuthoring(row) : row); }
    else throw new Error('Unknown Local App action');
    this.changed(); return { action, id };
  }
  private async authoringCopyPath(name: string): Promise<string> {
    const root = join(this.options.configHome, '.apps'); await fs.mkdir(root, { recursive: true, mode: 0o700 });
    return join(root, `${name.replace(/[^\p{L}\p{N} _-]/gu, '').slice(0, 50) || 'Application'}-${randomUUID().slice(0, 8)}`);
  }
  private cancelOpening(id: string): Promise<void> {
    const opening = this.opening.get(id);
    opening?.controller.abort();
    // Callers must also wait for open() to release its promise and cleanup, so
    // the next Open cannot join an operation that has already been canceled.
    return opening?.promise.then(() => undefined, () => undefined) ?? Promise.resolve();
  }
  private async authorize(id: string, signal: AbortSignal, revision?: number): Promise<void> {
    signal.throwIfAborted();
    const row = revision === undefined ? await this.store.get(id) : await this.store.getRevision(id, revision);
    const missing = (row.manifest.capabilities ?? []).filter(c => c !== 'agent' && !row.grants.includes(c));
    if (!missing.length) return;
    const labels: Record<LocalAppCapability, string> = { process: 'Run local programs and installation commands as your user', files: 'Choose files and folders to read or write', clipboard: 'Read and write the clipboard', notifications: 'Show notifications', camera: 'Use the camera', microphone: 'Use the microphone', photos: 'Select photos', share: 'Share files', agent: 'Request agent execution', background: 'Continue running in the background', network: 'Use network connections', location: 'Use location', calendar: 'Use calendar', contacts: 'Use contacts', haptics: 'Use haptics' };
    const options = { type: 'question' as const, signal, title: row.name, message: `Allow ${row.name} to use these capabilities?`,
      detail: missing.map(c => labels[c]).join('\n') + '\n\nYou can revoke access in Apps.', buttons: ['Cancel', 'Allow'], defaultId: 0, cancelId: 0 };
    const parent = this.options.mainWindow();
    signal.throwIfAborted();
    const result = parent ? await dialog.showMessageBox(parent, options) : await dialog.showMessageBox(options);
    signal.throwIfAborted();
    if (result.response !== 1) throw new Error('Application permission was not granted');
    await this.runtime.exclusive(id, async () => {
      signal.throwIfAborted();
      const current = await this.store.getRevision(id, row.revision);
      if (current.releaseDigest !== row.releaseDigest) throw new Error('Application changed while permission was being reviewed');
      current.grants = [...new Set([...current.grants, ...missing])];
      await this.store.save(current);
    });
  }
  async open(id: string, repair = false, restart = false, signal?: AbortSignal, revision?: number, expectedCurrentRevision?: number): Promise<void> {
    if (this.shuttingDown) throw new Error('Application host is stopping');
    signal?.throwIfAborted();
    const pending = this.opening.get(id); if (pending) return pending.promise;
    const controller = new AbortController();
    const cancel = () => controller.abort();
    signal?.addEventListener('abort', cancel, { once: true });
    const previousInvocation = this.runtime.invocationId(id);
    const operation = { controller, promise: this.openOnce(id, controller.signal, repair, restart, revision, expectedCurrentRevision) };
    this.opening.set(id, operation);
    operation.promise = operation.promise.catch(async error => {
      if (!this.shuttingDown && ['running', 'starting', 'installing', 'building'].includes(this.runtime.phase(id))) {
        const ownsNewRun = this.runtime.invocationId(id) !== previousInvocation;
        const orphanedForeground = !this.windows.has(id) && !(await this.store.get(id)).background;
        if (ownsNewRun || orphanedForeground) await this.runtime.stop(id);
      }
      throw error;
    }).finally(() => {
      signal?.removeEventListener('abort', cancel);
      if (this.opening.get(id) === operation) this.opening.delete(id);
    });
    await operation.promise;
  }
  private async openOnce(id: string, signal: AbortSignal, repair: boolean, restart: boolean, revision?: number, expectedCurrentRevision?: number): Promise<void> {
    const row = revision === undefined ? await this.store.get(id) : await this.store.getRevision(id, revision);
    const existing = this.windows.get(id);
    if (existing && !existing.isDestroyed() && !restart && this.runtime.phase(id) === 'running' && this.runtime.revision(id) === row.revision) { existing.show(); existing.focus(); return; }
    await this.store.assertTarget(row);
    await this.authorize(id, signal, revision);
    signal.throwIfAborted();
    await this.runtime.start(id, repair, signal, restart || this.runtime.revision(id) !== row.revision, revision, expectedCurrentRevision);
    const invocation = this.runtime.invocationId(id);
    // A replacement gets a fresh document host. Old pages cannot acquire the new run's APIs.
    if (existing && !existing.isDestroyed()) existing.destroy();
    const assertRunning = () => {
      signal.throwIfAborted();
      if (this.runtime.invocationId(id) !== invocation) throw new Error('Application changed while opening');
      const current = this.runtime.status(row);
      if (current.phase !== 'running') throw new Error(current.error ?? 'Application stopped while opening');
    };
    assertRunning();
    this.launchGrants.set(id, (await this.store.getRevision(id, this.runtime.revision(id))).grants.filter(cap => this.runtime.manifest(id)?.capabilities?.includes(cap)));
    await this.configureSession(id);
    await this.clearCodeCache(id);
    assertRunning();
    const entryUrl = appEntryUrl(id, this.runtime.manifest(id)!.entry);
    const window = new BrowserWindow({ width: 1080, height: 760, minWidth: 360, minHeight: 320, title: row.name, show: false,
      webPreferences: { partition: `persist:local-app-${id}`, preload: this.options.preload, contextIsolation: true, nodeIntegration: false, sandbox: true, webSecurity: true, navigateOnDragDrop: false } });
    this.windows.set(id, window);
    this.windowRuns.set(window, invocation!);
    const origin = `${LOCAL_APP_SCHEME}://${id}`;
    window.webContents.on('will-navigate', (event, url) => { if (!url.startsWith(origin + '/')) { event.preventDefault(); void this.external(url); } });
    window.webContents.setWindowOpenHandler(({ url }) => { void this.external(url); return { action: 'deny' }; });
    window.webContents.on('will-attach-webview', event => event.preventDefault());
    window.on('closed', () => {
      if (this.windows.get(id) === window) this.windows.delete(id);
      if (!this.shuttingDown) void this.store.get(id).then(row => !row.background && !this.windows.has(id) && !this.opening.has(id) ? this.runtime.stop(id) : undefined).catch(() => undefined);
    });
    try { await window.loadURL(entryUrl); assertRunning(); window.show(); }
    catch (error) { window.destroy(); await this.runtime.stop(id); throw error; }
  }
  private async external(raw: string): Promise<void> { try { const url = new URL(raw); if (['https:', 'http:', 'mailto:'].includes(url.protocol)) await shell.openExternal(url.toString()); } catch { /* invalid URL */ } }
  private async clearCodeCache(id: string): Promise<void> {
    const partition = session.fromPartition(`persist:local-app-${id}`);
    await partition.clearCache();
    // CacheStorage can contain user-created offline documents. Refresh code
    // without deleting origin data owned by the application.
    await partition.clearStorageData({ storages: ['serviceworkers'] });
  }
  private async configureSession(id: string): Promise<void> {
    if (this.configured.has(id)) return;
    const partition = session.fromPartition(`persist:local-app-${id}`);
    const permission = permissionCapabilities;
    partition.setPermissionCheckHandler((contents, kind, origin, details) => {
      const window = this.windows.get(id);
      if (!contents || !window || contents !== window.webContents || this.windowRuns.get(window) !== this.runtime.invocationId(id) || !details.isMainFrame || !isAppUrl(id, origin) || this.runtime.phase(id) !== 'running') return false;
      // The synchronous check only admits capabilities approved for this launch.
      return permission(kind, details).length > 0 && permission(kind, details).every(c => this.launchGrants.get(id)?.includes(c));
    });
    partition.setPermissionRequestHandler(async (contents, kind, callback, details) => {
      try {
        const window = this.windows.get(id), invocation = window ? this.windowRuns.get(window) : undefined;
        const current = () => contents === this.windows.get(id)?.webContents && !contents.isDestroyed() && details.isMainFrame && isAppUrl(id, details.requestingUrl) && isAppUrl(id, contents.getURL()) && this.runtime.phase(id) === 'running' && this.runtime.invocationId(id) === invocation;
        if (!current()) { callback(false); return; }
        const row = await this.store.getRevision(id, this.runtime.revision(id)), required = permission(kind, details);
        let allowed = required.length > 0 && required.every(c => row.grants.includes(c) && this.launchGrants.get(id)?.includes(c));
        if (allowed && process.platform === 'darwin') for (const c of required) if (c === 'microphone' || c === 'camera') allowed = await systemPreferences.askForMediaAccess(c) && allowed;
        callback(allowed && current() && required.every(c => this.launchGrants.get(id)?.includes(c)));
      } catch { callback(false); }
    });
    partition.protocol.handle(LOCAL_APP_SCHEME, async request => {
      try {
        const url = new URL(request.url);
        if (url.hostname !== id || url.username || url.password) return new Response('Not found', { status: 404 });
        const row = await this.store.get(id);
        const revision = this.runtime.revision(id);
        const workspace = this.runtime.workspace(id);
        const window = this.windows.get(id);
        if (!window || window.isDestroyed() || this.windowRuns.get(window) !== this.runtime.invocationId(id) || !workspace || !revision || this.runtime.phase(id) !== 'running') return new Response('Application run is unavailable', { status: 410 });
        const manifest = this.runtime.manifest(id)!;
        if (manifest.entry.type === 'server' && url.pathname === '/') {
          const landing = new URL(appEntryUrl(id, manifest.entry));
          if (landing.pathname !== '/') {
            landing.search = url.search;
            return new Response(null, { status: 307, headers: { Location: landing.href, 'Cache-Control': 'no-store' } });
          }
        }
        let response: Response;
        const serviceRoute = /^\/__services\/([\w-]+)(\/.*)?$/.exec(url.pathname);
        if (manifest.entry.type === 'server' || serviceRoute) {
          const service = this.runtime.service(id, serviceRoute?.[1] ?? (manifest.entry.type === 'server' ? manifest.entry.process : ''));
          if (!service) return new Response('Backend is unavailable', { status: 503 });
          const path = serviceRoute ? serviceRoute[2] ?? '/' : url.pathname;
          const runId = this.runtime.invocationId(id)!;
          response = await this.services.fetch(id, runId, serviceRoute?.[1] ?? (manifest.entry.type === 'server' ? manifest.entry.process : ''), service.port, path + url.search, request);
        } else {
          const assetRoot = dirname(manifest.entry.path);
          const path = url.pathname === '/' ? manifest.entry.path : join(assetRoot, decodeURIComponent(url.pathname).replace(/^\/+/, '')).split(sep).join('/');
          let file: string;
          try { file = await containedFile(workspace, path); }
          catch { if (request.headers.get('accept')?.includes('text/html')) file = await containedFile(workspace, manifest.entry.path); else throw new Error('Missing application asset'); }
          response = await net.fetch(pathToFileURL(file).toString(), { headers: request.headers });
        }
        const headers = new Headers(response.headers);
        headers.set('X-Content-Type-Options', 'nosniff');
        // Desktop supports modules, workers, network, and the app's own servers.
        headers.set('Content-Security-Policy', "default-src 'self' data: blob: https: http: ws: wss:; script-src 'self' 'unsafe-inline' 'unsafe-eval' blob: https: http:; style-src 'self' 'unsafe-inline' https: http:; worker-src 'self' blob:; object-src 'none'; frame-ancestors 'none'");
        headers.set('Cache-Control', 'no-store');
        return new Response(response.body, { status: response.status, statusText: response.statusText, headers });
      } catch { return new Response('Application resource unavailable', { status: 404 }); }
    });
    this.configured.add(id);
  }
  private readonly launchGrants = new Map<string, LocalAppCapability[]>();
  private async native(event: IpcMainInvokeEvent, input: { method: string; args?: Record<string, unknown> }): Promise<unknown> {
    const pair = [...this.windows].find(([, window]) => window.webContents === event.sender);
    if (!pair || event.senderFrame !== event.sender.mainFrame || !event.senderFrame?.url.startsWith(`${LOCAL_APP_SCHEME}://${pair[0]}/`)) throw new Error('Unauthorized application caller');
    const id = pair[0], invocation = this.windowRuns.get(pair[1]), row = await this.store.getRevision(id, this.runtime.revision(id)), args = input.args ?? {};
    const current = () => {
      if (!invocation || this.windows.get(id) !== pair[1] || pair[1].isDestroyed() || this.runtime.invocationId(id) !== invocation || this.runtime.phase(id) !== 'running') throw new Error('Application operation is no longer active');
    };
    current();
    const need = (cap: LocalAppCapability) => { current(); if (!row.grants.includes(cap) || !this.launchGrants.get(id)?.includes(cap)) throw new Error(`Application capability not granted: ${cap}`); };
    if (input.method === 'info') return { id, name: row.name, runId: this.runtime.invocationId(id), revision: this.runtime.revision(id), platform: process.platform, dataDirectory: this.store.dataPath(id) };
    if (input.method === 'serviceSocketUrl') {
      const service = this.runtime.service(id, String(args.name));
      if (!service) throw new Error('Backend is unavailable');
      const path = String(args.path ?? '/');
      if (!path.startsWith('/') || path.startsWith('//') || path.includes('\\')) throw new Error('Invalid service path');
      return this.services.socketUrl(id, invocation!, String(args.name), service.port, path, `${LOCAL_APP_SCHEME}://${id}`);
    }
    if (input.method === 'clipboard.read') { need('clipboard'); return clipboard.readText(); }
    if (input.method === 'clipboard.write') { need('clipboard'); if (typeof args.text !== 'string') throw new Error('Expected text'); clipboard.writeText(args.text); return; }
    if (input.method === 'notify') { need('notifications'); new Notification({ title: row.name, body: String(args.body ?? '') }).show(); return; }
    if (input.method === 'process.exec') { need('process'); if (typeof args.command !== 'string') throw new Error('Expected command'); return this.runtime.execute(id, args.command); }
    if (input.method === 'files.pickDirectory') {
      need('files'); const result = await dialog.showOpenDialog(pair[1], { properties: ['openDirectory', 'createDirectory'] });
      if (result.canceled || !result.filePaths[0]) return null;
      const token = randomUUID();
      await this.runtime.exclusive(id, async () => {
        need('files');
        const grants = await this.fileGrants(id); grants[token] = await fs.realpath(result.filePaths[0]!);
        need('files'); await atomicJson(join(this.store.directory(id), 'file-grants.json'), grants);
      });
      return { token, name: result.filePaths[0].split(sep).at(-1) };
    }
    if (input.method.startsWith('files.')) {
      need('files');
      const root = args.root === 'data' ? this.store.dataPath(id) : (await this.fileGrants(id))[String(args.root)];
      if (!root) throw new Error('Select a folder first');
      const rel = relativePath(args.path ?? '.');
      if (input.method === 'files.read') return fs.readFile(await containedFile(root, rel), 'utf8');
      if (input.method === 'files.list') return (await fs.readdir(await containedFile(root, rel), { withFileTypes: true })).map(e => ({ name: e.name, directory: e.isDirectory() }));
      if (input.method === 'files.write') {
        const leaf = rel.split('/').at(-1);
        if (!leaf || leaf === '.') throw new Error('Writing requires a file path');
        const parent = await containedFile(root, dirname(rel).split(sep).join('/'));
        const target = join(parent, leaf);
        try { if (!(await fs.lstat(target)).isFile()) throw new Error('Cannot write through a symlink or replace a directory'); } catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
        if (typeof args.text !== 'string') throw new Error('Expected text');
        const temp = `${target}.${randomUUID()}.tmp`;
        try { need('files'); await fs.writeFile(temp, args.text, { flag: 'wx', mode: 0o600 }); need('files'); await fs.rename(temp, target); }
        finally { await fs.rm(temp, { force: true }); }
        return;
      }
    }
    throw new Error('Unknown application API');
  }
  private async fileGrants(id: string): Promise<Record<string, string>> { try { return JSON.parse(await fs.readFile(join(this.store.directory(id), 'file-grants.json'), 'utf8')); } catch { return {}; } }
  async hasBackgroundApps(): Promise<boolean> {
    return (await this.list()).some(row => row.background && ['running', 'installing', 'building', 'starting'].includes(row.phase));
  }
  async dispose(): Promise<void> {
    this.shuttingDown = true;
    for (const operation of this.opening.values()) operation.controller.abort();
    if (this.changeTimer) clearTimeout(this.changeTimer);
    ipcMain.removeHandler(CH_LOCAL_APPS); ipcMain.removeHandler(CH_LOCAL_APP_NATIVE);
    this.server?.closeAllConnections(); await new Promise<void>(resolve => this.server ? this.server.close(() => resolve()) : resolve());
    await this.runtime.dispose();
    await this.services.dispose();
    await this.options.core.dispose();
    await Promise.allSettled([...this.opening.values()].map(operation => operation.promise));
    for (const window of this.windows.values()) window.destroy();
    for (const id of this.configured) {
      const partition = session.fromPartition(`persist:local-app-${id}`);
      partition.protocol.unhandle(LOCAL_APP_SCHEME);
      partition.setPermissionCheckHandler(null);
      partition.setPermissionRequestHandler(null);
      partition.flushStorageData();
    }
    this.configured.clear(); this.launchGrants.clear();
    const path = join(this.options.configHome, 'desktop-local-app-host.json');
    try { if (JSON.parse(await fs.readFile(path, 'utf8')).token === this.token) await fs.rm(path); } catch { /* Already removed. */ }
  }
}
