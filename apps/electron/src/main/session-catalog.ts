import type { ProviderAccountCredentialVersion } from './provider-accounts.js';
import type { HostProviderSettings, ProviderAccountCommand, ProviderAccountSnapshot } from '../shared/providerAccounts.js';
import { spawn } from 'node:child_process';
import { realpathSync } from 'node:fs';
import { homedir } from 'node:os';
import { basename, isAbsolute } from 'node:path';
import type { SessionRowDto } from '@lingxi/bridge-client';
import { validateClientEvent } from '@lingxi/bridge-client';
import { historyCursor, type TranscriptHistoryCursor, type TranscriptHistoryPage } from '../shared/sessionHistory.js';

import { buildBridgeArguments, buildBridgeEnvironment } from './host-utils.js';
import { resolveServerBin } from './bridgeDiscovery.js';
import type { BridgeManagerOptions, WorkspaceLaunchSettings } from './bridgeTypes.js';
import { isSessionId } from './sessionIdentity.js';
import { sessionCatalogScope, type SessionCatalogScope, type SessionOpenSelection } from '../shared/sessionWorkspace.js';

const MAX_OUTPUT_BYTES = 8 * 1024 * 1024;
const MAX_SESSIONS = 200;
const DEFAULT_CACHE_TTL_MS = 15_000;
const MAX_CATALOG_CHILDREN = 2;
const MAX_QUEUED_CATALOG_OPERATIONS = 512;
const MAX_WORKER_HEADER_BYTES = 4_096;
const MAX_WORKER_REQUEST_BYTES = 64 * 1024;
const DEFAULT_WORKER_IDLE_MS = 30_000;
const CHILD_TERMINATION_RETRY_MS = 100;

export interface ProviderAccountConfiguration {
  profilesJson: string;
  providerRegion: 'international' | 'china_mainland';
}

export interface ProjectSessionCatalogOptions {
  appVersion?: string;
  providerAccountChannel?: 'development' | 'production';
  serverBin?: string;
  isPackaged?: boolean;
  resourcesPath?: string;
  timeoutMs?: number;
  /** Keep project metadata warm while session/runtime work is in flight. */
  cacheTtlMs?: number;
  /** Use the metadata-only worker; false retains the public oneshot CLI path. */
  catalogWorker?: boolean;
  /** Retire an unused metadata worker rather than keeping its pipes/resources open. */
  workerIdleMs?: number;
  /**
   * Read this configuration home instead of the default one. The catalog modes
   * of the bridge server take the home from `LINGXI_CONFIG_DIR` only (they
   * ignore `--session-config-home`), so this sets that variable for the child.
   */
  configHome?: string;
  spawnProcess?: typeof spawn;
}

export interface ProjectSessionCatalogResult {
  sessions: ProjectSessionCatalogRow[];
}

export interface ProductPaths {
  brandHome: string;
  workspaceRoot: string;
  appProjectsRoot: string;
  globalConfigPath?: string;
}

export interface ProjectSessionCatalogRow extends SessionRowDto {
  /** Private bridge-server catalog marker; never crosses into renderer state. */
  empty_session: boolean;
  /** Private launch hint reconstructed from the session transcript. */
  resume_model?: string;
  /** Authoritative source cwd; used only by the host when opening this UUID. */
  workspace_path?: string;
  resume_title?: string;
}

interface CatalogCacheEntry {
  result: ProjectSessionCatalogResult;
  expiresAt: number;
}

/** A lease covers the child's lifetime, including a timed-out writer's drain. */
class CatalogChildAdmission {
  private active = 0;
  private readonly waiting: Array<() => void> = [];
  private readonly idle = new Map<() => void, () => void>();
  private readonly unreaped = new Set<() => void>();

  start<T>(start: (release: () => void) => T, waitMs = 15_000): Promise<Awaited<T>> {
    return new Promise<Awaited<T>>((resolve, reject) => {
      const launch = () => {
        this.active++;
        let released = false;
        const release = () => {
          if (released) return;
          released = true;
          this.idle.delete(release);
          this.unreaped.delete(release);
          this.active--;
          this.drain();
        };
        try { resolve(start(release) as Awaited<T>); }
        catch (error) { release(); reject(error); }
      };
      if (this.active < MAX_CATALOG_CHILDREN && this.waiting.length === 0) launch();
      else if (this.waiting.length >= MAX_QUEUED_CATALOG_OPERATIONS) reject(this.busyError());
      else {
        const admitted = () => { clearTimeout(timeout); launch(); };
        const timeout = setTimeout(() => {
          const index = this.waiting.indexOf(admitted);
          if (index < 0) return;
          this.waiting.splice(index, 1);
          reject(this.busyError());
          this.drain();
        }, waitMs);
        timeout.unref();
        this.waiting.push(admitted);
        this.drain();
      }
    });
  }

  busy(release: () => void): void { this.idle.delete(release); }

  notReaped(release: () => void): void { this.unreaped.add(release); }

  private busyError(): Error {
    return new Error(this.unreaped.size > 0
      ? 'session catalog is busy; a previous metadata child has not closed after termination was attempted'
      : 'session catalog is busy; try again shortly');
  }

  markIdle(release: () => void, retire: () => void): void {
    this.idle.set(release, retire);
    this.drain();
  }

