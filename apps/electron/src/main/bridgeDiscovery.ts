import { execFileSync } from 'node:child_process';
import { existsSync, lstatSync, readFileSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import type { BridgeManagerOptions, SessionRef } from './bridgeTypes.js';
import { isSessionId } from './sessionIdentity.js';
import { validateBridgeLockfile } from './validation.js';

const SERVER_BIN_NAME = process.platform === 'win32' ? 'bridge-server.exe' : 'bridge-server';

function moduleDir(): string {
  try {
    return dirname(fileURLToPath(import.meta.url));
  } catch {
    return process.cwd();
  }
}

function findWorkspaceServerBin(start: string): string | undefined {
  let dir = start;
  for (;;) {
    for (const profile of ['debug', 'release']) {
      const candidate = join(dir, 'target', profile, SERVER_BIN_NAME);
      if (existsSync(candidate)) return candidate;
    }
    const parent = dirname(dir);
    if (parent === dir) return undefined;
    dir = parent;
  }
}

/** Packaged builds use only process.resourcesPath; overrides/search are development-only. */
export function resolveServerBin(opts: Omit<BridgeManagerOptions, 'launchConfig'> = {}): string {
  if (opts.isPackaged) {
    const resourcesPath = opts.resourcesPath ?? process.resourcesPath;
    const packagedBinary = join(resourcesPath, 'bin', SERVER_BIN_NAME);
    if (existsSync(packagedBinary)) return packagedBinary;
    throw new Error(`packaged bridge-server is missing from application resources`);
  }
  const explicit = opts.serverBin ?? process.env['LINGXI_BRIDGE_SERVER_BIN'];
  if (explicit) return explicit;
  const found = findWorkspaceServerBin(moduleDir());
  if (found) return found;
  throw new Error('bridge-server binary not found; build it or set LINGXI_BRIDGE_SERVER_BIN in development');
}

export function lockfiles(dir: string): string[] {
  try {
    return readdirSync(dir).filter((name) => name.endsWith('.lock') && !name.startsWith('.'));
  } catch {
    return [];
  }
}

export interface ReusableBridge {
  launchDir: string;
  lockfilePath: string;
  pid: number;
  ownedProcess: boolean;
}

export interface ProcessCommand {
  pid: number;
  ppid?: number;
  command: string;
}

function privateOwnedPath(path: string, kind: 'directory' | 'file'): boolean {
  try {
    const metadata = lstatSync(path);
    if (kind === 'directory' ? !metadata.isDirectory() : !metadata.isFile()) return false;
    if (process.platform !== 'win32' && (metadata.mode & 0o077) !== 0) return false;
    if (process.getuid && metadata.uid !== process.getuid()) return false;
    return true;
  } catch {
    return false;
  }
}

export function processCommandOwnsSession(command: string, sessionId: string): boolean {
  if (!isSessionId(sessionId)) return false;
  const escaped = sessionId.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return new RegExp(`(?:^|\\s)--session-id(?:=|\\s+)${escaped}(?=\\s|$)`).test(command);
}

export function readProcessCommand(pid: number): string | undefined {
  if (process.platform === 'win32') return undefined;
  try {
    const command = execFileSync('/bin/ps', ['-p', String(pid), '-o', 'command='], {
      encoding: 'utf8',
      maxBuffer: 64 * 1024,
      timeout: 1_000,
    }).trim();
    return command || undefined;
  } catch {
    return undefined;
  }
}

export function listProcessCommands(): ProcessCommand[] {
  if (process.platform === 'win32') return [];
  try {
    return execFileSync('/bin/ps', ['-axww', '-o', 'pid=,ppid=,command='], {
      encoding: 'utf8',
      maxBuffer: 4 * 1024 * 1024,
      timeout: 2_000,
    }).split('\n').flatMap((line) => {
      const processInfo = parseProcessCommand(line);
      return processInfo ? [processInfo] : [];
    });
  } catch {
    return [];
  }
}

function parseProcessCommand(line: string): ProcessCommand | undefined {
  const match = /^\s*(\d+)\s+(\d+)\s+(.+)$/.exec(line);
  return match ? { pid: Number(match[1]), ppid: Number(match[2]), command: match[3] } : undefined;
}

function lockfilePort(name: string): number | undefined {
  if (!name.endsWith('.lock') || name.startsWith('.')) return undefined;
  const raw = name.slice(0, -'.lock'.length);
  const port = Number(raw);
  return Number.isInteger(port) && port >= 1 && port <= 65_535 && String(port) === raw
    ? port
    : undefined;
}

function reusableBridgeInLaunchDirectory(
  launchDir: string,
  ref: SessionRef,
  pid: number,
  ownedProcess: boolean,
): ReusableBridge | undefined {
  if (!privateOwnedPath(launchDir, 'directory')) return undefined;
  for (const name of lockfiles(launchDir)) {
    if (lockfilePort(name) === undefined) continue;
    const lockfilePath = join(launchDir, name);
    if (!privateOwnedPath(lockfilePath, 'file')) continue;
    try {
      const body = JSON.parse(readFileSync(lockfilePath, 'utf8')) as { pid?: unknown };
      validateBridgeLockfile(body, pid, ref.projectPath);
      return { launchDir, lockfilePath, pid, ownedProcess };
    } catch {
      // A stale or partially-written candidate is not reusable; keep looking.
    }
  }
  return undefined;
}

/**
 * Locate a still-running bridge-server previously detached from this exact
 * Desktop user-data root. The private lockfile authenticates the connection;
 * the process command supplies the missing session-id binding in the legacy
 * lockfile schema without weakening its Claude-compatible wire format.
 */
export function discoverReusableBridge(
  bridgeRoot: string,
  ref: SessionRef,
  processCommand: (pid: number) => string | undefined = readProcessCommand,
  isAlive: (pid: number) => boolean = processIsAlive,
): ReusableBridge | undefined {
  if (!privateOwnedPath(bridgeRoot, 'directory')) return undefined;
  let entries: string[];
  try {
    entries = readdirSync(bridgeRoot).sort((left, right) => right.localeCompare(left));
  } catch {
    return undefined;
  }
  for (const entry of entries) {
    if (!entry.startsWith('launch-')) continue;
    const launchDir = join(bridgeRoot, entry);
    if (!privateOwnedPath(launchDir, 'directory')) continue;
    for (const name of lockfiles(launchDir)) {
      if (lockfilePort(name) === undefined) continue;
      const lockfilePath = join(launchDir, name);
      if (!privateOwnedPath(lockfilePath, 'file')) continue;
      try {
        const body = JSON.parse(readFileSync(lockfilePath, 'utf8')) as { pid?: unknown };
        if (!Number.isSafeInteger(body.pid) || Number(body.pid) < 1) continue;
        const pid = Number(body.pid);
        // Crashed or force-quit runtimes leave their launch directories
        // behind; skip their pids without spawning `ps` for each one.
        if (!isAlive(pid)) continue;
        const command = processCommand(pid);
        if (!command || !processCommandOwnsSession(command, ref.sessionId)) continue;
        const reusable = reusableBridgeInLaunchDirectory(launchDir, ref, pid, true);
        if (reusable) return reusable;
      } catch {
        // A stale or partially-written candidate is not reusable; keep looking.
      }
    }
  }
  return undefined;
}

function argumentBetween(command: string, start: string, end: string): string | undefined {
  const startIndex = command.indexOf(start);
  if (startIndex < 0) return undefined;
  const valueStart = startIndex + start.length;
  const endIndex = command.indexOf(end, valueStart);
  if (endIndex < 0) return undefined;
  const value = command.slice(valueStart, endIndex).trim();
  return value || undefined;
}

export function discoverExternalReusableBridge(
  ref: SessionRef,
  processes: readonly ProcessCommand[] = listProcessCommands(),
): ReusableBridge | undefined {
  for (const processInfo of processes) {
    if (!processCommandOwnsSession(processInfo.command, ref.sessionId)) continue;
    const workspace = argumentBetween(processInfo.command, ' --cwd ', ' --bridge-dir ');
    const launchDir = argumentBetween(processInfo.command, ' --bridge-dir ', ' --session-id ');
    if (workspace !== ref.projectPath || !launchDir) continue;
    const reusable = reusableBridgeInLaunchDirectory(
      launchDir,
      ref,
      processInfo.pid,
      processInfo.ppid === 1,
    );
    if (reusable) return reusable;
  }
  return undefined;
}

export function processIsAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return (error as NodeJS.ErrnoException).code === 'EPERM';
  }
}

export function ignorableLockfileError(input: unknown): boolean {
  if (!(input instanceof Error)) return false;
  return (input as NodeJS.ErrnoException).code === 'ENOENT'
    || input instanceof SyntaxError
    || /bridge lockfile|JSON/.test(input.message);
}
