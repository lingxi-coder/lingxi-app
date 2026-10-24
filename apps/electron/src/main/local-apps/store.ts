import { promises as fs, constants, type BigIntStats } from 'node:fs';
import { join, dirname, relative, sep } from 'node:path';
import { randomUUID } from 'node:crypto';
import { pipeline } from 'node:stream/promises';
import type { DesktopLocalApp } from '../../shared/localApps.js';
import { relativePath } from './manifest.js';

export function appId(value: unknown): string {
  if (typeof value !== 'string' || !/^a[a-f0-9]{32}$/.test(value)) throw new Error('Invalid desktop application id');
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

export interface LocalAppPublishInput {
  id?: string; name?: string; summary?: string; html?: string; staged_path?: string; source_path?: string;
  source_session?: DesktopLocalApp['sourceSession']; expectedRevision?: number;
}
/** Platform execution adapter contract. The Rust library is the production store. */
export interface LocalAppStore {
  readonly root: string;
  directory(id: string): string;
  revisionPath(id: string, revision: number): string;
  dataPath(id: string): string;
  get(id: string): Promise<DesktopLocalApp>;
  list(): Promise<DesktopLocalApp[]>;
  save(row: DesktopLocalApp): Promise<void>;
  publish(input: LocalAppPublishInput): Promise<DesktopLocalApp>;
  prepareWorkspace(row: DesktopLocalApp): Promise<string>;
  remove(id: string): Promise<void>;
}