  private drain(): void {
    while (this.waiting.length > 0 && this.active < MAX_CATALOG_CHILDREN) this.waiting.shift()!();
    // A persistent reader must give up its process slot when another helper,
    // especially a durable metadata writer, is waiting to start.
    if (this.waiting.length > 0) {
      const idle = this.idle.entries().next().value;
      if (idle) { this.idle.delete(idle[0]); idle[1](); }
    }
  }
}

const catalogChildren = new CatalogChildAdmission();

/** Signalling is an attempt; only the ChildProcess close event returns its lease. */
class CatalogChildTermination {
  private requested = false;
  private exited = false;
  private closed = false;
  private retry?: ReturnType<typeof setTimeout>;

  constructor(private readonly child: ReturnType<typeof spawn>, private readonly release: () => void) {
    const stopRetries = () => { if (this.retry) clearTimeout(this.retry); this.retry = undefined; };
    child.once('exit', () => { this.exited = true; stopRetries(); });
    child.once('close', () => { this.closed = true; stopRetries(); });
  }

  error(): void {
    if (this.requested && !this.closed && !this.exited) catalogChildren.notReaped(this.release);
  }

  stop(): void {
    if (this.requested || this.closed || this.exited) return;
    this.requested = true;
    // EOF lets a private reader stop normally even if signal delivery fails.
    try { this.child.stdin?.end(); } catch { /* Signal attempts below remain bounded. */ }
    this.signal('SIGTERM');
    if (this.closed || this.exited) return;
    this.retry = setTimeout(() => {
      this.retry = undefined;
      if (this.closed || this.exited) return;
      this.signal('SIGKILL');
      if (this.closed || this.exited) return;
      this.retry = setTimeout(() => {
        this.retry = undefined;
        if (!this.closed && !this.exited) catalogChildren.notReaped(this.release);
      }, CHILD_TERMINATION_RETRY_MS);
      this.retry.unref();
    }, CHILD_TERMINATION_RETRY_MS);
    this.retry.unref();
  }

  private signal(signal: NodeJS.Signals): void {
    if (this.closed || this.exited) return;
    try { if (!this.child.kill(signal)) this.error(); }
    catch { this.error(); }
  }
}

interface CatalogWorkerQuery {
  cwd: string;
  sessionId?: string;
  scope?: SessionCatalogScope;
  transcriptPath?: string;
  historyBefore?: TranscriptHistoryCursor;
}

/** One request at a time bounds worker IO; catalog callers retain their own deduplication. */
class CatalogWorker {
  private nextId = 0;
  private current?: { id: number; resolve(output: string): void; reject(error: Error): void; timeout: ReturnType<typeof setTimeout> };
  private readonly header = Buffer.alloc(MAX_WORKER_HEADER_BYTES);
  private headerBytes = 0;
  private response?: { body: Buffer; bytes: number; kind: 'output' | 'error' };
  private stderr = Buffer.alloc(0);
  private idleTimer?: ReturnType<typeof setTimeout>;
  private alive = true;
  private retiring = false;
  private readonly termination: CatalogChildTermination;

  constructor(
    private readonly child: ReturnType<typeof spawn>,
    private readonly release: () => void,
    private readonly timeoutMs: number,
    private readonly idleMs: number,
  ) {
    this.termination = new CatalogChildTermination(child, release);
    child.stdout!.on('data', (chunk: Buffer | string) => this.receive(typeof chunk === 'string' ? Buffer.from(chunk) : chunk));
    child.stderr!.on('data', (chunk: Buffer | string) => {
      const bytes = typeof chunk === 'string' ? Buffer.from(chunk) : chunk;
      this.stderr = Buffer.concat([this.stderr, bytes.subarray(Math.max(0, bytes.length - 16 * 1024))]).subarray(-16 * 1024);
    });
    child.stdin!.on('error', () => this.fail(new Error('session catalog worker input failed')));
    child.on('error', (error) => {
      this.termination.error();
      this.fail(new Error(`session catalog failed to start: ${error.message}`));
    });
    child.once('close', (code, signal) => {
      this.alive = false;
      let firstCodepoint = 0;
      while (firstCodepoint < Math.min(3, this.stderr.length) && (this.stderr[firstCodepoint] & 0xc0) === 0x80) firstCodepoint++;
      const diagnostic = this.stderr.toString('utf8', firstCodepoint).trim();
      this.fail(new Error(`session catalog failed (code=${code ?? 'null'}, signal=${signal ?? 'null'}): ${diagnostic || 'worker closed'}`));
      this.release();
    });
  }

  get available(): boolean { return this.alive && !this.retiring; }

