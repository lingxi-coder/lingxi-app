import { spawn } from 'node:child_process';
import { realpathSync } from 'node:fs';
import type { SessionRowDto } from '@lingxi/bridge-client';

import { buildBridgeArguments, buildBridgeEnvironment } from './host-utils.js';
import { isSessionId, resolveServerBin, type BridgeManagerOptions } from './bridge.js';

const MAX_OUTPUT_BYTES = 8 * 1024 * 1024;
const MAX_SESSIONS = 200;

export interface ProjectSessionCatalogOptions {
  serverBin?: string;
  isPackaged?: boolean;
  resourcesPath?: string;
  timeoutMs?: number;
  spawnProcess?: typeof spawn;
}

export interface ProjectSessionCatalogResult {
  sessions: ProjectSessionCatalogRow[];
}

export interface ProjectSessionCatalogRow extends SessionRowDto {
  /** Private bridge-server catalog marker; never crosses into renderer state. */
  empty_session: boolean;
}

function boundedOutput(value: string): string {
  if (Buffer.byteLength(value, 'utf8') > MAX_OUTPUT_BYTES) throw new Error('session catalog output is too large');
  return value;
}

function sessionRow(value: unknown): ProjectSessionCatalogRow | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  const row = value as Record<string, unknown>;
  const messageCount = row['message_count'];
  if (
    !isSessionId(row['uuid'])
    || typeof row['title'] !== 'string'
    || typeof row['modified_rfc3339'] !== 'string'
    || typeof messageCount !== 'number'
    || !Number.isSafeInteger(messageCount)
    || typeof row['path'] !== 'string'
    || typeof row['empty_session'] !== 'boolean'
  ) return null;
  return {
    uuid: row['uuid'],
    title: row['title'],
    modified_rfc3339: row['modified_rfc3339'],
    message_count: messageCount,
    mode: row['mode'] === 'chat' ? 'chat' : 'code',
    path: row['path'],
    empty_session: row['empty_session'],
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
  constructor(private readonly options: ProjectSessionCatalogOptions = {}) {}

  list(projectPath: string): Promise<ProjectSessionCatalogResult> {
    return this.run(projectPath, MAX_SESSIONS);
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
    const result = await this.run(projectPath);
    return result.sessions.find((session) => session.uuid === sessionId);
  }

  private run(projectPath: string, maxSessions?: number): Promise<ProjectSessionCatalogResult> {
    const canonical = realpathSync.native(projectPath);
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
          resolve(parseCatalog(stdout, maxSessions));
        } catch (error) {
          reject(error instanceof Error ? error : new Error(String(error)));
        }
      });
    });
  }
}
