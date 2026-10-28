import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { realpathSync, statSync } from 'node:fs';
import { stat } from 'node:fs/promises';
import { basename, isAbsolute, join, win32 } from 'node:path';
import { StringDecoder } from 'node:string_decoder';
import { resolveServerBin } from './bridgeDiscovery.js';
import type { TerminalScope, TerminalSnapshot, TerminalEvent } from '../shared/terminal.js';

const MAX_OUTPUT = 1024 * 1024;
const MAX_FRAME = 256 * 1024;
const MAX_REQUEST = 512 * 1024;
const MAX_PENDING = 128;
const MAX_TERMINALS = 32;
const MAX_SCOPE_TERMINALS = 8;

/** Deliberately excludes model credentials, plugin secrets and loader injection variables. */
export function terminalEnvironment(source: NodeJS.ProcessEnv = process.env, platform: NodeJS.Platform = process.platform): NodeJS.ProcessEnv {
  const result: NodeJS.ProcessEnv = {};
  const windowsSource = platform === 'win32' ? new Map(Object.entries(source).map(([key, value]) => [key.toUpperCase(), value])) : undefined;
  for (const key of ['PATH', 'HOME', 'USER', 'LOGNAME', 'SHELL', 'LANG', 'LC_ALL', 'LC_CTYPE', 'TMPDIR', 'TMP', 'TEMP', 'TZ', 'SystemRoot', 'SYSTEMROOT', 'WINDIR', 'COMSPEC', 'ComSpec', 'PATHEXT', 'USERPROFILE', 'APPDATA', 'LOCALAPPDATA', 'HOMEDRIVE', 'HOMEPATH', 'SSH_AUTH_SOCK']) {
    const value = windowsSource ? windowsSource.get(key.toUpperCase()) : source[key];
    if (value !== undefined) result[key] = value;
  }
  result.TERM = 'xterm-256color';
  result.COLORTERM = 'truecolor';
  return result;
}

export interface TerminalManagerOptions {
  serverBin?: string;
  isPackaged?: boolean;
  resourcesPath?: string;
  spawnProcess?: typeof spawn;
  requestTimeoutMs?: number;
  windowsShellFileExists?: (path: string) => boolean | Promise<boolean>;
  windowsShellSearchTimeoutMs?: number;
}

/** Match Windows PATH unquoting before resolving entries in the original cwd. */
export function windowsShellSearchPaths(path: string, cwd: string): string[] {
  const entries: string[] = [];
  let entry = ''; let quoted = false;
  for (const character of path) {
    if (character === '"') quoted = !quoted;
    else if (character === ';' && !quoted) { entries.push(entry); entry = ''; }
    else entry += character;
  }
  entries.push(entry);
  return entries.map(value => {
    const driveRelative = /^([a-z]):(?![\\/])/i.exec(value);
    if (!driveRelative) return win32.resolve(cwd, value);
    const launchDrive = /([a-z]):[\\/]$/i.exec(win32.parse(cwd).root);
    const base = launchDrive?.[1].toUpperCase() === driveRelative[1].toUpperCase() ? cwd : `${driveRelative[1]}:\\`;
    // The broker's explicit environment does not inherit Windows' hidden
    // per-drive cwd entries, so another drive resolves from its root.
    return win32.resolve(base, value.slice(2));
  });
}

const activeWindowsShellProbes = new Set<Promise<void>>();
const windowsShellProbeWaiters = new Set<() => void>();
const MAX_WINDOWS_SHELL_PROBES = 2;

function beforeShellSearchDeadline<T>(promise: Promise<T>, deadline: number, signal?: AbortSignal): Promise<T> {
  if (signal?.aborted) return Promise.reject(signal.reason instanceof Error ? signal.reason : new Error('Terminal creation cancelled'));
  return new Promise((resolve, reject) => {
    const finish = () => { clearTimeout(timeout); signal?.removeEventListener('abort', abort); };
    const abort = () => { finish(); reject(signal?.reason instanceof Error ? signal.reason : new Error('Terminal creation cancelled')); };
    const timeout = setTimeout(() => { finish(); reject(new Error('Windows terminal shell search timed out')); }, Math.max(0, deadline - Date.now()));
    signal?.addEventListener('abort', abort, { once: true });
    promise.then(value => { finish(); resolve(value); }, error => { finish(); reject(error); });
  });
}