  request(query: CatalogWorkerQuery): Promise<string> {
    if (!this.available || this.current) return Promise.reject(new Error('session catalog worker is unavailable'));
    if (this.idleTimer) clearTimeout(this.idleTimer);
    catalogChildren.busy(this.release);
    const id = ++this.nextId;
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => this.fail(new Error('session catalog timed out')), this.timeoutMs);
      timeout.unref();
      this.current = { id, resolve, reject, timeout };
      const frame = `${JSON.stringify({ version: 2, id, ...query })}\n`;
      if (Buffer.byteLength(frame, 'utf8') > MAX_WORKER_REQUEST_BYTES) { this.fail(new Error('session catalog request is too large')); return; }
      try { this.child.stdin!.write(frame); }
      catch { this.fail(new Error('session catalog worker input failed')); }
    });
  }

  retire(): void {
    this.retiring = true;
    if (!this.current) this.stop();
  }

  private stop(): void {
    this.retiring = true;
    if (this.idleTimer) clearTimeout(this.idleTimer);
    catalogChildren.busy(this.release);
    if (this.alive) this.termination.stop();
  }

  private fail(error: Error): void {
    const current = this.current;
    this.current = undefined;
    this.headerBytes = 0;
    this.response = undefined;
    if (current) { clearTimeout(current.timeout); current.reject(error); }
    this.stop();
  }

  private receive(chunk: Buffer): void {
    if (!this.available && !this.current) return;
    let start = 0;
    while (start < chunk.length) {
      if (!this.response) {
        const newline = chunk.indexOf(10, start);
        const end = newline < 0 ? chunk.length : newline;
        const length = end - start;
        if (this.headerBytes + length > MAX_WORKER_HEADER_BYTES) {
          this.fail(new Error('session catalog worker header is too large')); return;
        }
        chunk.copy(this.header, this.headerBytes, start, end);
        this.headerBytes += length;
        if (newline < 0) return;
        let value: { version?: unknown; id?: unknown; bytes?: unknown; kind?: unknown };
        try { value = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(this.header.subarray(0, this.headerBytes))); }
        catch { this.fail(new Error('session catalog returned invalid worker JSON')); return; }
        this.headerBytes = 0;
        if (!this.current || !value || typeof value !== 'object' || Array.isArray(value)
          || Object.keys(value).some(key => !['version', 'id', 'bytes', 'kind'].includes(key))
          || value.version !== 2 || !Number.isSafeInteger(value.id) || value.id !== this.current.id
          || typeof value.bytes !== 'number' || !Number.isSafeInteger(value.bytes) || value.bytes < 1
          || value.bytes > MAX_OUTPUT_BYTES || (value.kind !== 'output' && value.kind !== 'error')) {
          this.fail(new Error('session catalog returned an invalid worker response')); return;
        }
        // The bounded length is known before allocation. A single destination
        // also bounds bookkeeping when the pipe fragments the body byte by byte.
        this.response = { body: Buffer.allocUnsafe(value.bytes), bytes: 0, kind: value.kind };
        start = newline + 1;
        continue;
      }
      const response = this.response;
      const length = Math.min(response.body.length - response.bytes, chunk.length - start);
      chunk.copy(response.body, response.bytes, start, start + length);
      response.bytes += length;
      start += length;
      if (response.bytes < response.body.length || start === chunk.length) return;
      if (chunk[start] !== 10) { this.fail(new Error('session catalog returned an invalid worker frame')); return; }
      start++;
      this.response = undefined;
      let output: string;
      try { output = new TextDecoder('utf-8', { fatal: true }).decode(response.body); }
      catch { this.fail(new Error('session catalog returned invalid worker UTF-8')); return; }
      const current = this.current!;
      this.current = undefined;
      clearTimeout(current.timeout);
      if (response.kind === 'error') {
        let error: unknown;
        try { error = JSON.parse(output); } catch { /* Checked below. */ }
        if (typeof error !== 'string') {
          current.reject(new Error('session catalog returned an invalid worker error'));
          this.retiring = true;
        } else current.reject(new Error(error));
      } else current.resolve(output);
      if (this.retiring) this.stop();
      else {
        this.idleTimer = setTimeout(() => this.retire(), this.idleMs);
        this.idleTimer.unref();
        catalogChildren.markIdle(this.release, () => this.retire());
      }
    }
  }
}

interface CatalogWorkerLane {
  worker?: CatalogWorker;
  key?: string;
  pending?: Promise<CatalogWorker>;
  tail: Promise<void>;
}

function boundedOutput(value: string): string {
  if (Buffer.byteLength(value, 'utf8') > MAX_OUTPUT_BYTES) throw new Error('session catalog output is too large');
  return value;
}

function sessionRow(value: unknown): ProjectSessionCatalogRow | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  const row = value as Record<string, unknown>;
  const messageCount = row['message_count'];
  const resumeModel = row['resume_model'];
  const workspace = row['workspace_path'];
  const resumeTitle = row['resume_title'];
  if (
    !isSessionId(row['uuid'])
    || typeof row['title'] !== 'string'
    || typeof row['modified_rfc3339'] !== 'string'
    || typeof messageCount !== 'number'
    || !Number.isSafeInteger(messageCount)
    || (row['mode'] !== 'chat' && row['mode'] !== 'code')
    || typeof row['path'] !== 'string'
    || typeof row['empty_session'] !== 'boolean'
    || (resumeTitle !== undefined && (typeof resumeTitle !== 'string' || resumeTitle.includes('\0')))
    || (workspace !== undefined && (typeof workspace !== 'string' || !isAbsolute(workspace)
      || workspace.length > 32_768 || workspace.includes('\0')))
    || (resumeModel !== undefined && (
      typeof resumeModel !== 'string'
      || resumeModel.length === 0
      || resumeModel.length > 256
      || resumeModel.includes('\0')
    ))
  ) return null;
  return {
    uuid: row['uuid'],
    title: row['title'],
    modified_rfc3339: row['modified_rfc3339'],
    message_count: messageCount,
    mode: row['mode'],
    path: row['path'],
    empty_session: row['empty_session'],
    ...(typeof resumeModel === 'string' ? { resume_model: resumeModel } : {}),
    ...(typeof workspace === 'string' ? { workspace_path: workspace } : {}),
    ...(typeof resumeTitle === 'string' ? { resume_title: resumeTitle } : {}),
  };
}

