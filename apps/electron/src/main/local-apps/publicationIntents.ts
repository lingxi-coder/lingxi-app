import { createHash, randomUUID } from 'node:crypto';
import { promises as fs } from 'node:fs';
import { basename, dirname, isAbsolute, join, relative, sep } from 'node:path';

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const APP = /^a[a-f0-9]{32}$/;
const HASH = /^[a-f0-9]{64}$/;

export interface PublicationIntent {
  requestId: string;
  sourceDirectory: string;
  authoringSessionId: string;
  appId?: string;
  expectedRevision: number;
  stagedDigest: string;
  sourcePath: string;
  stagingInputPath: string;
  native?: { sourceId: string; setCurrent: boolean };
  state: 'preparing' | 'pending' | 'completed';
  verifiedInputSha256: string;
}
type PublicationInput = Omit<PublicationIntent, 'state' | 'verifiedInputSha256'>;

function absolutePath(value: unknown): value is string {
  return typeof value === 'string' && isAbsolute(value) && !value.includes('\0')
    && !value.split(/[\\/]/).some(part => part === '.' || part === '..');
}
function digest(input: PublicationInput): string {
  return createHash('sha256').update(JSON.stringify({ requestId: input.requestId, sourceDirectory: input.sourceDirectory,
    authoringSessionId: input.authoringSessionId, appId: input.appId ?? null, expectedRevision: input.expectedRevision,
    stagedDigest: input.stagedDigest, sourcePath: input.sourcePath, stagingInputPath: input.stagingInputPath, native: input.native ?? null })).digest('hex');
}
function valid(input: PublicationInput, importsRoot: string): boolean {
  if (!UUID.test(input.requestId) || !UUID.test(input.authoringSessionId) || !absolutePath(input.sourceDirectory)
    || !absolutePath(input.sourcePath) || !absolutePath(input.stagingInputPath) || !HASH.test(input.stagedDigest)
    || (input.appId !== undefined && !APP.test(input.appId))
    || !Number.isSafeInteger(input.expectedRevision) || input.expectedRevision < 0) return false;
  const rel = relative(importsRoot, input.sourcePath);
  if (rel === '..' || rel.startsWith(`..${sep}`) || isAbsolute(rel) || rel.includes(sep)
    || basename(rel) !== `host-publish-${input.requestId}`) return false;
  const inputRel = relative(importsRoot, input.stagingInputPath);
  if (!inputRel || inputRel === '..' || inputRel.startsWith(`..${sep}`) || isAbsolute(inputRel)
    || input.stagingInputPath === input.sourcePath) return false;
  return input.native === undefined || (input.native !== null && typeof input.native === 'object' && !Array.isArray(input.native)
    && UUID.test(input.native.sourceId) && typeof input.native.setCurrent === 'boolean');
}
function sameOperation(a: PublicationInput, b: PublicationInput): boolean {
  return a.requestId === b.requestId && a.sourceDirectory === b.sourceDirectory && a.authoringSessionId === b.authoringSessionId
    && a.appId === b.appId && a.expectedRevision === b.expectedRevision && a.stagedDigest === b.stagedDigest
    && a.native?.sourceId === b.native?.sourceId && Boolean(a.native) === Boolean(b.native);
}

/** Crash-recovery receipts contain operation identity, never an authorization grant. */
export class PublicationIntents {
  private readonly directory: string;
  private tail: Promise<void> = Promise.resolve();
  constructor(root: string, private readonly importsRoot: string) { this.directory = join(root, 'publication-intents'); }

