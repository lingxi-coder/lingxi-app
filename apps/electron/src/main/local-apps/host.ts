import { BrowserWindow, dialog, session, net, shell, clipboard, Notification, ipcMain, systemPreferences, type IpcMainInvokeEvent } from 'electron';
import { createServer, type Server } from 'node:http';
import { randomBytes, randomUUID } from 'node:crypto';
import { promises as fs } from 'node:fs';
import { dirname, join, relative, sep } from 'node:path';
import { pathToFileURL } from 'node:url';
import type { DesktopAppManifest, DesktopLocalApp, LocalAppCapability } from '../../shared/localApps.js';
import { CH_LOCAL_APPS, CH_LOCAL_APPS_CHANGED, LOCAL_APP_SCHEME } from '../../shared/localApps.js';
import { CH_LOCAL_APP_NATIVE } from '../../shared/localAppNative.js';
import { LocalAppStore, appId, atomicJson, containedFile } from './store.js';
import { LocalAppRuntime, type RuntimeOptions } from './runtime.js';
import { relativePath } from './manifest.js';
import { isAppUrl, permissionCapabilities } from './permissions.js';

function appEntryUrl(id: string, entry: DesktopAppManifest['entry']): string {
  const path = entry.type === 'server' && entry.path ? entry.path.split('/').map(encodeURIComponent).join('/') : '';
  return new URL(`${LOCAL_APP_SCHEME}://${id}/${path}`).href;
}