function parseCatalog(output: string, maxSessions?: number): ProjectSessionCatalogResult {
  let parsed: unknown;
  try {
    parsed = JSON.parse(boundedOutput(output));
  } catch {
    throw new Error('session catalog returned invalid JSON');
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) throw new Error('session catalog returned an invalid payload');
  const input = parsed as Record<string, unknown>;
  if (input['version'] !== 1 || !Array.isArray(input['sessions'])) throw new Error('session catalog returned an unsupported payload');
  const sessions: ProjectSessionCatalogRow[] = [];
  for (const value of input['sessions']) {
    const row = sessionRow(value);
    if (!row) throw new Error('session catalog contains an invalid session row');
    sessions.push(row);
    if (maxSessions !== undefined && sessions.length >= maxSessions) break;
  }
  return { sessions };
}

/** Reads persisted sessions without assembling an engine or touching credentials. */
export class ProjectSessionCatalog {
  private readonly cache = new Map<string, CatalogCacheEntry>();
  private readonly pending = new Map<string, Promise<ProjectSessionCatalogResult>>();
  private readonly pendingSessions = new Map<string, Promise<ProjectSessionCatalogResult>>();
  private readonly pendingHistory = new Map<string, Promise<TranscriptHistoryPage>>();
  private nativeProductPaths?: Promise<ProductPaths>;
  private generation = 0;
  private readonly foregroundLane: CatalogWorkerLane = { tail: Promise.resolve() };
  private readonly historyLane: CatalogWorkerLane = { tail: Promise.resolve() };
  private catalogQueued = 0;
  private disposed = false;

  constructor(private readonly options: ProjectSessionCatalogOptions = {}) {}

  async captureMetadataSource(projectPath: string, sessionId: string, transcriptPath: string, sessionMode: 'chat' | 'code'): Promise<string> {
    if (!isSessionId(sessionId) || !isAbsolute(transcriptPath) || transcriptPath.includes('\0')) throw new Error('Invalid metadata source.');
    const result = await this.metadataRequest(projectPath, { action: 'capture', sessionId, transcriptPath, sessionMode });
    if (typeof result.receipt !== 'string' || !result.receipt || result.receipt.length > 128_000) throw new Error('Invalid metadata source receipt.');
    return result.receipt;
  }

  async mutateMetadataSource(projectPath: string, receipt: string, action: 'rename' | 'touch' | 'verify', title?: string): Promise<void> {
    const result = await this.metadataRequest(projectPath, { action, receipt, ...(title === undefined ? {} : { title }) });
    if (result.ok !== true) throw new Error('Invalid session metadata result.');
    if (action === 'rename' || action === 'touch') this.invalidate(projectPath);
  }

  private async metadataRequest(projectPath: string, input: Record<string, unknown>): Promise<Record<string, unknown>> {
    const args = ['--session-metadata-json', ...(this.options.configHome ? ['--session-config-home', this.options.configHome] : [])];
    const output = await this.runOutput(realpathSync.native(projectPath), args, `${JSON.stringify(input)}\n`,
      input.action !== 'rename' && input.action !== 'touch');
    const result: unknown = JSON.parse(boundedOutput(output));
    if (!result || typeof result !== 'object' || Array.isArray(result)) throw new Error('Invalid session metadata response.');
    return result as Record<string, unknown>;
  }

  async providerSettings(patch?: Record<string, unknown>, expected?: Record<string, unknown>): Promise<HostProviderSettings> {
    const output = await this.runOutput(realpathSync.native(homedir()), ['--provider-settings-command'], JSON.stringify({ patch, expected }));
    const result: unknown = JSON.parse(boundedOutput(output));
    if (!result || typeof result !== 'object' || Array.isArray(result)) throw new Error('Invalid provider settings response.');
    const value = result as Partial<HostProviderSettings>;
    if (!value.raw || typeof value.raw !== 'object' || Array.isArray(value.raw) || !value.effective || typeof value.effective !== 'object' || Array.isArray(value.effective) || !Array.isArray(value.locked) || value.locked.some(key => !['providers', 'routing', 'providerRegion'].includes(key))) throw new Error('Invalid provider settings response.');
    return value as HostProviderSettings;
  }

