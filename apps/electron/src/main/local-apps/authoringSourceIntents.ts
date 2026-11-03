import { createHash, randomUUID } from 'node:crypto';
import { promises as fs } from 'node:fs';
import { dirname, isAbsolute, join } from 'node:path';

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export interface SourceIntentInput {
  purpose: 'create' | 'rebind' | 'repair';
  sourceDirectory: string;
  projectPath: string;
  sessionId: string;
  appId?: string;
}
interface SourceIntent extends SourceIntentInput { requestId: string; verifiedInputSha256: string }
export interface CopyIntentInput { purpose: 'restoreSource' | 'remix'; appId: string; revision: number; sourcePath: string }
export interface CopyIntent extends CopyIntentInput {
  requestId: string; verifiedInputSha256: string; state: 'pending' | 'ready' | 'completed'; sourceId?: string; targetAppId?: string;
}

function validPath(value: unknown): value is string {
  return typeof value === 'string' && isAbsolute(value) && !value.includes('\0')
    && !value.split(/[\\/]/).some(part => part === '.' || part === '..');
}
function digest(input: SourceIntentInput): string {
  return createHash('sha256').update(JSON.stringify({ purpose: input.purpose, sourceDirectory: input.sourceDirectory,
    projectPath: input.projectPath, sessionId: input.sessionId, appId: input.appId ?? null })).digest('hex');
}
function validInput(input: SourceIntentInput): boolean {
  return (input.purpose === 'create' || input.purpose === 'rebind' || input.purpose === 'repair')
    && validPath(input.sourceDirectory) && input.projectPath === input.sourceDirectory && UUID.test(input.sessionId)
    && (input.purpose === 'create' ? input.appId === undefined : typeof input.appId === 'string' && /^a[a-f0-9]{32}$/.test(input.appId));
}
function copyDigest(input: CopyIntentInput): string {
  return createHash('sha256').update(JSON.stringify({ purpose: input.purpose, appId: input.appId,
    revision: input.revision, sourcePath: input.sourcePath })).digest('hex');
}
function validCopyInput(input: CopyIntentInput): boolean {
  return (input.purpose === 'restoreSource' || input.purpose === 'remix') && /^a[a-f0-9]{32}$/.test(input.appId)
    && Number.isSafeInteger(input.revision) && input.revision > 0 && validPath(input.sourcePath);
}

/** Persist idempotency intents, never authoring grants; the native library owns those. */
export class AuthoringSourceIntents {
  private readonly path: string;
  private readonly entries = new Map<string, SourceIntent>();
  private readonly copies = new Map<string, CopyIntent>();
  private loaded = false;
  private tail: Promise<void> = Promise.resolve();

  constructor(root: string) { this.path = join(root, 'authoring-source-intents.v1.json'); }

  private async locked<T>(operation: () => Promise<T>): Promise<T> {
    const previous = this.tail;
    let release!: () => void;
    this.tail = new Promise(resolve => { release = resolve; });
    await previous;
    try { return await operation(); } finally { release(); }
  }

