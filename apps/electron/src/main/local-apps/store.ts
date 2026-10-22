import { promises as fs, constants, type BigIntStats } from 'node:fs';
import { join, dirname, relative, resolve, sep } from 'node:path';
import { randomUUID } from 'node:crypto';
import { pipeline } from 'node:stream/promises';
import type { DesktopLocalApp } from '../../shared/localApps.js';
import { parseManifest, relativePath } from './manifest.js';

export function appId(value: unknown): string {
  if (typeof value !== 'string' || !/^a[a-f0-9]{20}$/.test(value)) throw new Error('Invalid desktop application id');
  return value;
}
export async function atomicJson(path: string, value: unknown): Promise<void> {
  await fs.mkdir(dirname(path), { recursive: true, mode: 0o700 });
  const temp = `${path}.${randomUUID()}.tmp`;
  try { await fs.writeFile(temp, JSON.stringify(value, null, 2), { mode: 0o600 }); await fs.rename(temp, path); }
  finally { await fs.rm(temp, { force: true }); }
}
export async function containedFile(root: string, path: string): Promise<string> {
  const rel = relativePath(path);
  const canonicalRoot = await fs.realpath(root);
  let current = canonicalRoot;
  for (const part of rel.split('/').filter(p => p !== '.')) {
    current = join(current, part);
    if ((await fs.lstat(current)).isSymbolicLink()) throw new Error('Application source and asset symlinks are not supported');
  }
  const canonical = await fs.realpath(current);
  const tail = relative(canonicalRoot, canonical);
  if (tail === '..' || tail.startsWith(`..${sep}`)) throw new Error('Application path leaves its root');
  return canonical;
}
/** Copy a source snapshot, without following links or importing credentials / build environments. */
export async function copySource(source: string, destination: string): Promise<void> {
  type Directory = { path: string; stat: BigIntStats };
  const sameFile = (a: BigIntStats, b: BigIntStats) => a.dev === b.dev && a.ino === b.ino;
  const changed = () => new Error('Application source changed while taking its snapshot');
  const checkDirectories = async (directories: Directory[]) => {
    for (const directory of directories) {
      const current = await fs.lstat(directory.path, { bigint: true });
      if (current.isSymbolicLink() || !current.isDirectory() || !sameFile(current, directory.stat)) throw changed();
    }
  };
  const copy = async (fromDirectory: string, toDirectory: string, parents: Directory[]) => {
    await checkDirectories(parents);
    const stat = await fs.lstat(fromDirectory, { bigint: true });
    if (stat.isSymbolicLink()) throw new Error('Application source contains a directory symlink');
    if (!stat.isDirectory()) throw new Error('Application source is not a directory');
    const directories = [...parents, { path: fromDirectory, stat }];
    await fs.mkdir(toDirectory, { recursive: true, mode: 0o700 });
    const entries = await fs.readdir(fromDirectory, { withFileTypes: true });
    await checkDirectories(directories);
    for (const entry of entries) {
      if (['.git', 'node_modules', '.venv', 'venv', '__pycache__', '.DS_Store', '.lingxi-prepared.json'].includes(entry.name)
        || entry.name === '.env' || (entry.name.startsWith('.env.') && entry.name !== '.env.example')) continue;
      if (entry.isSymbolicLink()) throw new Error(`Application source contains a symlink: ${entry.name}`);
      const from = join(fromDirectory, entry.name), to = join(toDirectory, entry.name);
      if (entry.isDirectory()) {
        const child = await fs.lstat(from, { bigint: true });
        if (child.isSymbolicLink()) throw new Error('Application source contains a directory symlink');
        if (!child.isDirectory()) throw changed();
        let virtualEnvironment = false;
        try { virtualEnvironment = (await fs.lstat(join(from, 'pyvenv.cfg'))).isFile(); }
        catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
        await checkDirectories([...directories, { path: from, stat: child }]);
        // Python/uv environments need not be named .venv. Their interpreter
        // links and installed packages are recreated by the manifest's steps.
        if (!virtualEnvironment) await copy(from, to, directories);
      }
      else if (entry.isFile()) {
        await checkDirectories(directories);
        const input = await fs.open(from, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0));
        try {
          const opened = await input.stat({ bigint: true });
          const checkFile = async () => {
            await checkDirectories(directories);
            const current = await fs.lstat(from, { bigint: true });
            if (!opened.isFile() || current.isSymbolicLink() || !current.isFile() || !sameFile(opened, current)) throw changed();
          };
          // O_NOFOLLOW protects the leaf. Verify the opened identity and its
          // parent chain as well, so a directory retarget cannot be published.
          await checkFile();
          const output = await fs.open(to, 'wx', 0o600);
          try {
            if (process.platform !== 'win32') await output.chmod(0o600 | (opened.mode & 0o111n ? 0o100 : 0));
            await pipeline(input.createReadStream(), output.createWriteStream());
            await checkFile();
          } finally { await output.close(); }
        } finally { await input.close(); }
      } else throw new Error(`Application source contains a special file: ${entry.name}`);
    }
    await checkDirectories(directories);
  };
  await copy(source, destination, []);
}