async function probeWindowsShell(candidate: string, isFile: (path: string) => boolean | Promise<boolean>, deadline: number, signal?: AbortSignal): Promise<boolean> {
  while (activeWindowsShellProbes.size >= MAX_WINDOWS_SHELL_PROBES) {
    // Do not attach a Promise.race reaction to a potentially permanent UNC
    // probe on every retry: timed-out waiters must be removable themselves.
    await new Promise<void>((resolve, reject) => {
      const cleanup = () => { clearTimeout(timeout); signal?.removeEventListener('abort', abort); windowsShellProbeWaiters.delete(ready); };
      const ready = () => { cleanup(); resolve(); };
      const abort = () => { cleanup(); reject(signal?.reason instanceof Error ? signal.reason : new Error('Terminal creation cancelled')); };
      const timeout = setTimeout(() => { cleanup(); reject(new Error('Windows terminal shell search timed out')); }, Math.max(0, deadline - Date.now()));
      windowsShellProbeWaiters.add(ready);
      signal?.addEventListener('abort', abort, { once: true });
      if (signal?.aborted) abort();
    });
  }
  if (signal?.aborted) throw signal.reason instanceof Error ? signal.reason : new Error('Terminal creation cancelled');
  if (Date.now() >= deadline) throw new Error('Windows terminal shell search timed out');
  const result = Promise.resolve().then(() => isFile(candidate)).catch(() => false);
  const slot = result.then(() => undefined);
  activeWindowsShellProbes.add(slot);
  void slot.then(() => {
    activeWindowsShellProbes.delete(slot);
    for (const ready of windowsShellProbeWaiters) ready();
  });
  // A timed-out filesystem call keeps its slot until it actually finishes,
  // bounding stuck UNC probes without blocking Electron's main thread.
  return beforeShellSearchDeadline(result, deadline, signal);
}

export async function windowsShellExecutable(
  path: string | undefined,
  cwd: string,
  isFile: (path: string) => boolean | Promise<boolean> = async candidate => {
    try { return (await stat(candidate)).isFile(); } catch { return false; }
  },
  timeoutMs = 5_000,
  signal?: AbortSignal,
): Promise<string | undefined> {
  if (signal?.aborted) throw signal.reason instanceof Error ? signal.reason : new Error('Terminal creation cancelled');
  const deadline = Date.now() + timeoutMs;
  // Windows split_paths treats a missing PATH like an empty entry, which
  // searches the original cwd. Preserve that selection without setting PATH.
  const directories = windowsShellSearchPaths(path ?? '', cwd);
  for (const name of ['pwsh.exe', 'powershell.exe']) {
    for (const directory of directories) {
      const candidate = win32.join(directory, name);
      if (await probeWindowsShell(candidate, isFile, deadline, signal)) return candidate;
    }
  }
  return undefined;
}

/** Add only the packaged application's native loader paths to the broker.
 * The Rust broker restores the original Windows PATH for its interactive shell. */
export function terminalBrokerEnvironment(
  options: Pick<TerminalManagerOptions, 'isPackaged' | 'resourcesPath'>,
  source: NodeJS.ProcessEnv = process.env,
  platform: NodeJS.Platform = process.platform,
): NodeJS.ProcessEnv {
  const environment = terminalEnvironment(source, platform);
  if (!options.isPackaged) return environment;
  const resourcesPath = options.resourcesPath ?? process.resourcesPath;
  if (!resourcesPath || !isAbsolute(resourcesPath)) throw new Error('Packaged terminal resources path must be absolute');
  if (platform === 'linux') {
    // The executable is resources/bin/bridge-server. glibc splits this single
    // entry before expanding $ORIGIN, so separators in the install path are
    // not interpreted as additional loader search directories.
    environment.LD_LIBRARY_PATH = '$ORIGIN/../gstreamer/lib';
  } else if (platform === 'win32') {
    if (resourcesPath.includes(';')) throw new Error('Packaged terminal resources path cannot contain a Windows PATH separator');
    // Only the app-owned directory is configured in PATH; the Windows DLL
    // loader also searches its built-in system locations. The broker restores
    // the user's original PATH when launching an interactive shell.
    environment.DESKTOP_TERMINAL_SHELL_PATH_PRESENT = environment.PATH === undefined ? '0' : '1';
    if (environment.PATH !== undefined) environment.DESKTOP_TERMINAL_SHELL_PATH = environment.PATH;
    environment.PATH = join(resourcesPath, 'gstreamer', 'bin');
  }
  return environment;
}

