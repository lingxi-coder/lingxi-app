import { createHash, randomUUID } from 'node:crypto';
import { promises as fs } from 'node:fs';
import { basename, dirname, isAbsolute, join } from 'node:path';
import type { LocalAppNewSessionIntent } from '../../shared/localApps.js';
import type { SessionRef } from '../../shared/settings.js';

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const APP_ID = /^a[a-f0-9]{32}$/;
export interface AuthoringSessionIntent extends LocalAppNewSessionIntent {
  requestId: string;
  projectPath: string;
  sessionId: string;
  state: 'pending' | 'ready' | 'completed';
  verifiedInputSha256: string;
  source?: AuthoringSessionSource;
  sourceSha256?: string;
}
export interface AuthoringSessionSource { scope: 'current' | 'repository' | 'all'; transcriptPath: string }
function validPath(value: unknown): value is string {
  return typeof value === 'string' && isAbsolute(value) && !value.includes('\0')
    && !value.split(/[\\/]/).some(part => part === '.' || part === '..');
}
export function validateNewAuthoringIntent(value: unknown): LocalAppNewSessionIntent {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid Local App session intent');
  const input = value as Record<string, unknown>;
  if (Object.keys(input).some(key => !['name', 'appId', 'setCurrent', 'sourceRequestId'].includes(key))
    || typeof input.name !== 'string' || input.name.length > 256 || input.name.includes('\0')
    || typeof input.setCurrent !== 'boolean'
    || (input.appId !== undefined && (typeof input.appId !== 'string' || !APP_ID.test(input.appId)))
    || (input.sourceRequestId !== undefined && (typeof input.sourceRequestId !== 'string' || !UUID.test(input.sourceRequestId)))
    || (input.appId === undefined && (input.setCurrent || input.sourceRequestId !== undefined))) {
    throw new Error('Invalid Local App session intent');
  }
  return { name: input.name, setCurrent: input.setCurrent,
    ...(typeof input.appId === 'string' ? { appId: input.appId } : {}),
    ...(typeof input.sourceRequestId === 'string' ? { sourceRequestId: input.sourceRequestId } : {}) };
}
function digest(input: LocalAppNewSessionIntent & { projectPath: string; sessionId: string }): string {
  return createHash('sha256').update(JSON.stringify({ name: input.name, appId: input.appId ?? null,
    setCurrent: input.setCurrent, sourceRequestId: input.sourceRequestId ?? null,
    projectPath: input.projectPath, sessionId: input.sessionId })).digest('hex');
}
function validSource(source: AuthoringSessionSource, sessionId: string): boolean {
  return Boolean(source && typeof source === 'object' && !Array.isArray(source)
    && Object.keys(source).every(key => key === 'scope' || key === 'transcriptPath')
    && ['current', 'repository', 'all'].includes(source.scope) && validPath(source.transcriptPath)
    && basename(source.transcriptPath).toLowerCase() === `${sessionId.toLowerCase()}.jsonl`);
}
function sourceDigest(source: AuthoringSessionSource): string {
  return createHash('sha256').update(JSON.stringify({ scope: source.scope, transcriptPath: source.transcriptPath })).digest('hex');
}

