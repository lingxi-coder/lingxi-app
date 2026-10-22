import { spawn } from 'node:child_process';
import { realpathSync } from 'node:fs';
import { isAbsolute } from 'node:path';
import type { SessionRowDto } from '@lingxi/bridge-client';

import { buildBridgeArguments, buildBridgeEnvironment } from './host-utils.js';
import { resolveServerBin } from './bridgeDiscovery.js';
import type { BridgeManagerOptions, WorkspaceLaunchSettings } from './bridgeTypes.js';
import { isSessionId } from './sessionIdentity.js';
import { sessionCatalogScope, type SessionCatalogScope, type SessionOpenSelection } from '../shared/sessionWorkspace.js';

const MAX_OUTPUT_BYTES = 8 * 1024 * 1024;
const MAX_SESSIONS = 200;
const DEFAULT_CACHE_TTL_MS = 15_000;

export interface ProjectSessionCatalogOptions {
  serverBin?: string;
  isPackaged?: boolean;
  resourcesPath?: string;
  timeoutMs?: number;
  /** Keep project metadata warm while session/runtime work is in flight. */
  cacheTtlMs?: number;
  spawnProcess?: typeof spawn;
}

export interface ProjectSessionCatalogResult {
  sessions: ProjectSessionCatalogRow[];
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
    mode: row['mode'] === 'chat' ? 'chat' : 'code',
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
  private generation = 0;

  constructor(private readonly options: ProjectSessionCatalogOptions = {}) {}

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

  async list(projectPath: string, scope: SessionCatalogScope = 'current'): Promise<ProjectSessionCatalogResult> {
    const result = await this.load(projectPath, sessionCatalogScope(scope));
    return {
      // Never hand callers the cached array itself: restore/archive code filters
      // its local result and must not mutate the shared project snapshot.
      sessions: result.sessions.slice(0, scope === 'current' ? MAX_SESSIONS : undefined).map((session) => ({ ...session })),
    };
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
    const canonical = realpathSync.native(projectPath);
    const cached = this.cache.get(`${canonical}\0current`);
    // Every session open waits on this lookup. Without a fresh catalog, ask
    // for this one row: the full catalog parses every transcript in the
    // Project, most of a second once one of them is tens of megabytes. A
    // runtime that predates the narrowed mode ignores the filter and returns
    // the full catalog, which the search below handles the same way.
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
    const canonical = realpathSync.native(projectPath);
    const result = await this.run(canonical, sessionId, sessionCatalogScope(selection.scope));
    const row = result.sessions.find((candidate) => candidate.uuid === sessionId);
    if (row && selection.transcriptPath !== undefined && row.path !== selection.transcriptPath) {
      throw new Error('This session selection has changed. Refresh the session list and try again.');
    }
    return row ? { ...row } : undefined;
  }

  /** Invalidate after a durable transcript/metadata mutation. */
  invalidate(projectPath?: string): void {
    this.generation++;
    // An alias may refer to the mutated transcript, so all discovery scopes
    // expire together. Pending readers may finish, but cannot repopulate them.
    this.cache.clear();
    this.pending.clear();
    this.pendingSessions.clear();
    void projectPath;
  }

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

  private run(canonical: string, sessionId?: string, scope?: SessionCatalogScope): Promise<ProjectSessionCatalogResult> {
    const args = buildBridgeArguments({
      workspace: canonical,
      bridgeDir: '',
      listSessionsJson: true,
      sessionScope: scope,
      ...(sessionId ? { sessionId } : {}),
      hasApiKey: false,
      trusted: false,
    });
    return this.runOutput(canonical, args).then((output) => parseCatalog(output));
  }

  private runOutput(canonical: string, args: string[]): Promise<string> {
    const bin = resolveServerBin({
      serverBin: this.options.serverBin,
      isPackaged: this.options.isPackaged,
      resourcesPath: this.options.resourcesPath,
    } as Omit<BridgeManagerOptions, 'launchConfig'>);
    return new Promise<string>((resolve, reject) => {
      const child = (this.options.spawnProcess ?? spawn)(bin, args, {
        cwd: canonical,
        env: buildBridgeEnvironment(process.env),
        stdio: ['ignore', 'pipe', 'pipe'],
        windowsHide: true,
      });
      const stdout: Buffer[] = [];
      let stdoutBytes = 0;
      const stderr: Buffer[] = [];
      let stderrBytes = 0;
      let settled = false;
      const timeout = setTimeout(() => {
        if (settled) return;
        settled = true;
        child.kill();
        reject(new Error('session catalog timed out'));
      }, this.options.timeoutMs ?? 15_000);
      timeout.unref();
      child.stdout.on('data', (chunk: Buffer | string) => {
        if (settled) return;
        const bytes = typeof chunk === 'string' ? Buffer.from(chunk) : chunk;
        stdoutBytes += bytes.length;
        if (stdoutBytes > MAX_OUTPUT_BYTES) {
          settled = true;
          clearTimeout(timeout);
          child.kill();
          reject(new Error('session catalog output is too large'));
          return;
        }
        stdout.push(bytes);
      });
      child.stderr.on('data', (chunk: Buffer | string) => {
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
      child.once('error', (error) => {
        if (settled) return;
        settled = true;
        clearTimeout(timeout);
        reject(new Error(`session catalog failed to start: ${error.message}`));
      });
      child.once('close', (code, signal) => {
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
    });
  }
}