  private async load(): Promise<void> {
    if (this.loaded) return;
    let content: string;
    try {
      const stat = await fs.lstat(this.path);
      if (!stat.isFile() || stat.nlink !== 1 || stat.size > 4 * 1024 * 1024) throw new Error('invalid source intent file');
      content = await fs.readFile(this.path, 'utf8');
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') { this.loaded = true; return; }
      throw new Error('Could not read Local App source intents; refusing to overwrite them', { cause: error });
    }
    let parsed: unknown;
    try { parsed = JSON.parse(content); } catch (error) {
      throw new Error('Local App source intents are invalid; refusing to overwrite them', { cause: error });
    }
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)
      || Object.keys(parsed).some(key => !['version', 'entries', 'copies'].includes(key))
      || (parsed as Record<string, unknown>).version !== 1 || !Array.isArray((parsed as Record<string, unknown>).entries)
      || !Array.isArray((parsed as Record<string, unknown>).copies)) {
      throw new Error('Local App source intents are invalid; refusing to overwrite them');
    }
    const seenIds = new Set<string>();
    const entries = new Map<string, SourceIntent>();
    for (const value of (parsed as { entries: unknown[] }).entries) {
      if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Local App source intent is invalid');
      const entry = value as SourceIntent;
      if (Object.keys(entry).some(key => !['purpose', 'sourceDirectory', 'projectPath', 'sessionId', 'appId', 'requestId', 'verifiedInputSha256'].includes(key))
        || !validInput(entry) || typeof entry.requestId !== 'string' || !UUID.test(entry.requestId)
        || entry.verifiedInputSha256 !== digest(entry) || entries.has(entry.verifiedInputSha256) || seenIds.has(entry.requestId)) {
        throw new Error('Local App source intent is invalid; refusing to overwrite it');
      }
      entries.set(entry.verifiedInputSha256, entry); seenIds.add(entry.requestId);
    }
    const copies = new Map<string, CopyIntent>();
    for (const value of (parsed as { copies: unknown[] }).copies) {
      if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Local App copy intent is invalid');
      const entry = value as CopyIntent;
      if (Object.keys(entry).some(key => !['purpose', 'appId', 'revision', 'sourcePath', 'requestId', 'verifiedInputSha256', 'state', 'sourceId', 'targetAppId'].includes(key))
        || !validCopyInput(entry) || typeof entry.requestId !== 'string' || !UUID.test(entry.requestId)
        || entry.verifiedInputSha256 !== copyDigest(entry) || seenIds.has(entry.requestId)
        || !['pending', 'ready', 'completed'].includes(entry.state)
        || (entry.state === 'pending' ? entry.sourceId !== undefined || entry.targetAppId !== undefined
          : typeof entry.sourceId !== 'string' || !UUID.test(entry.sourceId)
            || typeof entry.targetAppId !== 'string' || !/^a[a-f0-9]{32}$/.test(entry.targetAppId))
        || (entry.purpose === 'restoreSource' && entry.targetAppId !== undefined && entry.targetAppId !== entry.appId)) {
        throw new Error('Local App copy intent is invalid; refusing to overwrite it');
      }
      copies.set(entry.requestId, entry); seenIds.add(entry.requestId);
    }
    for (const [key, entry] of entries) this.entries.set(key, entry);
    for (const [key, entry] of copies) this.copies.set(key, entry);
    this.loaded = true;
  }

  private async persist(entries: Map<string, SourceIntent>, copies: Map<string, CopyIntent>): Promise<void> {
    const directory = dirname(this.path);
    await fs.mkdir(directory, { recursive: true, mode: 0o700 });
    const temporary = `${this.path}.${randomUUID()}.tmp`;
    try {
      const file = await fs.open(temporary, 'wx', 0o600);
      try {
        await file.writeFile(JSON.stringify({ version: 1, entries: [...entries.values()], copies: [...copies.values()] }));
        await file.sync();
      } finally { await file.close(); }
      await fs.rename(temporary, this.path);
      // Node cannot portably open directories for fsync on Windows. On POSIX,
      // synchronize both the entry and its parent before contacting the SDK.
      if (process.platform !== 'win32') {
        for (const path of [directory, dirname(directory)]) {
          const handle = await fs.open(path, 'r');
          try { await handle.sync(); } finally { await handle.close(); }
        }
      }
    } finally { await fs.rm(temporary, { force: true }); }
  }

  async sourceRequestId(input: SourceIntentInput): Promise<string> {
    if (!validInput(input)) throw new Error('Invalid Local App source intent');
    return this.locked(async () => {
      await this.load();
      const verifiedInputSha256 = digest(input), existing = this.entries.get(verifiedInputSha256);
      if (existing) return existing.requestId;
      const entry: SourceIntent = { ...input, requestId: randomUUID(), verifiedInputSha256 };
      const updated = new Map(this.entries); updated.set(verifiedInputSha256, entry);
      try { await this.persist(updated, this.copies); }
      catch (error) {
        // A rename may have succeeded before synchronization failed. Reload
        // its actual contents on retry instead of minting a conflicting ID.
        this.loaded = false; this.entries.clear(); this.copies.clear();
        throw error;
      }
      this.entries.set(verifiedInputSha256, entry);
      return entry.requestId;
    });
  }

  async findSourceRequestId(input: SourceIntentInput): Promise<string | undefined> {
    if (!validInput(input)) throw new Error('Invalid Local App source intent');
    return this.locked(async () => { await this.load(); return this.entries.get(digest(input))?.requestId; });
  }

  async createCopy(input: CopyIntentInput): Promise<CopyIntent> {
    if (!validCopyInput(input)) throw new Error('Invalid Local App source copy intent');
    return this.locked(async () => {
      await this.load();
      const entry: CopyIntent = { ...input, requestId: randomUUID(), verifiedInputSha256: copyDigest(input), state: 'pending' };
      const updated = new Map(this.copies); updated.set(entry.requestId, entry);
      try { await this.persist(this.entries, updated); }
      catch (error) { this.loaded = false; this.entries.clear(); this.copies.clear(); throw error; }
      this.copies.set(entry.requestId, entry);
      return { ...entry };
    });
  }

  async getCopy(requestId: string): Promise<CopyIntent | undefined> {
    if (!UUID.test(requestId)) throw new Error('Invalid Local App source copy intent');
    return this.locked(async () => { await this.load(); const entry = this.copies.get(requestId); return entry && { ...entry }; });
  }

  async pendingCopies(): Promise<CopyIntent[]> {
    return this.locked(async () => {
      await this.load();
      return [...this.copies.values()].filter(entry => entry.state !== 'completed').map(entry => ({ ...entry }));
    });
  }

  async recordCopy(requestId: string, sourceId: string, targetAppId: string): Promise<CopyIntent> {
    return this.locked(async () => {
      await this.load();
      const previous = this.copies.get(requestId);
      if (!previous || !UUID.test(sourceId) || !/^a[a-f0-9]{32}$/.test(targetAppId)
        || (previous.purpose === 'restoreSource' && targetAppId !== previous.appId)
        || (previous.sourceId !== undefined && (previous.sourceId !== sourceId || previous.targetAppId !== targetAppId))) {
        throw new Error('The Local App source copy result does not match its saved intent');
      }
      const entry: CopyIntent = { ...previous, sourceId, targetAppId, state: previous.state === 'completed' ? 'completed' : 'ready' };
      const updated = new Map(this.copies); updated.set(requestId, entry);
      try { await this.persist(this.entries, updated); }
      catch (error) { this.loaded = false; this.entries.clear(); this.copies.clear(); throw error; }
      this.copies.set(requestId, entry); return { ...entry };
    });
  }

  async completeCopy(requestId: string): Promise<void> {
    await this.locked(async () => {
      await this.load();
      const previous = this.copies.get(requestId);
      if (!previous || previous.state === 'pending') throw new Error('The Local App source copy has not been confirmed');
      if (previous.state === 'completed') return;
      const entry: CopyIntent = { ...previous, state: 'completed' };
      const updated = new Map(this.copies); updated.set(requestId, entry);
      try { await this.persist(this.entries, updated); }
      catch (error) { this.loaded = false; this.entries.clear(); this.copies.clear(); throw error; }
      this.copies.set(requestId, entry);
    });
  }
}
