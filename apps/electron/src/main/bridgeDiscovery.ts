import { execFile } from 'node:child_process';
import { existsSync } from 'node:fs';
import { lstat, readdir } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { readLockfileAsync } from '@lingxi/bridge-client';
import type { BridgeManagerOptions, SessionRef } from './bridgeTypes.js';
import { isSessionId } from './sessionIdentity.js';
import { validateBridgeLockfile } from './validation.js';

const SERVER_BIN_NAME = process.platform === 'win32' ? 'bridge-server.exe' : 'bridge-server';
const execFileAsync = promisify(execFile);

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

export async function lockfiles(dir: string): Promise<string[]> {
  try {
    return (await readdir(dir)).filter((name) => name.endsWith('.lock') && !name.startsWith('.'));
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

async function isPrivateBridgeDirectory(path: string): Promise<boolean> {
  try {
    const metadata = await lstat(path);
    if (!metadata.isDirectory()) return false;
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

async function readProcessCommand(pid: number): Promise<string | undefined> {
  if (process.platform === 'win32') return undefined;
  try {
    const { stdout } = await execFileAsync('/bin/ps', ['-p', String(pid), '-o', 'command='], {
      encoding: 'utf8',
      maxBuffer: 64 * 1024,
      timeout: 1_000,
    });
    return stdout.trim() || undefined;
  } catch {
    return undefined;
  }
}

async function listProcessCommands(): Promise<ProcessCommand[]> {
  if (process.platform === 'win32') return [];
  try {
    const { stdout } = await execFileAsync('/bin/ps', ['-axww', '-o', 'pid=,ppid=,command='], {
      encoding: 'utf8',
      maxBuffer: 4 * 1024 * 1024,
      timeout: 2_000,
    });
    return stdout.split('\n').flatMap((line) => {
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

async function reusableBridgeInLaunchDirectory(
  launchDir: string,
  ref: SessionRef,
  pid: number,
  ownedProcess: boolean,
): Promise<ReusableBridge | undefined> {
  if (!await isPrivateBridgeDirectory(launchDir)) return undefined;
  for (const name of await lockfiles(launchDir)) {
    const lockfilePath = join(launchDir, name);
    try {
      const { body } = await readLockfileAsync(lockfilePath, { ownerOnly: true });
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
export async function discoverReusableBridge(
  bridgeRoot: string,
  ref: SessionRef,
  processCommand: NonNullable<BridgeManagerOptions['readProcessCommand']> = readProcessCommand,
  isAlive: (pid: number) => boolean = processIsAlive,
): Promise<ReusableBridge | undefined> {
  if (!await isPrivateBridgeDirectory(bridgeRoot)) return undefined;
  let entries: string[];
  try {
    entries = (await readdir(bridgeRoot)).sort((left, right) => right.localeCompare(left));
  } catch {
    return undefined;
  }
  for (const entry of entries) {
    if (!entry.startsWith('launch-')) continue;
    const launchDir = join(bridgeRoot, entry);
    if (!await isPrivateBridgeDirectory(launchDir)) continue;
    for (const name of await lockfiles(launchDir)) {
      const lockfilePath = join(launchDir, name);
      try {
        const { body } = await readLockfileAsync(lockfilePath, { ownerOnly: true });
        const pid = body.pid;
        if (!Number.isSafeInteger(pid) || pid < 1) continue;
        // Crashed or force-quit runtimes leave their launch directories
        // behind; skip their pids without spawning `ps` for each one.
        if (!isAlive(pid)) continue;
        const command = await processCommand(pid);
        if (!command || !processCommandOwnsSession(command, ref.sessionId)) continue;
        const reusable = await reusableBridgeInLaunchDirectory(launchDir, ref, pid, true);
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

export async function discoverExternalReusableBridge(
  ref: SessionRef,
  processes: readonly ProcessCommand[],
): Promise<ReusableBridge | undefined> {
  for (const processInfo of processes) {
    if (!processCommandOwnsSession(processInfo.command, ref.sessionId)) continue;
    const workspace = argumentBetween(processInfo.command, ' --cwd ', ' --bridge-dir ');
    const launchDir = argumentBetween(processInfo.command, ' --bridge-dir ', ' --session-id ');
    if (workspace !== ref.projectPath || !launchDir) continue;
    const reusable = await reusableBridgeInLaunchDirectory(
      launchDir,
      ref,
      processInfo.pid,
      processInfo.ppid === 1,
    );
    if (reusable) return reusable;
  }
  return undefined;
}

/** Inspect the process table once for both private and external candidates. */
export async function discoverSessionBridge(
  bridgeRoot: string,
  ref: SessionRef,
  options: Pick<BridgeManagerOptions, 'listProcessCommands' | 'readProcessCommand'>,
): Promise<ReusableBridge | undefined> {
  let snapshot: Promise<readonly ProcessCommand[]> | undefined;
  const getProcesses = () => snapshot ??= Promise.resolve((options.listProcessCommands ?? listProcessCommands)());
  let commandsByPid: Map<number, string> | undefined;
  const readCapturedCommand = async (pid: number): Promise<string | undefined> => {
    commandsByPid ??= new Map((await getProcesses()).map(entry => [entry.pid, entry.command]));
    // The table may exceed its size budget, or a candidate may have started after capture.
    return commandsByPid.get(pid) ?? readProcessCommand(pid);
  };
  // Keep the external process-list override independent of private PID inspection.
  const readCommand = options.readProcessCommand ?? (options.listProcessCommands ? readProcessCommand : readCapturedCommand);
  const local = await discoverReusableBridge(bridgeRoot, ref, readCommand);
  return local ?? discoverExternalReusableBridge(ref, await getProcesses());
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
    || (input as NodeJS.ErrnoException).code === 'ELOOP'
    || input instanceof SyntaxError
    || /bridge lockfile|^lockfile (?:name not|missing authToken)|JSON/.test(input.message);
}