/** Recovery intents identify an operation; only native source receipts authorize it. */
export class AuthoringSessionIntents {
  private readonly path: string;
  private readonly entries = new Map<string, AuthoringSessionIntent>();
  private loaded = false;
  private tail: Promise<void> = Promise.resolve();
  constructor(root: string) { this.path = join(root, 'authoring-session-intents.v1.json'); }
  private async locked<T>(operation: () => Promise<T>): Promise<T> {
    const prior = this.tail; let release!: () => void;
    this.tail = new Promise(resolve => { release = resolve; });
    await prior;
    try { return await operation(); } finally { release(); }
  }
  private async load(): Promise<void> {
    if (this.loaded) return;
    let content: string;
    try {
      const stat = await fs.lstat(this.path);
      if (!stat.isFile() || stat.nlink !== 1 || stat.size > 4 * 1024 * 1024) throw new Error('invalid intent file');
      content = await fs.readFile(this.path, 'utf8');
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') { this.loaded = true; return; }
      throw new Error('Could not read Local App session intents; refusing to overwrite them', { cause: error });
    }
    let parsed: unknown;
    try { parsed = JSON.parse(content); } catch (cause) { throw new Error('Invalid Local App session intents', { cause }); }
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)
      || Object.keys(parsed).some(key => !['version', 'entries'].includes(key))
      || (parsed as { version?: unknown }).version !== 1 || !Array.isArray((parsed as { entries?: unknown }).entries)) {
      throw new Error('Invalid Local App session intents; refusing to overwrite them');
    }
    const entries = new Map<string, AuthoringSessionIntent>();
    for (const value of (parsed as { entries: unknown[] }).entries) {
      if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid Local App session intent');
      const row = value as AuthoringSessionIntent;
      if (Object.keys(row).some(key => !['name', 'appId', 'setCurrent', 'sourceRequestId', 'requestId', 'projectPath', 'sessionId', 'state', 'verifiedInputSha256', 'source', 'sourceSha256'].includes(key))
        || !validPath(row.projectPath) || typeof row.sessionId !== 'string' || !UUID.test(row.sessionId)
        || typeof row.requestId !== 'string' || !UUID.test(row.requestId) || entries.has(row.requestId)
        || !['pending', 'ready', 'completed'].includes(row.state)) throw new Error('Invalid Local App session intent');
      validateNewAuthoringIntent({ name: row.name, setCurrent: row.setCurrent,
        ...(row.appId !== undefined ? { appId: row.appId } : {}), ...(row.sourceRequestId !== undefined ? { sourceRequestId: row.sourceRequestId } : {}) });
      if (row.verifiedInputSha256 !== digest(row)) throw new Error('Invalid Local App session intent');
      if (row.source === undefined ? row.sourceSha256 !== undefined || row.state === 'ready'
        : !validSource(row.source, row.sessionId) || row.sourceSha256 !== sourceDigest(row.source)) throw new Error('Invalid Local App session source');
      entries.set(row.requestId, row);
    }
    for (const [key, row] of entries) this.entries.set(key, row);
    this.loaded = true;
  }
  private async save(entries: Map<string, AuthoringSessionIntent>): Promise<void> {
    const directory = dirname(this.path); await fs.mkdir(directory, { recursive: true, mode: 0o700 });
    const temporary = `${this.path}.${randomUUID()}.tmp`;
    try {
      const file = await fs.open(temporary, 'wx', 0o600);
      try { await file.writeFile(JSON.stringify({ version: 1, entries: [...entries.values()] })); await file.sync(); }
      finally { await file.close(); }
      await fs.rename(temporary, this.path);
      if (process.platform !== 'win32') for (const path of [directory, dirname(directory)]) {
        const handle = await fs.open(path, 'r'); try { await handle.sync(); } finally { await handle.close(); }
      }
    } finally { await fs.rm(temporary, { force: true }); }
  }
  private async update(entry: AuthoringSessionIntent): Promise<void> {
    const updated = new Map(this.entries); updated.set(entry.requestId, entry);
    try { await this.save(updated); }
    catch (error) { this.loaded = false; this.entries.clear(); throw error; }
    this.entries.set(entry.requestId, entry);
  }
  async create(ref: SessionRef, value: LocalAppNewSessionIntent): Promise<AuthoringSessionIntent> {
    const input = validateNewAuthoringIntent(value);
    if (!validPath(ref.projectPath) || !UUID.test(ref.sessionId)) throw new Error('Invalid Local App session owner');
    return this.locked(async () => {
      await this.load();
      if ([...this.entries.values()].some(row => row.sessionId === ref.sessionId)) throw new Error('Local App session already has an intent');
      const owner = { ...input, projectPath: ref.projectPath, sessionId: ref.sessionId };
      const entry: AuthoringSessionIntent = { ...owner, requestId: randomUUID(), state: 'pending', verifiedInputSha256: digest(owner) };
      await this.update(entry); return { ...entry };
    });
  }
  async get(requestId: string): Promise<AuthoringSessionIntent | undefined> {
    if (!UUID.test(requestId)) throw new Error('Invalid Local App session intent');
    return this.locked(async () => { await this.load(); const row = this.entries.get(requestId); return row && { ...row }; });
  }
  async pending(): Promise<AuthoringSessionIntent[]> {
    return this.locked(async () => { await this.load(); return [...this.entries.values()].filter(row => row.state !== 'completed').map(row => ({ ...row })); });
  }
  async ready(requestId: string): Promise<void> {
    await this.locked(async () => { await this.load(); const row = this.entries.get(requestId);
      if (!row || row.state === 'completed' || !row.source) throw new Error('Local App session intent is unavailable');
      if (row.state !== 'ready') await this.update({ ...row, state: 'ready' }); });
  }
  async recordSource(requestId: string, source: AuthoringSessionSource): Promise<AuthoringSessionIntent> {
    return this.locked(async () => { await this.load(); const row = this.entries.get(requestId);
      if (!row || row.state === 'completed' || !validSource(source, row.sessionId)) throw new Error('Invalid Local App session source');
      if (row.source && (row.source.scope !== source.scope || row.source.transcriptPath !== source.transcriptPath)) {
        throw new Error('The Local App session source changed');
      }
      const entry = { ...row, source: { ...source }, sourceSha256: sourceDigest(source) };
      if (!row.source) await this.update(entry);
      return { ...entry };
    });
  }
  async complete(ref: SessionRef): Promise<void> {
    await this.locked(async () => { await this.load();
      const row = [...this.entries.values()].find(entry => entry.sessionId === ref.sessionId && entry.projectPath === ref.projectPath && entry.state === 'ready');
      if (row) await this.update({ ...row, state: 'completed' }); });
  }
}