export interface LocalAppHostOptions extends RuntimeOptions {
  configHome: string;
  preload: string;
  mainWindow: () => BrowserWindow | undefined;
  edit: (row: DesktopLocalApp) => Promise<void>;
}
export class LocalAppHost {
  readonly store: LocalAppStore;
  readonly runtime: LocalAppRuntime;
  private server?: Server;
  private readonly token = randomBytes(32).toString('hex');
  private readonly windows = new Map<string, BrowserWindow>();
  private readonly opening = new Map<string, { promise: Promise<void>; controller: AbortController }>();
  private readonly configured = new Set<string>();
  private readonly trustedWindows = new Map<number, string>();
  private changeTimer?: ReturnType<typeof setTimeout>;
  private shuttingDown = false;
  constructor(private readonly options: LocalAppHostOptions) {
    this.store = new LocalAppStore(join(options.configHome, 'local-apps', 'desktop'));
    this.runtime = new LocalAppRuntime(this.store, () => this.changed(), options);
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
    await fs.mkdir(join(this.store.root, 'imports'), { recursive: true, mode: 0o700 });
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
    if (!input || typeof input !== 'object') throw new Error('Invalid Local App request');
    const action = String(input.action);
    if (action === 'list') return { action, apps: await this.list() };
    if (action === 'publish') {
      const key = input.id === undefined ? '__publish_new__' : appId(input.id);
      const row = await this.runtime.exclusive(key, () => this.store.publish(input));
      this.changed();
      return { action, id: row.id, revision: row.revision, name: row.name, summary: row.summary, runtime: 'desktop', warnings: [], opensFrom: 'chat card or Apps library' };
    }
    const id = appId(input.id);
    if (action === 'read' || action === 'status') {
      const row = this.runtime.status(await this.store.get(id));
      const sourceSnapshotPath = this.store.revisionPath(id, row.revision);
      let sourcePath = row.sourcePath;
      if (sourcePath) { try { await fs.access(sourcePath); } catch { sourcePath = undefined; } }
      return { action, ...row, sourcePath: sourcePath ?? sourceSnapshotPath, sourceSnapshotPath };
    }
    if (action === 'logs') return { action, id, logs: this.runtime.logs(id) };
    if (action === 'open' || action === 'start') { await this.open(id, false, false, signal); return { action, id, phase: this.runtime.status(await this.store.get(id)).phase }; }
    if (action === 'stop') { this.cancelOpening(id); await this.runtime.stop(id); this.windows.get(id)?.close(); }
    else if (action === 'restart' || action === 'repair') {
      this.cancelOpening(id);
      await this.runtime.stop(id);
      await this.opening.get(id)?.promise.catch(() => undefined);
      await this.open(id, action === 'repair', true);
    } else if (action === 'background') {
      if (typeof input.enabled !== 'boolean') throw new Error('Invalid background preference');
      await this.runtime.exclusive(id, async () => { const row = await this.store.get(id); row.background = input.enabled as boolean; await this.store.save(row); });
      if (!input.enabled && !this.windows.has(id)) await this.runtime.stop(id);
    } else if (action === 'revoke') {
      this.cancelOpening(id);
      this.launchGrants.delete(id);
      await this.runtime.stop(id); this.windows.get(id)?.close();
      await this.runtime.exclusive(id, async () => { const row = await this.store.get(id); row.grants = []; await this.store.save(row); await fs.rm(join(this.store.directory(id), 'file-grants.json'), { force: true }); });
    } else if (action === 'remove') {
      const parent = this.options.mainWindow();
      const options = { type: 'question' as const, message: `Delete ${(await this.store.get(id)).name}?`, detail: 'This removes the application and its saved data.', buttons: ['Cancel', 'Delete'], defaultId: 0, cancelId: 0 };
      const choice = parent ? await dialog.showMessageBox(parent, options) : await dialog.showMessageBox(options);
      if (choice.response !== 1) return;
      this.cancelOpening(id);
      this.windows.get(id)?.destroy();
      await this.runtime.remove(id);
      await session.fromPartition(`persist:local-app-${id}`).clearStorageData();
    } else if (action === 'edit') await this.options.edit(await this.store.get(id));
    else throw new Error('Unknown Local App action');
    this.changed(); return { action, id };
  }
  private cancelOpening(id: string): void { this.opening.get(id)?.controller.abort(); }
  private async authorize(id: string, signal: AbortSignal): Promise<void> {
    signal.throwIfAborted();
    const row = await this.store.get(id);
    const missing = (row.manifest.capabilities ?? []).filter(c => !row.grants.includes(c));
    if (!missing.length) return;
    const labels: Record<LocalAppCapability, string> = { process: 'Run local programs and installation commands as your user', files: 'Choose files and folders to read or write', clipboard: 'Read and write the clipboard', notifications: 'Show notifications', camera: 'Use the camera', microphone: 'Use the microphone' };
    const options = { type: 'question' as const, signal, title: row.name, message: `Allow ${row.name} to use these capabilities?`,
      detail: missing.map(c => labels[c]).join('\n') + '\n\nYou can revoke access in Apps.', buttons: ['Cancel', 'Allow'], defaultId: 0, cancelId: 0 };
    const parent = this.options.mainWindow();
    signal.throwIfAborted();
    const result = parent ? await dialog.showMessageBox(parent, options) : await dialog.showMessageBox(options);
    signal.throwIfAborted();
    if (result.response !== 1) throw new Error('Application permission was not granted');
    await this.runtime.exclusive(id, async () => {
      signal.throwIfAborted();
      const current = await this.store.get(id);
      current.grants = [...new Set([...current.grants, ...missing])];
      await this.store.save(current);
    });
  }
  async open(id: string, repair = false, restart = false, signal?: AbortSignal): Promise<void> {
    if (this.shuttingDown) throw new Error('Application host is stopping');
    signal?.throwIfAborted();
    const pending = this.opening.get(id); if (pending) return pending.promise;
    const controller = new AbortController();
    const cancel = () => controller.abort();
    signal?.addEventListener('abort', cancel, { once: true });
    const previousInvocation = this.runtime.invocationId(id);
    const operation = { controller, promise: this.openOnce(id, controller.signal, repair, restart) };
    this.opening.set(id, operation);
    try { await operation.promise; }
    catch (error) {
      if (!this.shuttingDown && ['running', 'starting', 'installing', 'building'].includes(this.runtime.phase(id))) {
        const ownsNewRun = this.runtime.invocationId(id) !== previousInvocation;
        const orphanedForeground = !this.windows.has(id) && !(await this.store.get(id)).background;
        if (ownsNewRun || orphanedForeground) await this.runtime.stop(id);
      }
      throw error;
    } finally {
      signal?.removeEventListener('abort', cancel);
      if (this.opening.get(id) === operation) this.opening.delete(id);
    }
  }
  private async openOnce(id: string, signal: AbortSignal, repair: boolean, restart: boolean): Promise<void> {
    const row = await this.store.get(id);
    const existing = this.windows.get(id);
    if (existing && !existing.isDestroyed() && !restart && this.runtime.phase(id) === 'running' && this.runtime.revision(id) === row.revision) { existing.show(); existing.focus(); return; }
    await this.authorize(id, signal);
    signal.throwIfAborted();
    if (this.runtime.revision(id) !== row.revision) await this.runtime.stop(id);
    signal.throwIfAborted();
    await this.runtime.start(id, repair, signal);
    signal.throwIfAborted();
    this.launchGrants.set(id, (await this.store.get(id)).grants.filter(cap => this.runtime.manifest(id)?.capabilities?.includes(cap)));
    await this.configureSession(id);
    await this.clearCodeCache(id);
    signal.throwIfAborted();
    const entryUrl = appEntryUrl(id, this.runtime.manifest(id)!.entry);
    if (existing && !existing.isDestroyed()) {
      await existing.loadURL(entryUrl); existing.show(); existing.focus(); return;
    }
    const window = new BrowserWindow({ width: 1080, height: 760, minWidth: 360, minHeight: 320, title: row.name, show: false,
      webPreferences: { partition: `persist:local-app-${id}`, preload: this.options.preload, contextIsolation: true, nodeIntegration: false, sandbox: true, webSecurity: true, navigateOnDragDrop: false } });
    this.windows.set(id, window);
    const origin = `${LOCAL_APP_SCHEME}://${id}`;
    window.webContents.on('will-navigate', (event, url) => { if (!url.startsWith(origin + '/')) { event.preventDefault(); void this.external(url); } });
    window.webContents.setWindowOpenHandler(({ url }) => { void this.external(url); return { action: 'deny' }; });
    window.webContents.on('will-attach-webview', event => event.preventDefault());
    window.on('closed', () => {
      if (this.windows.get(id) === window) this.windows.delete(id);
      if (!this.shuttingDown) void this.store.get(id).then(row => !row.background && !this.windows.has(id) && !this.opening.has(id) ? this.runtime.stop(id) : undefined).catch(() => undefined);
    });
    try { await window.loadURL(entryUrl); window.show(); }
    catch (error) { window.destroy(); await this.runtime.stop(id); throw error; }
  }
  private async external(raw: string): Promise<void> { try { const url = new URL(raw); if (['https:', 'http:', 'mailto:'].includes(url.protocol)) await shell.openExternal(url.toString()); } catch { /* invalid URL */ } }
  private async clearCodeCache(id: string): Promise<void> {
    const partition = session.fromPartition(`persist:local-app-${id}`);
    await partition.clearCache();
    await partition.clearStorageData({ storages: ['serviceworkers', 'cachestorage'] });
  }
  private async configureSession(id: string): Promise<void> {
    if (this.configured.has(id)) return;
    const partition = session.fromPartition(`persist:local-app-${id}`);
    const permission = permissionCapabilities;
    partition.setPermissionCheckHandler((contents, kind, origin, details) => {
      if (!contents || contents !== this.windows.get(id)?.webContents || !details.isMainFrame || !isAppUrl(id, origin) || this.runtime.phase(id) !== 'running') return false;
      // The synchronous check only admits capabilities approved for this launch.
      return permission(kind, details).length > 0 && permission(kind, details).every(c => this.launchGrants.get(id)?.includes(c));
    });
    partition.setPermissionRequestHandler(async (contents, kind, callback, details) => {
      try {
        const invocation = this.runtime.invocationId(id);
        const current = () => contents === this.windows.get(id)?.webContents && !contents.isDestroyed() && details.isMainFrame && isAppUrl(id, details.requestingUrl) && isAppUrl(id, contents.getURL()) && this.runtime.phase(id) === 'running' && this.runtime.invocationId(id) === invocation;
        if (!current()) { callback(false); return; }
        const row = await this.store.get(id), required = permission(kind, details);
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
        if (!workspace || !revision || this.runtime.status(row).phase !== 'running') return new Response('Application is stopped', { status: 503 });
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
          const target = service.url + path + url.search;
          const headers = new Headers(request.headers); headers.delete('host'); headers.delete('origin');
          response = await partition.fetch(target, { method: request.method, headers, ...(request.method !== 'GET' && request.method !== 'HEAD' ? { body: await request.arrayBuffer() } : {}), redirect: 'manual' });
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
    const id = pair[0], invocation = this.runtime.invocationId(id), row = await this.store.get(id), args = input.args ?? {};
    const current = () => {
      if (this.windows.get(id) !== pair[1] || pair[1].isDestroyed() || this.runtime.invocationId(id) !== invocation || this.runtime.phase(id) !== 'running') throw new Error('Application operation is no longer active');
    };
    current();
    const need = (cap: LocalAppCapability) => { current(); if (!row.grants.includes(cap) || !this.launchGrants.get(id)?.includes(cap)) throw new Error(`Application capability not granted: ${cap}`); };
    if (input.method === 'info') return { id, name: row.name, revision: this.runtime.revision(id), platform: process.platform, dataDirectory: this.store.dataPath(id) };
    if (input.method === 'serviceSocketUrl') {
      const service = this.runtime.service(id, String(args.name));
      if (!service) throw new Error('Backend is unavailable');
      const path = String(args.path ?? '/');
      if (!path.startsWith('/') || path.startsWith('//') || path.includes('\\')) throw new Error('Invalid service path');
      return service.url.replace('http:', 'ws:') + path;
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