export function terminalBrokerLaunchOptions(
  options: Pick<TerminalManagerOptions, 'isPackaged' | 'resourcesPath'>,
  source: NodeJS.ProcessEnv = process.env,
  platform: NodeJS.Platform = process.platform,
): { env: NodeJS.ProcessEnv; cwd?: string } {
  const env = terminalBrokerEnvironment(options, source, platform);
  const cwd = options.isPackaged && platform === 'win32'
    ? join(options.resourcesPath ?? process.resourcesPath, 'bin') : undefined;
  return { env, cwd };
}

function sameScope(a: TerminalScope, b: TerminalScope): boolean {
  return a.projectPath === b.projectPath && a.sessionId === b.sessionId;
}
function clone(row: TerminalSnapshot): TerminalSnapshot { return { ...row, scope: { ...row.scope } }; }
function validateScope(scope: TerminalScope): TerminalScope {
  if (!scope || typeof scope.projectPath !== 'string' || !isAbsolute(scope.projectPath) || scope.projectPath.includes('\0') || typeof scope.sessionId !== 'string' || !scope.sessionId || scope.sessionId.length > 256 || scope.sessionId.includes('\0')) throw new Error('Invalid terminal scope');
  const projectPath = realpathSync(scope.projectPath);
  if (!statSync(projectPath).isDirectory()) throw new Error('Terminal directory does not exist');
  return { projectPath, sessionId: scope.sessionId };
}
function trimOutput(value: string): string {
  if (value.length <= MAX_OUTPUT) return value;
  let start = value.length - MAX_OUTPUT;
  const code = value.charCodeAt(start);
  if (code >= 0xdc00 && code <= 0xdfff) start++;
  return value.slice(start);
}

/** A dedicated local PTY broker. It never uses the model bridge or conversation history. */
export class TerminalManager {
  private child?: ChildProcessWithoutNullStreams;
  private disposed = false;
  private disposal?: Promise<void>;
  private rows = new Map<string, TerminalSnapshot>();
  private brokerTerminalIds = new Set<string>();
  private creating = new Map<string, AbortController>();
  private closing = new Map<string, Promise<void>>();
  private stopped = new WeakMap<ChildProcessWithoutNullStreams, Promise<void>>();
  private listeners = new Set<(event: TerminalEvent) => void>();
  private pending = new Map<string, { resolve: () => void; reject: (error: Error) => void; timer: ReturnType<typeof setTimeout> }>();
  private writes: Promise<void> = Promise.resolve();
  private outputPauseOwners = new Set<string>();
  private resumeOutput?: () => void;
  private windowsShellContext?: { path: string | undefined; cwd: string };
  constructor(private readonly options: TerminalManagerOptions = {}) {}

  get(id: string): TerminalSnapshot | undefined { const row = this.rows.get(id); return row && clone(row); }
  list(scope: TerminalScope): TerminalSnapshot[] { return [...this.rows.values()].filter(row => sameScope(row.scope, scope)).map(clone); }
  onEvent(listener: (event: TerminalEvent) => void): () => void { this.listeners.add(listener); return () => this.listeners.delete(listener); }
  /** Backpressure from renderer delivery. Reading resumes only after all owners release it. */
  setOutputPaused(owner: string, paused: boolean): void {
    if (paused && !this.disposed) this.outputPauseOwners.add(owner);
    else this.outputPauseOwners.delete(owner);
    if (this.outputPauseOwners.size === 0) { this.resumeOutput?.(); this.resumeOutput = undefined; }
  }
  private async waitForOutput(): Promise<void> {
    while (this.outputPauseOwners.size > 0 && !this.disposed) await new Promise<void>(resolve => { this.resumeOutput = resolve; });
  }
  private releaseOutput(): void {
    this.outputPauseOwners.clear(); this.resumeOutput?.(); this.resumeOutput = undefined;
  }
  private emit(event: TerminalEvent): void { for (const listener of this.listeners) { try { listener(event); } catch { /* A detached window must not stop the PTY reader. */ } } }