export class LocalAppStore {
  constructor(readonly root: string) {}
  directory(id: string): string { return join(this.root, appId(id)); }
  revisionPath(id: string, revision: number): string {
    if (!Number.isSafeInteger(revision) || revision < 1) throw new Error('Invalid app revision');
    return join(this.directory(id), 'revisions', String(revision));
  }
  dataPath(id: string): string { return join(this.directory(id), 'data'); }
  workspacePath(id: string, revision: number): string { return join(this.directory(id), 'workspaces', String(revision)); }
  async get(id: string): Promise<DesktopLocalApp> {
    const row = JSON.parse(await fs.readFile(join(this.directory(id), 'app.json'), 'utf8')) as DesktopLocalApp;
    if (row.id !== id) throw new Error('Application record identity mismatch');
    row.manifest = parseManifest(row.manifest);
    return row;
  }
  async list(): Promise<DesktopLocalApp[]> {
    await fs.mkdir(this.root, { recursive: true, mode: 0o700 });
    const rows: DesktopLocalApp[] = [];
    for (const entry of await fs.readdir(this.root, { withFileTypes: true })) {
      if (!entry.isDirectory() || !/^a[a-f0-9]{20}$/.test(entry.name)) continue;
      try { rows.push(await this.get(entry.name)); }
      catch (error) {
        // First publication creates its private directory before the atomic
        // app.json commit. A crash can leave the same uncommitted directory.
        if ((error as NodeJS.ErrnoException).code === 'ENOENT') continue;
        throw new Error(`Cannot load application ${entry.name}: ${error instanceof Error ? error.message : String(error)}`, { cause: error });
      }
    }
    return rows.sort((a, b) => b.updatedAt - a.updatedAt);
  }
  async save(row: DesktopLocalApp): Promise<void> {
    const { phase: _phase, error: _error, runningRevision: _running, ...persisted } = row;
    await atomicJson(join(this.directory(row.id), 'app.json'), { ...persisted, phase: 'stopped' });
  }
  async publish(input: { id?: string; name?: string; summary?: string; html?: string; staged_path?: string; source_path?: string; source_session?: DesktopLocalApp['sourceSession'] }): Promise<DesktopLocalApp> {
    const previous = input.id ? await this.get(input.id) : undefined;
    const id = previous?.id ?? `a${randomUUID().replaceAll('-', '').slice(0, 20)}`;
    let latestRevision = previous?.revision ?? 0;
    try {
      for (const entry of await fs.readdir(join(this.directory(id), 'revisions'), { withFileTypes: true })) {
        const number = Number(entry.name);
        if (entry.isDirectory() && /^[1-9]\d*$/.test(entry.name) && Number.isSafeInteger(number)) latestRevision = Math.max(latestRevision, number);
      }
    } catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
    // A crash after installing a snapshot but before app.json commits must
    // never make that snapshot collide with every subsequent publication.
    const revision = latestRevision + 1;
    const temp = join(this.directory(id), `publish-${randomUUID()}`);
    const target = this.revisionPath(id, revision);
    await fs.mkdir(temp, { recursive: true, mode: 0o700 });
    let published = false;
    try {
      if (input.html !== undefined) {
        if (!input.html.trim()) throw new Error('The application document is empty');
        await fs.writeFile(join(temp, 'index.html'), input.html);
        await atomicJson(join(temp, 'lingxi-app.json'), { schema: 1, name: input.name ?? previous?.name, summary: input.summary ?? previous?.summary,
          entry: { type: 'static', path: 'index.html' } });
      } else if (input.staged_path) {
        // Only the authenticated tool can submit its private staging directory.
        const imports = await fs.realpath(join(this.root, 'imports'));
        const staged = await fs.realpath(input.staged_path);
        if (!staged.startsWith(imports + sep) || relative(imports, staged).includes(sep)) throw new Error('Invalid application staging directory');
        await copySource(staged, temp);
      } else throw new Error('Publishing requires html or a staged project');
      const sourceManifest = parseManifest(JSON.parse(await fs.readFile(join(temp, 'lingxi-app.json'), 'utf8')));
      const manifest = parseManifest({ ...sourceManifest, name: input.name ?? sourceManifest.name, summary: input.summary ?? sourceManifest.summary });
      await atomicJson(join(temp, 'lingxi-app.json'), manifest);
      if (manifest.entry.type === 'static' && !manifest.build?.length) {
        if (!(await fs.stat(await containedFile(temp, manifest.entry.path))).isFile()) throw new Error('The application entry is not a file');
      }
      await fs.mkdir(dirname(target), { recursive: true, mode: 0o700 });
      await fs.rename(temp, target);
      published = true;
      const now = Date.now();
      const row: DesktopLocalApp = { id, revision, manifest, name: manifest.name, summary: manifest.summary ?? '',
        createdAt: previous?.createdAt ?? now, updatedAt: now, phase: 'stopped', background: previous?.background ?? false,
        grants: previous?.grants ?? [], sourcePath: input.source_path ?? previous?.sourcePath,
        sourceSession: input.source_session ?? previous?.sourceSession };
      await this.save(row);
      return row;
    } catch (error) {
      if (published) await fs.rm(target, { recursive: true, force: true });
      // An interrupted first publish must not leave a broken library row.
      if (!previous) await fs.rm(this.directory(id), { recursive: true, force: true });
      throw error;
    } finally { await fs.rm(temp, { recursive: true, force: true }); }
  }
  async prepareWorkspace(row: DesktopLocalApp): Promise<string> {
    const workspace = this.workspacePath(row.id, row.revision);
    try { await fs.access(join(workspace, 'lingxi-app.json')); }
    catch {
      const temp = `${workspace}.${randomUUID()}.tmp`;
      try {
        await copySource(this.revisionPath(row.id, row.revision), temp);
        await fs.mkdir(dirname(workspace), { recursive: true, mode: 0o700 });
        await fs.rename(temp, workspace);
      } finally { await fs.rm(temp, { recursive: true, force: true }); }
    }
    await fs.mkdir(this.dataPath(row.id), { recursive: true, mode: 0o700 });
    return workspace;
  }
  async remove(id: string): Promise<void> { await fs.rm(this.directory(id), { recursive: true, force: true }); }
}