  /** Headless host account management uses the same canonical Rust registry. */
  async providerAccountCommand(request: ProviderAccountCommand, credential?: string, credentialPresence?: string[]): Promise<ProviderAccountSnapshot> {
    const { action, expectedRevision, accountId, displayName, providerId, accountIds, enabled, authKind, executionBindingId, allowSubscriptionToMetered } = request;
    if (credential !== undefined && action !== 'test') throw new Error('Account metadata commands do not accept credentials.');
    if (credentialPresence !== undefined && action !== 'list') throw new Error('Credential presence is only available for account listing.');
    // Rust's tagged action enum rejects irrelevant fields; retain the exact
    // narrow command shape at this boundary.
    const command = action === 'list' ? { action, expectedRevision }
      : action === 'add' ? { action, expectedRevision, accountId, providerId, displayName, authKind, executionBindingId }
      : action === 'set_policy' ? { action, expectedRevision, providerId, allowSubscriptionToMetered }
      : action === 'update' ? { action, expectedRevision, accountId, displayName }
      : action === 'reorder' ? { action, expectedRevision, providerId, accountIds }
      : action === 'enable' ? { action: 'set_enabled', expectedRevision, accountId, enabled }
      : { action, expectedRevision, accountId };
    const output = await this.runOutput(realpathSync.native(homedir()), ['--provider-account-command'],
      JSON.stringify({ ...(credentialPresence ? { credential_presence: credentialPresence } : {}), ...(this.options.appVersion !== undefined ? { app_version: this.options.appVersion } : {}), request_json: JSON.stringify(command), metadata_only: true, backend: 'desktop-broker', credential_channel: this.options.providerAccountChannel ?? 'development', ...(credential ? { credential } : {}) }));
    const result = JSON.parse(boundedOutput(output));
    if (result.error) throw new Error('Provider account operation failed.');
    const snapshot = typeof result.snapshot_json === 'string' ? JSON.parse(result.snapshot_json) : result;
    if (!snapshot || typeof snapshot.storeScope !== 'string' || !Number.isSafeInteger(snapshot.configRevision)
      || !Array.isArray(snapshot.accounts) || !Array.isArray(snapshot.health)) throw new Error('Invalid provider account snapshot.');
    return snapshot as ProviderAccountSnapshot;
  }

  /** Private authority commit: never exposed as an account-management command. */
  async publishProviderAccountCredentialVersion(version: ProviderAccountCredentialVersion): Promise<void> {
    if (!/^[a-f0-9-]{36}$/.test(version.accountId) || !Number.isSafeInteger(version.identityGeneration) || version.identityGeneration < 1
      || !Number.isSafeInteger(version.credentialRevision) || version.credentialRevision < 1) throw new Error('Invalid credential version.');
    const output = await this.runOutput(realpathSync.native(homedir()), ['--provider-account-command'], JSON.stringify({
      metadata_only: true, backend: 'desktop-broker', credential_channel: this.options.providerAccountChannel ?? 'development',
      request_json: '', publish_credential_version: { accountId: version.accountId, identityGeneration: version.identityGeneration, credentialRevision: version.credentialRevision },
    }));
    const result = JSON.parse(boundedOutput(output));
    if (!result || result.published !== true) throw new Error('Credential version could not be committed.');
  }

  /** Product directory discovery must work even if workspace settings are broken. */
  async productPaths(): Promise<ProductPaths> {
    if (!this.nativeProductPaths) {
      const operation = this.discoverProductPaths().catch(error => {
        if (this.nativeProductPaths === operation) this.nativeProductPaths = undefined;
        throw error;
      });
      this.nativeProductPaths = operation;
    }
    return { ...await this.nativeProductPaths };
  }

  private async discoverProductPaths(): Promise<ProductPaths> {
    const output = await this.runOutput(realpathSync.native(homedir()), ['--product-paths-json']);
    let value: unknown;
    try { value = JSON.parse(boundedOutput(output)); }
    catch { throw new Error('invalid product paths response'); }
    if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('invalid product paths response');
    const input = value as Record<string, unknown>;
    if (input['version'] !== 2 || [input['brand_home'], input['workspace_root'], input['app_projects_root']].some(path =>
      typeof path !== 'string' || !isAbsolute(path) || path.includes('\0') || path.length > 32_768)
      || (input['global_config_path'] !== null && (typeof input['global_config_path'] !== 'string'
        || !isAbsolute(input['global_config_path']) || input['global_config_path'].includes('\0') || input['global_config_path'].length > 32_768))) {
      throw new Error('invalid product paths response');
    }
    return { brandHome: input['brand_home'] as string, workspaceRoot: input['workspace_root'] as string, appProjectsRoot: input['app_projects_root'] as string,
      ...(typeof input['global_config_path'] === 'string' ? { globalConfigPath: input['global_config_path'] } : {}) };
  }

  /** Read settings with the engine's merge rules before starting its process. */
  async workspaceSettings(projectPath: string): Promise<WorkspaceLaunchSettings> {
    const canonical = realpathSync.native(projectPath);
    const output = await this.runOutput(canonical, ['--cwd', canonical, '--workspace-environment-json']);
    const input = JSON.parse(boundedOutput(output)) as { version?: unknown; environment?: unknown; config_home?: unknown; global_config_path?: unknown };
    if (input?.version !== 1 || !input.environment || typeof input.environment !== 'object' || Array.isArray(input.environment)) {
      throw new Error('invalid workspace environment response');
    }
    const entries = Object.entries(input.environment);
    if (entries.length > 4096 || entries.some(([key, value]) => !key || /[=\0]/.test(key)
      || typeof value !== 'string' || value.includes('\0'))) throw new Error('invalid workspace environment entry');
    if (typeof input.config_home !== 'string' || !isAbsolute(input.config_home) || input.config_home.includes('\0')
      || (input.global_config_path !== null && input.global_config_path !== undefined && (typeof input.global_config_path !== 'string'
        || !isAbsolute(input.global_config_path) || input.global_config_path.includes('\0')))) throw new Error('invalid workspace configuration owner');
    return { environment: Object.fromEntries(entries) as Record<string, string>, configHome: input.config_home,
      ...(typeof input.global_config_path === 'string' ? { globalConfigPath: input.global_config_path } : {}) };
  }

