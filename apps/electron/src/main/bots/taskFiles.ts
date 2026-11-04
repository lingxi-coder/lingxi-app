/**
 * Files that change hands between two Bots for a task. A Bot decides which file
 * it names, and the host does the copying with the owner's rights, so every file
 * is checked as it is copied: it lies inside the workspace it is named in,
 * nothing on the way to it is a symbolic link, it is a regular file with one
 * name, and it is within the size cap. The copy is hashed as it is made and read
 * back to be sure it is what was hashed. Copies land in `inbox/<task id>/` of the
 * other workspace, which is checked the same way, so a link planted there cannot
 * send them somewhere else. Free of Electron.
 */

import { createHash } from 'node:crypto';
import { constants } from 'node:fs';
import { lstat, mkdir, open, realpath, rmdir, unlink } from 'node:fs/promises';
import { basename, dirname, isAbsolute, join, relative, sep } from 'node:path';

import { isInside } from './files.js';

export const MAX_TRANSFER_FILES = 16;
export const MAX_FILE_BYTES = 32 * 1024 * 1024;
export const MAX_TRANSFER_BYTES = 64 * 1024 * 1024;
const MAX_NAME_CHARS = 200;
const CHUNK_BYTES = 64 * 1024;
// eslint-disable-next-line no-control-regex
const CONTROL = /[\u0000-\u001f\u007f]/g;

/** A file that is not copied, and why, in words the Bot that named it can act on. */
export class FileRefusal extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'FileRefusal';
  }
}

/** A copy that was made. */
export interface CopiedFile {
  /** The file's name in the inbox. */
  name: string;
  /** Relative to the workspace it was copied into: `inbox/<task id>/<name>`. */
  path: string;
  size: number;
  sha256: string;
}

export interface TransferLimits {
  maxFiles: number;
  maxFileBytes: number;
  maxTotalBytes: number;
}

export const TRANSFER_LIMITS: TransferLimits = {
  maxFiles: MAX_TRANSFER_FILES,
  maxFileBytes: MAX_FILE_BYTES,
  maxTotalBytes: MAX_TRANSFER_BYTES,
};

/** The folders a path may be given in: the workspace as it is spelled and as it really is. */
async function workspaceRoots(workspace: string): Promise<{ real: string; spellings: string[] }> {
  const real = await realpath(workspace);
  return { real, spellings: [...new Set([real, workspace])] };
}

/** The parts of a path below the workspace, for a path named inside it. No `..`, nothing outside. */
function partsBelow(roots: string[], given: string): string[] {
  if (typeof given !== 'string' || given.length === 0 || given.includes('\0')) throw new FileRefusal('Name the file by its path in your workspace.');
  if (given.split(/[\\/]+/).includes('..')) throw new FileRefusal(`${given}: a path may not contain "..".`);
  let below = given;
  if (isAbsolute(given)) {
    const root = roots.find((candidate) => isInside(given, candidate));
    if (root === undefined) throw new FileRefusal(`${given} is outside your workspace.`);
    below = relative(root, given);
  }
  const parts = below.split(sep).filter((part) => part !== '' && part !== '.');
  if (parts.length === 0) throw new FileRefusal(`${given} is not a file.`);
  return parts;
}

/** The name a copy gets: the file's own, as one safe line. */
function copyName(parts: readonly string[]): string {
  const cleaned = basename(parts[parts.length - 1]!).replace(CONTROL, '').replace(/[\\/]/g, '_').trim().slice(0, MAX_NAME_CHARS);
  return cleaned === '' || cleaned === '.' || cleaned === '..' ? 'file' : cleaned;
}

async function hashOf(path: string): Promise<string> {
  const hash = createHash('sha256');
  const handle = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const chunk = Buffer.allocUnsafe(CHUNK_BYTES);
    for (;;) {
      const { bytesRead } = await handle.read(chunk, 0, chunk.length, null);
      if (bytesRead === 0) break;
      hash.update(chunk.subarray(0, bytesRead));
    }
  } finally {
    await handle.close();
  }
  return hash.digest('hex');
}

/** `inbox/<task id>` below a workspace, made if need be; refused if any of it is a link or not a folder. */
async function inboxOf(workspace: string, taskId: string): Promise<{ directory: string; real: string }> {
  const { real: root } = await workspaceRoots(workspace);
  let current = root;
  for (const part of ['inbox', taskId]) {
    current = join(current, part);
    let stats = await lstat(current).catch((error: NodeJS.ErrnoException) => {
      if (error.code === 'ENOENT') return undefined;
      throw error;
    });
    if (stats === undefined) {
      await mkdir(current, { mode: 0o700 });
      stats = await lstat(current);
    }
    if (stats.isSymbolicLink() || !stats.isDirectory()) throw new FileRefusal('The other workspace has something in the way of its inbox folder.');
  }
  return { directory: current, real: root };
}

