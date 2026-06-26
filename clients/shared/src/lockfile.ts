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

import { readdirSync, readFileSync, statSync } from 'node:fs';
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
  const body = JSON.parse(readFileSync(path, 'utf8')) as LockfileBody;
  if (typeof body.authToken !== 'string' || body.authToken.length === 0) {
    throw new Error(`lockfile missing authToken: ${path}`);
  }
  return { path, port, body };
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