  async list(
    projectPath: string,
    scope: SessionCatalogScope = 'current',
    include: (session: ProjectSessionCatalogRow) => boolean = () => true,
    limitKey: (session: ProjectSessionCatalogRow) => string = (session) => session.mode,
  ): Promise<ProjectSessionCatalogResult> {
    const result = await this.load(projectPath, sessionCatalogScope(scope));
    const count = new Map<string, number>();
    return {
      // Exclude hidden/archived rows before applying a separate bound to each
      // space. A busy Code space must not hide older Chat history (or vice versa).
      sessions: result.sessions.filter((session) => {
        if (!include(session)) return false;
        if (scope !== 'current') return true;
        const key = limitKey(session), previous = count.get(key) ?? 0;
        count.set(key, previous + 1);
        return previous < MAX_SESSIONS;
      })
        .map((session) => ({ ...session })),
    };
  }

  /** Complete authenticated catalog for persistent space membership, before UI truncation. */
  async listComplete(projectPath: string, scope: SessionCatalogScope = 'current'): Promise<ProjectSessionCatalogResult> {
    const result = await this.load(projectPath, sessionCatalogScope(scope));
    return { sessions: result.sessions.map((session) => ({ ...session })) };
  }

  /**
   * Resolve one persisted session without applying the UI list limit.
   *
   * The catalog process already returns the complete persisted session set;
   * only `list()` is bounded for sidebar rendering. Keeping this lookup
   * separate prevents an old or pinned session from being mistaken for a
   * foreign session merely because it falls after the first 200 rows.
   */
  async find(projectPath: string, sessionId: string): Promise<ProjectSessionCatalogRow | undefined> {
    if (!isSessionId(sessionId)) throw new Error('invalid session id');
    sessionId = sessionId.toLowerCase();
    const canonical = realpathSync.native(projectPath);
    const cached = this.cache.get(`${canonical}\0current`);
    // Every session open waits on this lookup. Without a fresh catalog, ask
    // for this one row: the full catalog parses every transcript in the
    // Project, most of a second once one of them is tens of megabytes.
    const match = cached && cached.expiresAt > Date.now()
      ? cached.result.sessions.find((candidate) => candidate.uuid === sessionId) : undefined;
    if (match) return { ...match };
    const result = await this.loadSession(canonical, sessionId);
    const session = result.sessions.find((candidate) => candidate.uuid === sessionId);
    return session ? { ...session } : undefined;
  }

  /** A picker selection must retain its displayed scope and exact file. */
  async select(projectPath: string, sessionId: string, selection: SessionOpenSelection): Promise<ProjectSessionCatalogRow | undefined> {
    if (!isSessionId(sessionId)) throw new Error('invalid session id');
    sessionId = sessionId.toLowerCase();
    if (selection.transcriptPath !== undefined && (!isAbsolute(selection.transcriptPath)
      || selection.transcriptPath.includes('\0') || basename(selection.transcriptPath).toLowerCase() !== `${sessionId}.jsonl`)) {
      throw new Error('invalid selected transcript path');
    }
    const canonical = realpathSync.native(projectPath);
    const result = await this.run(canonical, sessionId, sessionCatalogScope(selection.scope), selection.transcriptPath);
    const row = result.sessions.find((candidate) => candidate.uuid === sessionId);
    if (row && selection.transcriptPath !== undefined && row.path !== selection.transcriptPath) {
      throw new Error('This session selection has changed. Refresh the session list and try again.');
    }
    return row ? { ...row } : undefined;
  }

  async readHistory(projectPath: string, sessionId: string, transcriptPath: string, before: TranscriptHistoryCursor): Promise<TranscriptHistoryPage> {
    if (!isSessionId(sessionId) || !isAbsolute(transcriptPath) || transcriptPath.includes('\0')
      || basename(transcriptPath).toLowerCase() !== `${sessionId.toLowerCase()}.jsonl`) throw new Error('Invalid history source.');
    const query = { cwd: realpathSync.native(projectPath), sessionId: sessionId.toLowerCase(), transcriptPath, historyBefore: historyCursor(before) };
    const key = JSON.stringify(query);
    const existing = this.pendingHistory.get(key);
    if (existing) return existing;
    const operation = this.loadHistory(query).finally(() => {
      if (this.pendingHistory.get(key) === operation) this.pendingHistory.delete(key);
    });
    this.pendingHistory.set(key, operation);
    return operation;
  }

  private async loadHistory(query: CatalogWorkerQuery & { sessionId: string; historyBefore: TranscriptHistoryCursor }): Promise<TranscriptHistoryPage> {
    const output = await this.runWorker(query, this.historyLane);
    const page = JSON.parse(output) as TranscriptHistoryPage;
    if (!page || typeof page !== 'object' || Array.isArray(page)
      || Object.keys(page).some(key => !['messages', 'before'].includes(key)) || !Array.isArray(page.messages)) throw new Error('Invalid history page');
    const sessionId = query.sessionId, before = query.historyBefore;
    const event = validateClientEvent({ type: 'session_resumed', mode: 'code', session_id: sessionId, messages: page.messages });
    if (event.type !== 'session_resumed') throw new Error('Invalid history page');
    const next = page.before == null ? null : historyCursor(page.before);
    if (next && (next.uuid === before.uuid || next.offset == null
      || (before.offset != null && next.offset >= before.offset))) throw new Error('History cursor did not advance');
    return { messages: event.messages, before: next };
  }