/** Open the file named by `parts` below `root` for reading, refusing links, other kinds of file and hard links. */
async function openSource(root: string, parts: readonly string[], given: string, maxBytes: number): Promise<{ handle: Awaited<ReturnType<typeof open>>; size: number; path: string }> {
  let current = root;
  for (const [index, part] of parts.entries()) {
    current = join(current, part);
    const stats = await lstat(current).catch((error: NodeJS.ErrnoException) => {
      if (error.code === 'ENOENT' || error.code === 'ENOTDIR') throw new FileRefusal(`${given} does not exist.`);
      throw error;
    });
    if (stats.isSymbolicLink()) throw new FileRefusal(`${given}: symbolic links are not copied.`);
    if (index < parts.length - 1 ? !stats.isDirectory() : !stats.isFile()) throw new FileRefusal(`${given} is not a regular file.`);
  }
  const handle = await open(current, constants.O_RDONLY | constants.O_NOFOLLOW).catch((error: NodeJS.ErrnoException) => {
    if (error.code === 'ELOOP') throw new FileRefusal(`${given}: symbolic links are not copied.`);
    throw error;
  });
  try {
    const stats = await handle.stat();
    if (!stats.isFile() || stats.nlink !== 1) throw new FileRefusal(`${given} is not a regular file with a single name.`);
    if (stats.size > maxBytes) throw new FileRefusal(`${given} is larger than the ${Math.floor(maxBytes / (1024 * 1024))} MiB a task may carry.`);
    // The folders on the way may have been swapped for links since they were looked at.
    if (!isInside(await realpath(dirname(current)), root)) throw new FileRefusal(`${given} is outside your workspace.`);
    return { handle, size: stats.size, path: current };
  } catch (error) {
    await handle.close();
    throw error;
  }
}

/**
 * Copy files named in one workspace into `inbox/<task id>/` of another: all of
 * them, or none. Names that meet get a number after them.
 */
export async function transferFiles(
  fromWorkspace: string,
  paths: readonly string[],
  toWorkspace: string,
  taskId: string,
  limits: TransferLimits = TRANSFER_LIMITS,
): Promise<CopiedFile[]> {
  if (paths.length > limits.maxFiles) throw new FileRefusal(`A task carries at most ${limits.maxFiles} files.`);
  if (paths.length === 0) return [];
  const source = await workspaceRoots(fromWorkspace);
  const wanted = paths.map((given) => ({ given, parts: partsBelow(source.spellings, given) }));
  const inbox = await inboxOf(toWorkspace, taskId);
  const copies: Array<CopiedFile & { absolute: string }> = [];
  let total = 0;
  try {
    for (const { given, parts } of wanted) {
      const { handle, size, path } = await openSource(source.real, parts, given, limits.maxFileBytes);
      try {
        if (total + size > limits.maxTotalBytes) throw new FileRefusal(`Files over ${Math.floor(limits.maxTotalBytes / (1024 * 1024))} MiB in all cannot be carried.`);
        const copy = await copyOne(handle, size, given, copyName(parts), inbox, taskId, limits.maxFileBytes);
        copies.push(copy);
        total += copy.size;
      } finally {
        await handle.close();
      }
    }
  } catch (error) {
    await Promise.all(copies.map((copy) => unlink(copy.absolute).catch(() => undefined)));
    // The folders made for them go too, unless something else is in them.
    await rmdir(inbox.directory).then(() => rmdir(dirname(inbox.directory))).catch(() => undefined);
    throw error;
  }
  return copies.map(({ absolute: _absolute, ...copy }) => copy);
}

async function copyOne(
  source: Awaited<ReturnType<typeof open>>,
  size: number,
  given: string,
  name: string,
  inbox: { directory: string; real: string },
  taskId: string,
  maxBytes: number,
): Promise<CopiedFile & { absolute: string }> {
  let destination: Awaited<ReturnType<typeof open>> | undefined;
  let final = name;
  for (let attempt = 1; destination === undefined; attempt += 1) {
    try {
      destination = await open(join(inbox.directory, final), constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600);
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'EEXIST' || attempt >= 1000) throw error;
      const dot = name.lastIndexOf('.');
      final = dot > 0 ? `${name.slice(0, dot)} (${attempt + 1})${name.slice(dot)}` : `${name} (${attempt + 1})`;
    }
  }
  const absolute = join(inbox.directory, final);
  try {
    const hash = createHash('sha256');
    const chunk = Buffer.allocUnsafe(CHUNK_BYTES);
    let copied = 0;
    for (;;) {
      const { bytesRead } = await source.read(chunk, 0, chunk.length, null);
      if (bytesRead === 0) break;
      copied += bytesRead;
      if (copied > maxBytes) throw new FileRefusal(`${given} is larger than the ${Math.floor(maxBytes / (1024 * 1024))} MiB a task may carry.`);
      hash.update(chunk.subarray(0, bytesRead));
      await destination.write(chunk, 0, bytesRead);
    }
    if (copied !== size) throw new FileRefusal(`${given} changed while it was being copied.`);
    await destination.close();
    destination = undefined;
    const sha256 = hash.digest('hex');
    if (!isInside(await realpath(dirname(absolute)), inbox.real)) throw new FileRefusal('The other workspace has something in the way of its inbox folder.');
    if (await hashOf(absolute) !== sha256) throw new FileRefusal(`${given} could not be copied faithfully.`);
    return { name: final, path: `inbox/${taskId}/${final}`, size, sha256, absolute };
  } catch (error) {
    await destination?.close().catch(() => undefined);
    await unlink(absolute).catch(() => undefined);
    throw error;
  }
}
