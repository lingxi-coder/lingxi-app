/**
 * Bridge discovery-lockfile reader — the Node-side mirror of the Rust
 * `bridge::lockfile` writer (`bridge/src/lockfile.rs`).
 *
 * The bridge-server writes a JSON discovery file at
 * `~/.lingxi/bridge/<port>.lock` (F2-04) whose body matches claude-code's
 * `LockfileJsonContent`: camelCase keys, the port encoded in the FILENAME (not
 * the body), and a 32-char hex `authToken` the client presents in the
 * `X-LingXi-Ide-Authorization` WS upgrade header.
 */

import { constants, readdirSync, readFileSync, statSync, type Stats } from 'node:fs';
import { lstat, open, readdir, readFile, stat } from 'node:fs/promises';
import { homedir } from 'node:os';
import { basename, join } from 'node:path';

/** JSON body of a lockfile — matches the Rust `LockfileBody` (camelCase keys). */
export interface LockfileBody {
  pid: number;
  workspaceFolders: string[];
  ideName: string;
  transport: string;
  runningInWindows: boolean;
  authToken: string;
}

/** A parsed lockfile: its on-disk path, the port from the filename, and body. */
export interface Lockfile {
  path: string;
  port: number;
  body: LockfileBody;
}

/** The default `~/.lingxi/bridge` discovery directory the bridge-server writes to. */
export function defaultBridgeDir(): string {
  return join(homedir(), '.lingxi', 'bridge');
}

/** Recover the port encoded in a `<port>.lock` filename, or `null` if malformed. */
function portFromFilename(path: string): number | null {
  const name = basename(path);
  if (!name.endsWith('.lock') || name.startsWith('.')) {
    return null;
  }
  const stem = name.slice(0, -'.lock'.length);
  const port = Number.parseInt(stem, 10);
  if (!Number.isInteger(port) || String(port) !== stem || port < 0 || port > 65535) {
    return null;
  }
  return port;
}

/**
 * Parse a single lockfile from disk. Throws if the file is missing, not valid
 * JSON, or the filename is not `<port>.lock`.
 */
export function readLockfile(path: string): Lockfile {
  const port = portFromFilename(path);
  if (port === null) {
    throw new Error(`lockfile name not <port>.lock: ${path}`);
  }
  return parseLockfile(path, port, readFileSync(path, 'utf8'));
}

function parseLockfile(path: string, port: number, source: string): Lockfile {
  const body = JSON.parse(source) as LockfileBody;
  if (typeof body?.authToken !== 'string' || body.authToken.length === 0) {
    throw new Error(`lockfile missing authToken: ${path}`);
  }
  return { path, port, body };
}

const MAX_PRIVATE_LOCKFILE_BYTES = 64 * 1024;

function sameLockfile(before: Stats, after: Stats): boolean {
  return after.isFile() && before.dev === after.dev && before.ino === after.ino
    && before.size === after.size && before.mode === after.mode && before.uid === after.uid
    && before.ctimeMs === after.ctimeMs && before.mtimeMs === after.mtimeMs;
}

async function readOwnerOnlyLockfile(path: string): Promise<string> {
  const expected = await lstat(path);
  if (!expected.isFile()) throw new Error('bridge lockfile is not a regular file');
  if (expected.size > MAX_PRIVATE_LOCKFILE_BYTES) throw new Error('bridge lockfile is too large');
  if (process.platform !== 'win32' && (expected.mode & 0o077) !== 0) {
    throw new Error('bridge lockfile permissions are not owner-only');
  }
  if (process.getuid && expected.uid !== process.getuid()) throw new Error('bridge lockfile owner mismatch');

  const file = await open(path, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0) | (constants.O_NONBLOCK ?? 0));
  try {
    if (!sameLockfile(expected, await file.stat())) throw new Error('bridge lockfile changed while opening');
    const bytes = Buffer.alloc(expected.size + 1);
    let offset = 0;
    while (offset < bytes.length) {
      const { bytesRead } = await file.read(bytes, offset, bytes.length - offset, offset);
      if (!bytesRead) break;
      offset += bytesRead;
    }
    if (offset !== expected.size || !sameLockfile(expected, await lstat(path))) {
      throw new Error('bridge lockfile changed while reading');
    }
    return bytes.subarray(0, offset).toString('utf8');
  } finally {
    await file.close();
  }
}

/** Capture one connection's discovery file without blocking the host event loop. */
export async function readLockfileAsync(path: string, options?: { ownerOnly: boolean }): Promise<Lockfile> {
  const port = portFromFilename(path);
  if (port === null) throw new Error(`lockfile name not <port>.lock: ${path}`);
  if (options?.ownerOnly && port === 0) throw new Error('bridge lockfile port is invalid');
  const source = options?.ownerOnly ? await readOwnerOnlyLockfile(path) : await readFile(path, 'utf8');
  return parseLockfile(path, port, source);
}

/**
 * Discover the most-recently-modified `<port>.lock` in `dir` (defaults to
 * `~/.lingxi/bridge`). Mirrors the Rust `discover_latest`: skips `.tmp` shadows
 * and non-`.lock` files, and returns `null` when nothing parses.
 */
export function discoverLatestLockfile(dir: string = defaultBridgeDir()): Lockfile | null {
  let entries: string[];
  try {
    entries = readdirSync(dir);
  } catch {
    return null;
  }

  let best: { mtimeMs: number; path: string } | null = null;
  for (const entry of entries) {
    if (portFromFilename(entry) === null) {
      continue;
    }
    const path = join(dir, entry);
    let mtimeMs: number;
    try {
      mtimeMs = statSync(path).mtimeMs;
    } catch {
      continue;
    }
    if (best === null || mtimeMs > best.mtimeMs) {
      best = { mtimeMs, path };
    }
  }

  if (best === null) {
    return null;
  }
  try {
    return readLockfile(best.path);
  } catch {
    return null;
  }
}

/** Asynchronous discovery with bounded filesystem concurrency and the same selection rule. */
export async function discoverLatestLockfileAsync(dir: string = defaultBridgeDir()): Promise<Lockfile | null> {
  let entries: string[];
  try { entries = await readdir(dir); } catch { return null; }
  let best: { mtimeMs: number; path: string } | null = null;
  for (let offset = 0; offset < entries.length; offset += 32) {
    const candidates = await Promise.all(entries.slice(offset, offset + 32).map(async entry => {
      if (portFromFilename(entry) === null) return null;
      const path = join(dir, entry);
      try { return { path, mtimeMs: (await stat(path)).mtimeMs }; } catch { return null; }
    }));
    for (const candidate of candidates) {
      if (candidate && (best === null || candidate.mtimeMs > best.mtimeMs)) best = candidate;
    }
  }
  if (!best) return null;
  try { return await readLockfileAsync(best.path); } catch { return null; }
}