  /** Invalidate after a durable transcript/metadata mutation. */
  invalidate(projectPath?: string): void {
    this.generation++;
    // An alias may refer to the mutated transcript, so all discovery scopes
    // expire together. Pending readers may finish, but cannot repopulate them.
    this.cache.clear();
    this.pending.clear();
    this.pendingSessions.clear();
    this.pendingHistory.clear();
    // Retire after the current captured response; a mutation must never reuse
    // projections from before its durable revision/generation boundary.
    for (const lane of [this.foregroundLane, this.historyLane]) {
      lane.worker?.retire();
      lane.worker = undefined;
      lane.key = undefined;
    }
    void projectPath;
  }

  /** Release metadata resources when the owning Host is disposed. */
  dispose(): void { this.disposed = true; this.invalidate(); }

  private load(projectPath: string, scope: SessionCatalogScope): Promise<ProjectSessionCatalogResult> {
    const canonical = realpathSync.native(projectPath);
    const key = `${canonical}\0${scope}`;
    const generation = this.generation;
    const cached = this.cache.get(key);
    if (cached && cached.expiresAt > Date.now()) return Promise.resolve(cached.result);
    if (cached) this.cache.delete(key);
    const existing = this.pending.get(key);
    if (existing) return existing;
    const operation = this.run(canonical, undefined, scope)
      .then((result) => {
        if (this.generation === generation) this.cache.set(key, {
          result,
          expiresAt: Date.now() + (this.options.cacheTtlMs ?? DEFAULT_CACHE_TTL_MS),
        });
        return result;
      })
      .finally(() => {
        if (this.pending.get(key) === operation) this.pending.delete(key);
      });
    this.pending.set(key, operation);
    return operation;
  }

  private loadSession(canonical: string, sessionId: string): Promise<ProjectSessionCatalogResult> {
    const key = `${canonical}\0${sessionId}`;
    const existing = this.pendingSessions.get(key);
    if (existing) return existing;
    const operation = this.run(canonical, sessionId).finally(() => {
      if (this.pendingSessions.get(key) === operation) this.pendingSessions.delete(key);
    });
    this.pendingSessions.set(key, operation);
    return operation;
  }

  private run(canonical: string, sessionId?: string, scope?: SessionCatalogScope, transcriptPath?: string): Promise<ProjectSessionCatalogResult> {
    if (this.disposed) return Promise.reject(new Error('session catalog is disposed'));
    if (this.options.catalogWorker !== false) {
      return this.runWorker({ cwd: canonical, ...(sessionId ? { sessionId } : {}),
        ...(scope ? { scope } : {}), ...(transcriptPath ? { transcriptPath } : {}) }).then(parseCatalog);
    }
    const args = buildBridgeArguments({
      workspace: canonical,
      bridgeDir: '',
      listSessionsJson: true,
      sessionScope: scope,
      ...(sessionId ? { sessionId } : {}),
      ...(transcriptPath ? { sessionTranscriptPath: transcriptPath } : {}),
      trusted: false,
    });
    return this.runOutput(canonical, args).then((output) => parseCatalog(output));
  }

  private runWorker(query: CatalogWorkerQuery, lane = this.foregroundLane): Promise<string> {
    if (this.disposed) return Promise.reject(new Error('session catalog is disposed'));
    if (this.catalogQueued >= MAX_QUEUED_CATALOG_OPERATIONS) return Promise.reject(new Error('session catalog is busy; try again shortly'));
    this.catalogQueued++;
    const operation = lane.tail.then(async () => {
      if (this.disposed) throw new Error('session catalog is disposed');
      const worker = await this.getWorker(query.cwd, lane);
      try {
        return await worker.request(query);
      } catch (error) {
        worker.retire();
        if (lane.worker === worker) lane.worker = undefined;
        throw error;
      }
    });
    lane.tail = operation.then(() => undefined, () => undefined);
    return operation.finally(() => { this.catalogQueued--; });
  }

  private getWorker(canonical: string, lane: CatalogWorkerLane): Promise<CatalogWorker> {
    if (this.disposed) return Promise.reject(new Error('session catalog is disposed'));
    const bin = resolveServerBin({ serverBin: this.options.serverBin, isPackaged: this.options.isPackaged,
      resourcesPath: this.options.resourcesPath } as Omit<BridgeManagerOptions, 'launchConfig'>);
    const env = { ...buildBridgeEnvironment(process.env), ...(this.options.configHome ? { LINGXI_CONFIG_DIR: this.options.configHome } : {}) };
    const key = JSON.stringify([bin, Object.entries(env).sort(([left], [right]) => left.localeCompare(right))]);
    if (lane.worker?.available && lane.key === key) return Promise.resolve(lane.worker);
    if (lane.pending) return lane.pending;
    lane.worker?.retire();
    lane.worker = undefined;
    let generation = this.generation;
    const operation: Promise<CatalogWorker> = catalogChildren.start((release) => {
      if (this.disposed) { release(); throw new Error('session catalog is disposed'); }
      generation = this.generation;
      const child = (this.options.spawnProcess ?? spawn)(bin, ['--session-catalog-worker-json'], {
        cwd: canonical, env, stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true,
      });
      return new CatalogWorker(child, release, this.options.timeoutMs ?? 15_000, this.options.workerIdleMs ?? DEFAULT_WORKER_IDLE_MS);
    }, this.options.timeoutMs).then((worker) => {
      if (generation !== this.generation) {
        worker.retire();
        if (lane.pending === operation) lane.pending = undefined;
        return this.getWorker(canonical, lane);
      }
      lane.worker = worker;
      lane.key = key;
      return worker;
    }).finally(() => { if (lane.pending === operation) lane.pending = undefined; });
    lane.pending = operation;
    return operation;
  }

