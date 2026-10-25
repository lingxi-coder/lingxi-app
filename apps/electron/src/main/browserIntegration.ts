import { createServer, type IncomingMessage, type ServerResponse, type Server } from 'node:http';
import { createHash, randomBytes, randomUUID, timingSafeEqual } from 'node:crypto';
import { chmodSync, existsSync, mkdirSync, renameSync, unlinkSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { homedir } from 'node:os';
import type { ComputerDevicePolicy, ComputerPermission } from '../shared/computerSettings.js';

export const BROWSER_PROTOCOL = 'lingxi-browser/1';
export const BROWSER_EXTENSION_ID = 'nciganheacfkdbpdfdnjnjdkcabfbkmg';
const NATIVE_HOST = 'com.lingxi.browser';
const MAX_BODY = 16 * 1024 * 1024;
const ACTIONS = new Set(['navigate', 'tabs', 'screenshot', 'click', 'type', 'key', 'history', 'download', 'site_tools', 'cdp']);
type RecordValue = Record<string, unknown>;
type BrowserStatus = { connected: boolean; installed: boolean; extensionDirectory: string; error?: string };
interface BrowserOptions {
  policy(): ComputerDevicePolicy;
  descriptorPath: string;
  nativeHostPath: string;
  bridgeExecutable: string;
  extensionDirectory: string;
  onChanged?(): void;
  /** Hermetic tests never install browser configuration in the real profile. */
  nativeManifestPath?: string;
}
interface PendingCommand { resolve(value: unknown): void; reject(error: Error): void; timer: NodeJS.Timeout; revision: string; sessionId?: string }
interface Approval { fingerprint: string; sessionId: string; revision: string; origin: string; scope: string; expires: number }
function object(value: unknown): RecordValue {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid browser request');
  return value as RecordValue;
}
export function browserOrigin(value: unknown): string {
  if (typeof value !== 'string' || value.length > 8192) throw new Error('A valid HTTP or HTTPS URL is required');
  const url = new URL(value);
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password) throw new Error('Only HTTP and HTTPS sites are supported');
  return url.origin;
}
function canonical(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonical).join(',')}]`;
  if (value && typeof value === 'object') return `{${Object.entries(value).sort(([a],[b]) => a.localeCompare(b)).map(([key,item]) => `${JSON.stringify(key)}:${canonical(item)}`).join(',')}}`;
  return JSON.stringify(value);
}
export function browserPermission(policy: ComputerDevicePolicy, origin: string, scope: string): ComputerPermission {
  if (!policy.enabled) return 'deny';
  if (scope === 'history') return policy.browser.history;
  const site = policy.browser.sites.find(site => site.origin === origin);
  return scope === 'downloads' ? site?.downloads ?? policy.browser.defaultDownloads : site?.browsing ?? policy.browser.defaultBrowsing;
}
function shellQuote(value: string): string { return `'${value.replaceAll("'", "'\\''")}'`; }
function writePrivate(path: string, value: unknown): void {
  mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
  const temp = `${path}.${randomUUID()}.tmp`;
  writeFileSync(temp, `${JSON.stringify(value)}\n`, { mode: 0o600 });
  renameSync(temp, path);
}

/** Native Chrome extension transport; policy and credentials stay in Electron main. */
export class BrowserIntegration {
  private server?: Server;
  private readonly token = randomBytes(32).toString('hex');
  private nativeToken = randomBytes(32).toString('hex');
  private peerId?: string;
  private endpoint?: string;
  private lastHeartbeat = 0;
  private lastError?: string;
  private poll?: { response: ServerResponse; timer: NodeJS.Timeout };
  private readonly queue: RecordValue[] = [];
  private readonly pending = new Map<string, PendingCommand>();
  private readonly approvals = new Map<string, Approval>();
  private readonly grants = new Map<string, Set<string>>();
  private readonly sessionOperations = new Map<string, Promise<unknown>>();
  private readonly activeOperations = new Map<string, AbortController>();
  private readonly operationControllers = new Set<AbortController>();
  private heartbeatTimer?: NodeJS.Timeout;
  private nativeEnabled: boolean;
  constructor(private readonly options: BrowserOptions) { this.nativeEnabled = existsSync(this.manifestPath); }

  private get manifestPath(): string {
    return this.options.nativeManifestPath ?? join(homedir(), 'Library/Application Support/Google/Chrome/NativeMessagingHosts', `${NATIVE_HOST}.json`);
  }
  status(): BrowserStatus {
    const elapsed = Date.now() - this.lastHeartbeat;
    return { connected: this.nativeEnabled && existsSync(this.manifestPath) && this.lastHeartbeat > 0 && elapsed >= 0 && elapsed < 35_000, installed: existsSync(this.manifestPath), extensionDirectory: this.options.extensionDirectory, ...(this.lastError ? { error: this.lastError } : {}) };
  }
  async start(): Promise<{ url: string; token: string }> {
    if (this.endpoint) return { url: this.endpoint, token: this.token };
    const server = createServer((request,response) => { void this.handle(request,response).catch(error => this.reply(response, { error: error instanceof Error ? error.message : 'Browser request failed' }, 400)); });
    this.server = server;
    await new Promise<void>((resolve,reject) => { server.once('error',reject); server.listen(0,'127.0.0.1',() => { server.off('error',reject); resolve(); }); });
    const address = server.address();
    if (!address || typeof address === 'string') throw new Error('Browser host did not open a loopback port');
    this.endpoint = `http://127.0.0.1:${address.port}`;
    writePrivate(this.options.descriptorPath, { url: this.endpoint, token: this.token });
    writePrivate(`${this.options.descriptorPath}.native`, { url: this.endpoint, token: this.nativeToken });
    this.heartbeatTimer = setInterval(() => this.options.onChanged?.(), 15_000);
    this.heartbeatTimer.unref();
    return { url: this.endpoint, token: this.token };
  }
  async retry(): Promise<BrowserStatus> {
    try { await this.dispatch({ action:'ping' },this.options.policy().revision); this.lastError = undefined; }
    catch (error) { this.lastError = error instanceof Error ? error.message : 'Browser extension is not connected'; }
    this.options.onChanged?.(); return this.status();
  }
  async install(assertCurrent: () => void = () => {}): Promise<BrowserStatus> {
    assertCurrent();
    if (process.platform !== 'darwin' && !this.options.nativeManifestPath) throw new Error('Chrome native-host setup is currently supported on macOS');
    await this.start();
    assertCurrent();
    if (!existsSync(this.options.bridgeExecutable) || !existsSync(join(this.options.extensionDirectory,'manifest.json'))) throw new Error('Browser integration resources are missing from this Desktop installation');
    writePrivate(`${this.options.descriptorPath}.native`,{ url:this.endpoint,token:this.nativeToken });
    mkdirSync(dirname(this.options.nativeHostPath), { recursive: true, mode: 0o700 });
    const script = `#!/bin/sh\nexec ${shellQuote(this.options.bridgeExecutable)} --browser-native-host ${shellQuote(`${this.options.descriptorPath}.native`)} "$@"\n`;
    writeFileSync(this.options.nativeHostPath,script,{ mode:0o700 }); chmodSync(this.options.nativeHostPath,0o700);
    writePrivate(this.manifestPath,{ name:NATIVE_HOST, description:'LingXi browser integration', path:this.options.nativeHostPath, type:'stdio', allowed_origins:[`chrome-extension://${BROWSER_EXTENSION_ID}/`] });
    this.nativeEnabled = true;
    this.lastError = this.status().connected ? undefined : 'Load the LingXi Browser extension in Chrome to connect';
    this.options.onChanged?.(); return this.status();
  }
  async remove(): Promise<BrowserStatus> {
    this.policyChanged();
    this.nativeEnabled = false;
    this.nativeToken = randomBytes(32).toString('hex');
    this.peerId = undefined;
    this.queue.length = 0;
    if (this.poll) { const poll = this.poll; this.poll = undefined; clearTimeout(poll.timer); this.reply(poll.response,{ type:'disconnect' }); }
    for (const path of [this.manifestPath,this.options.nativeHostPath,`${this.options.descriptorPath}.native`]) {
      try { unlinkSync(path); } catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
    }
    this.lastHeartbeat = 0; this.lastError = undefined; this.options.onChanged?.(); return this.status();
  }
  policyChanged(): void {
    this.approvals.clear(); this.grants.clear();
    for (const controller of this.operationControllers) controller.abort();
    for (const command of this.pending.values()) { clearTimeout(command.timer); command.reject(new Error('Browser permissions changed; request access again')); }
    this.pending.clear(); this.queue.length = 0;
    let revision: string;
    try { revision = this.options.policy().revision; } catch { revision = `unavailable-${randomUUID()}`; }
    this.enqueue({ type:'revoke', revision });
  }
  async close(): Promise<void> {
    this.policyChanged();
    this.nativeEnabled = false; this.lastHeartbeat = 0; this.peerId = undefined;
    clearInterval(this.heartbeatTimer);
    if (this.poll) { clearTimeout(this.poll.timer); this.reply(this.poll.response,{ type:'disconnect' }); this.poll = undefined; }
    const server = this.server; this.server = undefined; this.endpoint = undefined;
    server?.closeAllConnections();
    if (server) await new Promise<void>(resolve => server.close(() => resolve()));
    for (const path of [this.options.descriptorPath,`${this.options.descriptorPath}.native`]) try { unlinkSync(path); } catch { /* Already removed. */ }
  }
  private reply(response: ServerResponse, value: unknown, status = 200): void {
    if (response.writableEnded || response.destroyed) return;
    const bytes = Buffer.from(JSON.stringify(value));
    response.writeHead(status,{ 'Content-Type':'application/json', 'Content-Length':bytes.length, 'Cache-Control':'no-store', 'Connection':'close' }); response.end(bytes);
  }
  private async readBody(request: IncomingMessage): Promise<RecordValue> {
    const chunks: Buffer[] = []; let size = 0;
    for await (const part of request) { size += part.length; if (size > MAX_BODY) throw new Error('Browser message is too large'); chunks.push(Buffer.from(part)); }
    return object(JSON.parse(Buffer.concat(chunks).toString('utf8')));
  }
  private async handle(request: IncomingMessage,response: ServerResponse): Promise<void> {
    const supplied = request.headers.authorization?.replace(/^Bearer /,'') ?? '';
    const expected = request.url?.startsWith('/extension/') ? this.nativeToken : this.token;
    if (request.headers.origin || supplied.length !== expected.length || !timingSafeEqual(Buffer.from(supplied),Buffer.from(expected))) { this.reply(response,{ error:'Unauthorized browser transport' },403); return; }
    if (request.url?.startsWith('/extension/') && (!this.nativeEnabled || !existsSync(this.manifestPath))) { this.reply(response,{ error:'Browser connection was removed' },403); return; }
    if (request.url === '/extension/poll' && request.method === 'GET') {
      if (!this.peerId || request.headers['x-lingxi-browser-peer'] !== this.peerId) { this.reply(response,{ error:'Browser profile is not paired' },403); return; }
      if (this.poll) { clearTimeout(this.poll.timer); this.reply(this.poll.response,{ type:'ping' }); }
      const next = this.queue.shift();
      if (next) { this.reply(response,next); return; }
      const timer = setTimeout(() => { if (this.poll?.response === response) this.poll = undefined; this.reply(response,{ type:'ping' }); },20_000);
      this.poll = { response,timer };
      response.once('close',() => { clearTimeout(timer); if (this.poll?.response === response) this.poll = undefined; }); return;
    }
    if (request.url === '/extension/message' && request.method === 'POST') {
      const message = await this.readBody(request);
      if (message.protocol !== BROWSER_PROTOCOL || message.extensionId !== BROWSER_EXTENSION_ID) throw new Error('Unsupported browser extension protocol');
      if (typeof message.peerId !== 'string' || !/^[a-zA-Z0-9-]{1,64}$/.test(message.peerId)) throw new Error('Invalid browser profile identity');
      if (message.peerId !== this.peerId && message.type !== 'hello') throw new Error('Browser handshake is required');
      if (this.peerId && message.peerId !== this.peerId && this.status().connected) throw new Error('Another Chrome profile is already connected');
      // A new extension instance may belong to a different Chrome profile.
      // No approval or conversation grant can cross that identity boundary.
      if (this.peerId && this.peerId !== message.peerId) this.policyChanged();
      this.peerId = message.peerId;
      const wasConnected = this.status().connected; this.lastHeartbeat = Date.now();
      if (!wasConnected) { this.lastError = undefined; this.options.onChanged?.(); }
      if (message.type === 'hello') this.enqueue({ type:'revoke',revision:this.options.policy().revision });
      if (typeof message.id === 'string') {
        const pending = this.pending.get(message.id);
        if (pending) {
          this.pending.delete(message.id); clearTimeout(pending.timer);
          try {
            if (pending.revision !== this.options.policy().revision) pending.reject(new Error('Browser permissions changed'));
            else if (typeof message.error === 'string') pending.reject(new Error(message.error));
            else pending.resolve(message.result);
          } catch { pending.reject(new Error('Browser permission settings are unavailable')); }
        }
      }
      this.reply(response,{ ok:true }); return;
    }
    if (request.url === '/agent' && request.method === 'POST') {
      const input = await this.readBody(request);
      if (typeof input.sessionId !== 'string' || !/^[A-Za-z0-9_.:-]{1,192}$/.test(input.sessionId)) throw new Error('Invalid browser session identity');
      const sessionId = input.sessionId;
      const controller = new AbortController();
      this.operationControllers.add(controller);
      const previous = this.sessionOperations.get(input.sessionId) ?? Promise.resolve();
      const operation = previous.catch(() => undefined).then(async () => {
        if (controller.signal.aborted) throw new Error('Browser operation was cancelled');
        this.activeOperations.set(sessionId,controller);
        try { return await this.execute(input,controller.signal); }
        finally { if (this.activeOperations.get(sessionId) === controller) this.activeOperations.delete(sessionId); }
      });
      this.sessionOperations.set(input.sessionId,operation);
      response.once('close',() => {
        if (response.writableEnded) return;
        controller.abort();
        if (this.activeOperations.get(sessionId) === controller) this.cancelSession(sessionId);
      });
      try { this.reply(response,await operation); } finally { this.operationControllers.delete(controller); if (this.sessionOperations.get(input.sessionId) === operation) this.sessionOperations.delete(input.sessionId); }
      return;
    }
    this.reply(response,{ error:'Unknown browser transport operation' },404);
  }
  private enqueue(command: RecordValue): void {
    if (this.poll) { const poll = this.poll; this.poll = undefined; clearTimeout(poll.timer); this.reply(poll.response,command); }
    else this.queue.push(command);
  }
  private dispatch(payload: RecordValue, revision: string, signal?: AbortSignal): Promise<unknown> {
    if (signal?.aborted) return Promise.reject(new Error('Browser operation was cancelled'));
    if (!this.status().connected && payload.action !== 'ping') return Promise.reject(new Error('LingXi Browser extension is not connected. Install or reconnect it in Computer use settings.'));
    if (this.pending.size >= 64) return Promise.reject(new Error('Too many browser operations are pending'));
    const id = randomUUID();
    return new Promise((resolve,reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        const index = this.queue.findIndex(item => item.id === id); if (index >= 0) this.queue.splice(index,1);
        // Timing out the HTTP result does not stop a command already delivered
        // to Chrome. Invalidate its generation before another call can run.
        if (typeof payload.sessionId === 'string') this.cancelSession(payload.sessionId);
        reject(new Error('Browser extension did not respond'));
      },payload.action === 'ping' ? 3000 : 30_000);
      this.pending.set(id,{ resolve,reject,timer,revision,sessionId:typeof payload.sessionId === 'string' ? payload.sessionId : undefined }); this.enqueue({ type:'command',id,revision,payload });
    });
  }
  private async execute(envelope: RecordValue, signal: AbortSignal): Promise<unknown> {
    const check = () => { if (signal.aborted) throw new Error('Browser operation was cancelled'); };
    check();
    const { approvalNonce,approved,...input } = envelope;
    const action = input.action;
    if (typeof action !== 'string' || !ACTIONS.has(action)) throw new Error('Unknown browser action');
    if (canonical(input).length > 262_144) throw new Error('Browser action is too large');
    const sessionId = input.sessionId as string;
    const policy = this.options.policy();
    if (!policy.enabled) throw new Error('Computer use is turned off');
    if (action === 'site_tools' && !policy.browser.siteTools) throw new Error('Site tools are disabled in Browser settings');
    if (action === 'cdp' && !policy.browser.fullCdp) throw new Error('Full CDP access is disabled in Browser settings');
    if (action === 'tabs') {
      const result = object(await this.dispatch({ action:'tabs',sessionId },policy.revision,signal));
      check();
      const tabs = Array.isArray(result.tabs) ? result.tabs.filter(tab => { try { const origin = browserOrigin(object(tab).url); return this.permitted(policy,sessionId,origin,'browsing'); } catch { return false; } }) : [];
      return { result:{ tabs } };
    }
    const scope = action === 'history' ? 'history' : action === 'download' ? 'downloads' : 'browsing';
    const origin = action === 'history' ? '*' : ['navigate','download'].includes(action) ? browserOrigin(input.url) : browserOrigin(object(await this.dispatch({ action:'inspect',sessionId,tabId:input.tabId },policy.revision,signal)).url);
    check();
    const fingerprint = createHash('sha256').update(canonical(input)).digest('hex');
    if (approvalNonce !== undefined) {
      if (typeof approvalNonce !== 'string') throw new Error('Invalid browser approval');
      const approval = this.approvals.get(approvalNonce); this.approvals.delete(approvalNonce);
      if (!approval || approved !== true || approval.fingerprint !== fingerprint || approval.sessionId !== sessionId || approval.revision !== policy.revision || approval.origin !== origin || approval.scope !== scope || approval.expires < Date.now()) throw new Error('Browser approval expired or does not match this request');
      const grants = this.grants.get(sessionId) ?? new Set<string>(); grants.add(`${scope}:${origin}`); this.grants.set(sessionId,grants);
    }
    const permission = browserPermission(policy,origin,scope);
    if (permission === 'deny') throw new Error(`Browser ${scope} access is denied for ${origin}`);
    if (!this.permitted(policy,sessionId,origin,scope)) {
      for (const [key,value] of this.approvals) if (value.expires < Date.now()) this.approvals.delete(key);
      if (this.approvals.size >= 128) throw new Error('Too many browser approvals are pending');
      const nonce = randomBytes(24).toString('hex');
      this.approvals.set(nonce,{ fingerprint,sessionId,revision:policy.revision,origin,scope,expires:Date.now()+300_000 });
      return { approval:{ nonce,label:scope === 'history' ? 'Browser history' : `${scope === 'downloads' ? 'Downloads from' : 'Browsing'} ${origin}`,revision:policy.revision } };
    }
    if (this.options.policy().revision !== policy.revision) throw new Error('Browser permissions changed');
    const allowedOrigins = new Set(policy.browser.sites.filter(site => site.browsing === 'allow').map(site => site.origin));
    for (const grant of this.grants.get(sessionId) ?? []) if (grant.startsWith('browsing:')) allowedOrigins.add(grant.slice(9));
    if (scope === 'browsing') allowedOrigins.add(origin);
    const result = await this.dispatch({ ...input,expectedOrigin:origin,policy:{ revision:policy.revision,defaultBrowsing:policy.browser.defaultBrowsing,sites:policy.browser.sites,allowedOrigins:[...allowedOrigins],fullCdp:policy.browser.fullCdp,siteTools:policy.browser.siteTools } },policy.revision,signal);
    check();
    if (this.options.policy().revision !== policy.revision) throw new Error('Browser permissions changed');
    return { result };
  }
  private permitted(policy: ComputerDevicePolicy,sessionId: string,origin: string,scope: string): boolean {
    const permission = browserPermission(policy,origin,scope);
    return permission === 'allow' || permission === 'ask' && (this.grants.get(sessionId)?.has(`${scope}:${origin}`) ?? false);
  }
  private cancelSession(sessionId: string): void {
    for (const [id,command] of this.pending) if (command.sessionId === sessionId) { clearTimeout(command.timer); command.reject(new Error('Browser operation was cancelled')); this.pending.delete(id); }
    for (let index = this.queue.length-1; index >= 0; index--) if (object(this.queue[index]!.payload ?? {}).sessionId === sessionId) this.queue.splice(index,1);
    this.enqueue({ type:'cancel',sessionId });
  }
}
