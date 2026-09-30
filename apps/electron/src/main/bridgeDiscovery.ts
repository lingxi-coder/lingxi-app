import { execFileSync } from 'node:child_process';
import { existsSync, lstatSync, readFileSync, readdirSync } from 'node:fs';
import { dirname, isAbsolute, join, relative, resolve } from 'node:path';
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
      const match = /^\s*(\d+)\s+(\d+)\s+(.+)$/.exec(line);
      return match ? [{ pid: Number(match[1]), ppid: Number(match[2]), command: match[3] }] : [];
    });
  } catch {
    return [];
  }
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

function processCommandArgument(command: string, name: string): string | undefined {
  const marker = ` --${name} `;
  const startIndex = command.indexOf(marker);
  if (startIndex < 0) return undefined;
  const valueStart = startIndex + marker.length;
  const nextFlag = command.indexOf(' --', valueStart);
  const value = command.slice(valueStart, nextFlag < 0 ? undefined : nextFlag).trim();
  return value || undefined;
}

function processCommandHasFlag(command: string, name: string): boolean {
  const escaped = name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return new RegExp(`(?:^|\\s)--${escaped}(?=\\s|$)`).test(command);
}

function pathIsWithin(root: string, candidate: string): boolean {
  const relativePath = relative(resolve(root), resolve(candidate));
  return relativePath === '' || (!relativePath.startsWith('..') && !isAbsolute(relativePath));
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

/**
 * Find only pre-broker Desktop bridges that launchd inherited after their
 * parent exited. Current packaged bridges always carry the stdin-only marker;
 * constraining candidates to this private Desktop root avoids terminating CLI,
 * TUI, test, or another app installation's background sessions.
 */
export function discoverLegacyOrphanBridges(
  bridgeRoot: string,
  processes: readonly ProcessCommand[] = listProcessCommands(),
): ReusableBridge[] {
  if (!privateOwnedPath(bridgeRoot, 'directory')) return [];
  const candidates: ReusableBridge[] = [];
  const seen = new Set<number>();
  for (const processInfo of processes) {
    if (processInfo.ppid !== 1 || seen.has(processInfo.pid)) continue;
    if (processCommandHasFlag(processInfo.command, 'packaged-credential-stdin-only')) continue;
    const workspace = processCommandArgument(processInfo.command, 'cwd');
    const launchDir = processCommandArgument(processInfo.command, 'bridge-dir');
    const sessionId = processCommandArgument(processInfo.command, 'session-id');
    if (!workspace || !launchDir || !sessionId || !isSessionId(sessionId)) continue;
    if (!pathIsWithin(bridgeRoot, launchDir)) continue;
    if (!processCommandOwnsSession(processInfo.command, sessionId)) continue;
    const reusable = reusableBridgeInLaunchDirectory(
      launchDir,
      { projectPath: workspace, sessionId },
      processInfo.pid,
      true,
    );
    if (!reusable) continue;
    seen.add(processInfo.pid);
    candidates.push(reusable);
  }
  return candidates;
}

interface StopLegacyOrphanBridgesOptions {
  processes?: readonly ProcessCommand[];
  signalProcess?: (pid: number, signal: NodeJS.Signals) => boolean;
  processIsAlive?: (pid: number) => boolean;
  wait?: (milliseconds: number) => Promise<void>;
  stopTimeoutMs?: number;
}

function signalProcessGroup(pid: number, signal: NodeJS.Signals): boolean {
  try {
    process.kill(-pid, signal);
    return true;
  } catch {
    try {
      process.kill(pid, signal);
      return true;
    } catch {
      return false;
    }
  }
}

export function processIsAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return (error as NodeJS.ErrnoException).code === 'EPERM';
  }
}

export async function stopLegacyOrphanBridges(
  bridgeRoot: string,
  options: StopLegacyOrphanBridgesOptions = {},
): Promise<number[]> {
  const signalProcess = options.signalProcess ?? signalProcessGroup;
  const isAlive = options.processIsAlive ?? processIsAlive;
  const wait = options.wait ?? ((milliseconds: number) => new Promise<void>((resolveWait) => {
    const timer = setTimeout(resolveWait, milliseconds);
    timer.unref();
  }));
  const stopped: number[] = [];
  for (const candidate of discoverLegacyOrphanBridges(bridgeRoot, options.processes)) {
    if (!signalProcess(candidate.pid, 'SIGINT')) continue;
    const deadline = Date.now() + (options.stopTimeoutMs ?? 1_000);
    while (isAlive(candidate.pid) && Date.now() < deadline) await wait(50);
    if (isAlive(candidate.pid)) {
      signalProcess(candidate.pid, 'SIGKILL');
      await wait(25);
    }
    if (!isAlive(candidate.pid)) stopped.push(candidate.pid);
  }
  return stopped;
}

export function ignorableLockfileError(input: unknown): boolean {
  if (!(input instanceof Error)) return false;
  return (input as NodeJS.ErrnoException).code === 'ENOENT'
    || input instanceof SyntaxError
    || /bridge lockfile|JSON/.test(input.message);
}