  private runOutput(canonical: string, args: string[], input?: string, killOnTimeout = true): Promise<string> {
    const bin = resolveServerBin({
      serverBin: this.options.serverBin,
      isPackaged: this.options.isPackaged,
      resourcesPath: this.options.resourcesPath,
    } as Omit<BridgeManagerOptions, 'launchConfig'>);
    return catalogChildren.start((release) => new Promise<string>((resolve, reject) => {
      if (this.disposed) { release(); reject(new Error('session catalog is disposed')); return; }
      let child: ReturnType<typeof spawn>;
      try {
        child = (this.options.spawnProcess ?? spawn)(bin, args, {
          cwd: canonical,
          env: { ...buildBridgeEnvironment(process.env), ...(this.options.configHome ? { LINGXI_CONFIG_DIR: this.options.configHome } : {}) },
          stdio: [input === undefined ? 'ignore' : 'pipe', 'pipe', 'pipe'],
          windowsHide: true,
          detached: !killOnTimeout,
        });
      } catch (error) { release(); reject(error); return; }
      const termination = new CatalogChildTermination(child, release);
      if (input !== undefined) {
        child.stdin?.on('error', () => { /* Process close carries the bounded failure. */ });
        child.stdin?.end(input);
      }
      const stdout: Buffer[] = [];
      let stdoutBytes = 0;
      const stderr: Buffer[] = [];
      let stderrBytes = 0;
      let settled = false;
      const timeout = setTimeout(() => {
        if (settled) return;
        settled = true;
        if (killOnTimeout) termination.stop(); else child.unref();
        reject(new Error(killOnTimeout ? 'session catalog timed out' : 'The metadata result is unconfirmed; its writer is still completing.'));
      }, this.options.timeoutMs ?? 15_000);
      timeout.unref();
      child.stdout!.on('data', (chunk: Buffer | string) => {
        if (settled) return;
        const bytes = typeof chunk === 'string' ? Buffer.from(chunk) : chunk;
        stdoutBytes += bytes.length;
        if (stdoutBytes > MAX_OUTPUT_BYTES) {
          settled = true;
          clearTimeout(timeout);
          if (killOnTimeout) termination.stop(); else child.unref();
          reject(new Error(killOnTimeout ? 'session catalog output is too large' : 'The metadata result is unconfirmed after oversized output.'));
          return;
        }
        stdout.push(bytes);
      });
      child.stderr!.on('data', (chunk: Buffer | string) => {
        if (settled) return;
        const bytes = typeof chunk === 'string' ? Buffer.from(chunk) : chunk;
        // Keep the same diagnostic tail as before, bounded in raw bytes.
        const fragment = Buffer.from(bytes.subarray(Math.max(0, bytes.length - 16 * 1024)));
        stderr.push(fragment);
        stderrBytes += fragment.length;
        while (stderrBytes > 16 * 1024) {
          const first = stderr[0];
          const removed = Math.min(first.length, stderrBytes - 16 * 1024);
          if (removed === first.length) stderr.shift();
          else stderr[0] = first.subarray(removed);
          stderrBytes -= removed;
        }
      });
      child.on('error', (error) => {
        termination.error();
        if (settled) return;
        settled = true;
        clearTimeout(timeout);
        if (killOnTimeout) termination.stop();
        reject(new Error(`session catalog failed to start: ${error.message}`));
      });
      child.once('close', (code, signal) => {
        release();
        if (settled) return;
        settled = true;
        clearTimeout(timeout);
        if (code !== 0) {
          const diagnosticBytes = Buffer.concat(stderr, stderrBytes);
          let firstCodepoint = 0;
          // A bounded tail can begin inside a codepoint; omit only those
          // continuation bytes rather than manufacturing replacement text.
          while (firstCodepoint < Math.min(3, diagnosticBytes.length)
            && (diagnosticBytes[firstCodepoint] & 0xc0) === 0x80) firstCodepoint++;
          const diagnostic = diagnosticBytes.toString('utf8', firstCodepoint).trim();
          reject(new Error(`session catalog failed (code=${code ?? 'null'}, signal=${signal ?? 'null'}): ${diagnostic || 'unknown error'}`));
          return;
        }
        try {
          resolve(Buffer.concat(stdout, stdoutBytes).toString('utf8'));
        } catch (error) {
          reject(error instanceof Error ? error : new Error(String(error)));
        }
      });
    }), this.options.timeoutMs);
  }
}