  private async locked<T>(operation: () => Promise<T>): Promise<T> {
    const previous = this.tail;
    let release!: () => void;
    this.tail = new Promise<void>(resolve => { release = resolve; });
    await previous;
    try { return await operation(); } finally { release(); }
  }
  private async read(requestId: string): Promise<PublicationIntent | undefined> {
    if (!UUID.test(requestId)) throw new Error('Invalid Local App publication request');
    const path = join(this.directory, `${requestId}.json`);
    let value: unknown;
    try {
      const stat = await fs.lstat(path);
      if (!stat.isFile() || stat.nlink !== 1 || stat.size > 16384) throw new Error('invalid publication intent file');
      value = JSON.parse(await fs.readFile(path, 'utf8'));
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') return;
      throw new Error('Could not read Local App publication intent; refusing to overwrite it', { cause: error });
    }
    if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid Local App publication intent');
    const entry = value as PublicationIntent;
    if (Object.keys(entry).some(key => !['requestId', 'sourceDirectory', 'authoringSessionId', 'appId', 'expectedRevision', 'stagedDigest', 'sourcePath', 'stagingInputPath', 'native', 'state', 'verifiedInputSha256'].includes(key))
      || (entry.native && Object.keys(entry.native).some(key => key !== 'sourceId' && key !== 'setCurrent'))
      || !valid(entry, this.importsRoot) || entry.requestId !== requestId || !['preparing', 'pending', 'completed'].includes(entry.state)
      || entry.verifiedInputSha256 !== digest(entry)) throw new Error('Invalid Local App publication intent; refusing to overwrite it');
    return entry;
  }
  private async write(entry: PublicationIntent): Promise<void> {
    await fs.mkdir(this.directory, { recursive: true, mode: 0o700 });
    const path = join(this.directory, `${entry.requestId}.json`), temporary = `${path}.${randomUUID()}.tmp`;
    try {
      const file = await fs.open(temporary, 'wx', 0o600);
      try { await file.writeFile(JSON.stringify(entry)); await file.sync(); } finally { await file.close(); }
      await fs.rename(temporary, path);
      if (process.platform !== 'win32') {
        for (const directory of [this.directory, dirname(this.directory), dirname(dirname(this.directory)), dirname(dirname(dirname(this.directory)))]) {
          const handle = await fs.open(directory, 'r');
          try { await handle.sync(); } finally { await handle.close(); }
        }
      }
    } finally { await fs.rm(temporary, { force: true }); }
  }
  async capture(input: PublicationInput): Promise<{ entry: PublicationIntent; created: boolean }> {
    if (!valid(input, this.importsRoot)) throw new Error('Invalid Local App publication intent');
    return this.locked(async () => {
      const previous = await this.read(input.requestId);
      if (previous) {
        if (!sameOperation(previous, input)) throw new Error('The Local App publication request conflicts with its saved intent');
        return { entry: previous, created: false };
      }
      const entry: PublicationIntent = { ...input, state: 'preparing', verifiedInputSha256: digest(input) };
      await this.write(entry);
      return { entry, created: true };
    });
  }
  async get(requestId: string): Promise<PublicationIntent | undefined> {
    return this.locked(() => this.read(requestId));
  }
  async pending(): Promise<PublicationIntent[]> {
    return this.locked(async () => {
      let names: string[];
      try { names = await fs.readdir(this.directory); }
      catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT') return []; throw error; }
      const result: PublicationIntent[] = [];
      for (const name of names) {
        if (!name.endsWith('.json')) continue;
        const requestId = name.slice(0, -5);
        const entry = await this.read(requestId);
        if (entry && entry.state !== 'completed') result.push(entry);
      }
      return result;
    });
  }
  async ready(requestId: string): Promise<void> {
    await this.locked(async () => {
      const entry = await this.read(requestId);
      if (!entry) throw new Error('Local App publication intent is missing');
      if (entry.state === 'preparing') await this.write({ ...entry, state: 'pending' });
    });
  }
  /** Replace only a never-admitted preparing input which has disappeared. */
  async replacePreparingInput(requestId: string, stagingInputPath: string): Promise<PublicationIntent> {
    return this.locked(async () => {
      const entry = await this.read(requestId);
      if (!entry || entry.state !== 'preparing') throw new Error('The Local App publication is no longer preparing');
      for (const path of [entry.sourcePath, entry.stagingInputPath]) {
        try { await fs.lstat(path); return entry; }
        catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
      }
      const updated = { ...entry, stagingInputPath };
      if (!valid(updated, this.importsRoot)) throw new Error('Invalid replacement Local App staging');
      await fs.lstat(stagingInputPath);
      updated.verifiedInputSha256 = digest(updated);
      await this.write(updated);
      return updated;
    });
  }
  async complete(requestId: string): Promise<void> {
    await this.locked(async () => {
      const entry = await this.read(requestId);
      if (!entry) throw new Error('Local App publication intent is missing');
      if (entry.state !== 'completed') await this.write({ ...entry, state: 'completed' });
    });
  }
}