  async create(inputScope: TerminalScope): Promise<TerminalSnapshot> {
    if (this.disposed) throw new Error('Terminal manager is disposed');
    const scope = validateScope(inputScope);
    // A create may fail while its close is still in flight. Keep that close
    // counted until it settles so the reserved broker queue stays bounded.
    if (new Set([...this.rows.keys(), ...this.closing.keys()]).size >= MAX_TERMINALS || this.list(scope).length >= MAX_SCOPE_TERMINALS) throw new Error('Terminal limit reached');
    const id = randomUUID();
    const row: TerminalSnapshot = { id, scope, title: basename(scope.projectPath) || scope.projectPath, status: 'running', exitCode: null, output: '', sequence: 0 };
    this.rows.set(id, row);
    const controller = new AbortController();
    this.creating.set(id, controller);
    try {
      const command: Record<string, unknown> = { kind: 'create', terminalId: id, cwd: scope.projectPath, cols: 80, rows: 24 };
      if (this.options.isPackaged && process.platform === 'win32') {
        // The renderer supplies only a scope. Shell selection stays in Desktop
        // main and is recomputed against the launch-time PATH for each create.
        const broker = this.start();
        const context = this.windowsShellContext!;
        command.shellSelection = true;
        const selected = await windowsShellExecutable(context.path, context.cwd, this.options.windowsShellFileExists, this.options.windowsShellSearchTimeoutMs, controller.signal);
        if (this.child !== broker) throw new Error('Terminal service stopped during shell selection');
        if (selected) command.shellProgram = selected;
        else command.shellLaunchCwd = context.cwd;
      }
      if (this.disposed || controller.signal.aborted || this.rows.get(id) !== row) throw new Error('Terminal creation cancelled');
      this.brokerTerminalIds.add(id);
      await this.request(command);
      if (this.disposed || controller.signal.aborted || this.rows.get(id) !== row) throw new Error('Terminal creation cancelled');
    }
    catch (error) { this.rows.delete(id); this.brokerTerminalIds.delete(id); throw error; }
    finally { this.creating.delete(id); }
    return clone(row);
  }
  async input(id: string, data: string): Promise<void> {
    this.running(id);
    if (typeof data !== 'string' || Buffer.byteLength(data) > 64 * 1024) throw new Error('Terminal input is too large');
    await this.request({ kind: 'input', terminalId: id, data });
  }
  async resize(id: string, cols: number, rows: number): Promise<void> {
    this.running(id);
    if (!Number.isInteger(cols) || !Number.isInteger(rows) || cols < 1 || rows < 1 || cols > 1000 || rows > 1000) throw new Error('Invalid terminal dimensions');
    await this.request({ kind: 'resize', terminalId: id, cols, rows });
  }
  close(id: string): Promise<void> {
    const existing = this.closing.get(id);
    if (existing) return existing;
    const operation = this.closeInner(id);
    this.closing.set(id, operation);
    const cleanup = () => { if (this.closing.get(id) === operation) this.closing.delete(id); };
    void operation.then(cleanup, cleanup);
    return operation;
  }
  private async closeInner(id: string): Promise<void> {
    const row = this.rows.get(id);
    if (!row) return;
    this.creating.get(id)?.abort(new Error('Terminal creation cancelled'));
    const child = this.child;
    try {
      if (this.brokerTerminalIds.has(id)) await this.request({ kind: 'close', terminalId: id });
    }
    catch (error) {
      // Exited handles may already have been evicted by a later create. Any
      // other error leaves ownership uncertain, so stop the broker instead of
      // silently forgetting a potentially live terminal.
      if (!(error instanceof Error && error.message === 'unknown terminal')) {
        if (child) {
          this.fail(child, new Error('Terminal service close failed'));
          await this.waitForStop(child);
        }
        throw error;
      }
    }
    this.brokerTerminalIds.delete(id);
    this.rows.delete(id);
    this.emit({ kind: 'closed', terminalId: id });
  }
  async closeScope(scope: TerminalScope): Promise<void> { for (const row of this.list(scope)) await this.close(row.id); }
  async closeProject(projectPath: string): Promise<void> { for (const row of [...this.rows.values()]) if (row.scope.projectPath === projectPath) await this.close(row.id); }
  /** Host-only ownership transfer for drafts, alias restores and directory changes.
   * Existing shells retain their process state and cwd; new terminals use the new scope. */
  migrateScope(from: TerminalScope, inputTo: TerminalScope): void {
    const to = validateScope(inputTo);
    for (const row of this.rows.values()) if (sameScope(row.scope, from)) {
      row.scope = { ...to };
      this.emit({ kind: 'scope', terminalId: row.id, scope: { ...to } });
    }
  }
  private running(id: string): TerminalSnapshot {
    const row = this.rows.get(id);
    if (!row || row.status !== 'running') throw new Error('Terminal is not running');
    return row;
  }
  private start(): ChildProcessWithoutNullStreams {
    if (this.disposed) throw new Error('Terminal manager is disposed');
    if (this.child) return this.child;
    if (this.options.isPackaged && process.platform === 'win32') this.windowsShellContext = { path: process.env.PATH, cwd: process.cwd() };
    const child = (this.options.spawnProcess ?? spawn)(resolveServerBin(this.options), ['--desktop-terminal'], { stdio: ['pipe', 'pipe', 'pipe'], ...terminalBrokerLaunchOptions(this.options), windowsHide: true }) as ChildProcessWithoutNullStreams;
    this.child = child;
    this.stopped.set(child, new Promise(resolve => child.once('close', () => resolve())));
    child.stderr.resume(); // Diagnostics are deliberately kept out of terminal output and secrets are not logged.
    child.on('error', () => this.fail(child, new Error('Terminal service could not start')));
    child.on('close', () => this.fail(child, new Error('Terminal service stopped'), true));
    child.stdin.on('error', () => this.fail(child, new Error('Terminal service input closed')));
    void this.read(child);
    return child;
  }
  private request(command: Record<string, unknown>): Promise<void> {
    // Closing must remain possible when ordinary requests fill the queue.
    // In-flight closes count against the terminal limit, and close()
    // deduplicates concurrent requests for the same id.
    const limit = command.kind === 'close' ? MAX_PENDING + MAX_TERMINALS : MAX_PENDING;
    if (this.pending.size >= limit) return Promise.reject(new Error('Too many pending terminal operations'));
    let child: ChildProcessWithoutNullStreams;
    try { child = this.start(); } catch (error) { return Promise.reject(error); }
    const requestId = randomUUID();
    const line = JSON.stringify({ ...command, requestId }) + '\n';
    if (Buffer.byteLength(line) > MAX_REQUEST) return Promise.reject(new Error('Terminal service request exceeds limit'));
    const completion = new Promise<void>((resolve, reject) => {
      const onTimeout = () => {
        if (this.outputPauseOwners.size > 0) {
          const pending = this.pending.get(requestId);
          if (pending) { pending.timer = setTimeout(onTimeout, this.options.requestTimeoutMs ?? 15_000); pending.timer.unref?.(); }
        } else this.fail(child, new Error('Terminal service timed out'));
      };
      const timer = setTimeout(onTimeout, this.options.requestTimeoutMs ?? 15_000);
      timer.unref?.();
      this.pending.set(requestId, { resolve, reject, timer });
    });
    this.writes = this.writes.then(() => {
      if (this.child !== child || !this.pending.has(requestId)) return;
      return new Promise<void>((resolve, reject) => child.stdin.write(line, error => error ? reject(error) : resolve()));
    }).catch(() => this.fail(child, new Error('Terminal service input failed')));
    return completion;
  }
  private async read(child: ChildProcessWithoutNullStreams): Promise<void> {
    const decoder = new StringDecoder('utf8');
    let buffer = '';
    try {
      for await (const chunk of child.stdout) {
        await this.waitForOutput();
        if (this.child !== child) return;
        buffer += decoder.write(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
        let newline: number;
        while ((newline = buffer.indexOf('\n')) >= 0) {
          await this.waitForOutput();
          if (newline > MAX_FRAME) throw new Error('Terminal service frame exceeds limit');
          const line = buffer.slice(0, newline);
          buffer = buffer.slice(newline + 1);
          if (this.child !== child) return;
          if (line) this.message(JSON.parse(line));
        }
        if (buffer.length > MAX_FRAME) throw new Error('Terminal service frame exceeds limit');
      }
      buffer += decoder.end();
      if (buffer.trim()) throw new Error('Incomplete terminal service frame');
    } catch { this.fail(child, new Error('Invalid terminal service output')); }
  }
  private message(message: Record<string, unknown>): void {
    if (!message || typeof message !== 'object') throw new Error('Invalid terminal event');
    if (message.kind === 'response' && typeof message.requestId === 'string') {
      const pending = this.pending.get(message.requestId);
      if (!pending) return;
      clearTimeout(pending.timer); this.pending.delete(message.requestId);
      if (message.error !== undefined && message.error !== null) pending.reject(new Error(typeof message.error === 'string' ? message.error.slice(0, 1024) : 'Terminal operation failed'));
      else pending.resolve();
      return;
    }
    // The broker emits this host-only frame after EOF cleanup. It is not a
    // terminal event and must not be forwarded to renderer windows.
    if (message.kind === 'shutdown' && message.reason === 'eof') return;
    if (typeof message.terminalId !== 'string') throw new Error('Invalid terminal identity');
    const row = this.rows.get(message.terminalId);
    if (!row) return;
    if (message.kind === 'output' && typeof message.data === 'string') {
      row.output = trimOutput(row.output + message.data); row.sequence++;
      this.emit({ kind: 'output', terminalId: row.id, data: message.data, sequence: row.sequence });
    } else if (message.kind === 'exit' && (message.exitCode === null || Number.isSafeInteger(message.exitCode))) {
      row.status = 'exited'; row.exitCode = message.exitCode as number | null;
      this.emit({ kind: 'exit', terminalId: row.id, exitCode: row.exitCode });
    }
    // Anything else is a frame this build does not know how to render. Throwing
    // here reaches `read`'s catch, which calls `fail()` — killing the broker,
    // rejecting every pending request and marking EVERY terminal in EVERY
    // project exited. A broker that grows a frame kind (the protocol already
    // declares `reset`) must not be able to take the whole service down, so an
    // unrecognised frame is ignored. Genuinely corrupt framing is still caught
    // by the JSON/length checks in `read`.
  }
  private async waitForStop(child: ChildProcessWithoutNullStreams): Promise<void> {
    const stopped = this.stopped.get(child);
    if (!stopped) throw new Error('Terminal service stop could not be confirmed');
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      await Promise.race([stopped, new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error('Terminal service stop could not be confirmed')), 8_000); })]);
    } finally { if (timer) clearTimeout(timer); }
  }
  private fail(child: ChildProcessWithoutNullStreams, error: Error, alreadyClosed = false): void {
    if (this.child !== child) return;
    this.child = undefined;
    for (const controller of this.creating.values()) controller.abort(error);
    this.releaseOutput();
    this.brokerTerminalIds.clear();
    // EOF gives the broker time to terminate/reap PTY process trees even after a protocol error.
    if (!alreadyClosed) {
      const killTimer = setTimeout(() => child.kill(), 6000);
      killTimer.unref?.();
      child.once('close', () => clearTimeout(killTimer));
      child.stdin.end();
    }
    for (const pending of this.pending.values()) { clearTimeout(pending.timer); pending.reject(error); }
    this.pending.clear();
    for (const row of this.rows.values()) if (row.status === 'running') {
      row.status = 'exited'; row.exitCode = null;
      this.emit({ kind: 'exit', terminalId: row.id, exitCode: null });
    }
  }
  dispose(): Promise<void> {
    this.disposal ??= this.disposeInner();
    return this.disposal;
  }
  private async disposeInner(): Promise<void> {
    if (this.disposed) return;
    this.disposed = true;
    for (const controller of this.creating.values()) controller.abort(new Error('Terminal creation cancelled'));
    this.releaseOutput();
    for (const pending of this.pending.values()) { clearTimeout(pending.timer); pending.reject(new Error('Terminal manager disposed')); }
    this.pending.clear();
    const child = this.child;
    if (child) {
      // EOF is the broker's graceful shutdown protocol, which kills/reaps all PTY children.
      child.stdin.end();
      await new Promise<void>(resolve => {
        const timer = setTimeout(() => { child.kill(); resolve(); }, 6000);
        child.once('close', () => { clearTimeout(timer); resolve(); });
      });
      this.fail(child, new Error('Terminal manager disposed'));
    }
    this.rows.clear(); this.brokerTerminalIds.clear(); this.listeners.clear();
  }
}
