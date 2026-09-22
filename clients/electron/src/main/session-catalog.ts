import { spawn } from 'node:child_process';
import { realpathSync } from 'node:fs';
import type { SessionRowDto } from '@lingxi/bridge-client';

import { buildBridgeArguments, buildBridgeEnvironment } from './host-utils.js';
import { isSessionId, resolveServerBin, type BridgeManagerOptions } from './bridge.js';

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
  if (
    !isSessionId(row['uuid'])
    || typeof row['title'] !== 'string'
    || typeof row['modified_rfc3339'] !== 'string'
    || typeof messageCount !== 'number'
    || !Number.isSafeInteger(messageCount)
    || typeof row['path'] !== 'string'
    || typeof row['empty_session'] !== 'boolean'
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

  constructor(private readonly options: ProjectSessionCatalogOptions = {}) {}

  async list(projectPath: string): Promise<ProjectSessionCatalogResult> {
    const result = await this.load(projectPath);
    return {
      // Never hand callers the cached array itself: restore/archive code filters
      // its local result and must not mutate the shared project snapshot.
      sessions: result.sessions.slice(0, MAX_SESSIONS).map((session) => ({ ...session })),
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
    const result = await this.load(projectPath);
    const session = result.sessions.find((candidate) => candidate.uuid === sessionId);
    return session ? { ...session } : undefined;
  }

  /** Invalidate after a durable transcript/metadata mutation. */
  invalidate(projectPath?: string): void {
    if (!projectPath) {
      this.cache.clear();
      return;
    }
    try { this.cache.delete(realpathSync.native(projectPath)); } catch { this.cache.delete(projectPath); }
  }

  private load(projectPath: string): Promise<ProjectSessionCatalogResult> {
    const canonical = realpathSync.native(projectPath);
    const cached = this.cache.get(canonical);
    if (cached && cached.expiresAt > Date.now()) return Promise.resolve(cached.result);
    if (cached) this.cache.delete(canonical);
    const existing = this.pending.get(canonical);
    if (existing) return existing;
    const operation = this.run(canonical)
      .then((result) => {
        this.cache.set(canonical, {
          result,
          expiresAt: Date.now() + (this.options.cacheTtlMs ?? DEFAULT_CACHE_TTL_MS),
        });
        return result;
      })
      .finally(() => {
        if (this.pending.get(canonical) === operation) this.pending.delete(canonical);
      });
    this.pending.set(canonical, operation);
    return operation;
  }

  private run(canonical: string): Promise<ProjectSessionCatalogResult> {
    const args = buildBridgeArguments({
      workspace: canonical,
      bridgeDir: '',
      listSessionsJson: true,
      hasApiKey: false,
      trusted: false,
    });
    const bin = resolveServerBin({
      serverBin: this.options.serverBin,
      isPackaged: this.options.isPackaged,
      resourcesPath: this.options.resourcesPath,
    } as Omit<BridgeManagerOptions, 'launchConfig'>);
    return new Promise<ProjectSessionCatalogResult>((resolve, reject) => {
      const child = (this.options.spawnProcess ?? spawn)(bin, args, {
        cwd: canonical,
        env: buildBridgeEnvironment(process.env),
        stdio: ['ignore', 'pipe', 'pipe'],
        windowsHide: true,
      });
      let stdout = '';
      let stderr = '';
      let settled = false;
      const timeout = setTimeout(() => {
        if (settled) return;
        settled = true;
        child.kill();
        reject(new Error('session catalog timed out'));
      }, this.options.timeoutMs ?? 15_000);
      timeout.unref();
      child.stdout.on('data', (chunk: Buffer | string) => {
        stdout += chunk.toString();
        if (Buffer.byteLength(stdout, 'utf8') > MAX_OUTPUT_BYTES) child.kill();
      });
      child.stderr.on('data', (chunk: Buffer | string) => {
        stderr += chunk.toString();
        if (Buffer.byteLength(stderr, 'utf8') > 16 * 1024) stderr = stderr.slice(-16 * 1024);
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
          reject(new Error(`session catalog failed (code=${code ?? 'null'}, signal=${signal ?? 'null'}): ${stderr.trim() || 'unknown error'}`));
          return;
        }
        try {
          resolve(parseCatalog(stdout));
        } catch (error) {
          reject(error instanceof Error ? error : new Error(String(error)));
        }
      });
    });
  }
}
