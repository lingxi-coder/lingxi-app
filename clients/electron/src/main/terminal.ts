import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { realpathSync, statSync } from 'node:fs';
import { basename, isAbsolute } from 'node:path';
import { StringDecoder } from 'node:string_decoder';
import { resolveServerBin } from './bridgeDiscovery.js';
import type { TerminalScope, TerminalSnapshot, TerminalEvent } from '../shared/terminal.js';

const MAX_OUTPUT = 1024 * 1024;
const MAX_FRAME = 256 * 1024;
const MAX_PENDING = 128;
const MAX_TERMINALS = 32;
const MAX_SCOPE_TERMINALS = 8;

/** Deliberately excludes model credentials, plugin secrets and loader injection variables. */
export function terminalEnvironment(source: NodeJS.ProcessEnv = process.env): NodeJS.ProcessEnv {
  const result: NodeJS.ProcessEnv = {};
  for (const key of ['PATH', 'HOME', 'USER', 'LOGNAME', 'SHELL', 'LANG', 'LC_ALL', 'LC_CTYPE', 'TMPDIR', 'TMP', 'TEMP', 'TZ', 'SystemRoot', 'SYSTEMROOT', 'WINDIR', 'COMSPEC', 'ComSpec', 'PATHEXT', 'USERPROFILE', 'APPDATA', 'LOCALAPPDATA', 'HOMEDRIVE', 'HOMEPATH', 'SSH_AUTH_SOCK']) {
    if (source[key] !== undefined) result[key] = source[key];
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
  private listeners = new Set<(event: TerminalEvent) => void>();
  private pending = new Map<string, { resolve: () => void; reject: (error: Error) => void; timer: ReturnType<typeof setTimeout> }>();
  private writes: Promise<void> = Promise.resolve();
  private outputPauseOwners = new Set<string>();
  private resumeOutput?: () => void;
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
    if (this.rows.size >= MAX_TERMINALS || this.list(scope).length >= MAX_SCOPE_TERMINALS) throw new Error('Terminal limit reached');
    const id = randomUUID();
    const row: TerminalSnapshot = { id, scope, title: basename(scope.projectPath) || scope.projectPath, status: 'running', exitCode: null, output: '', sequence: 0 };
    this.rows.set(id, row);
    this.brokerTerminalIds.add(id);
    try { await this.request({ kind: 'create', terminalId: id, cwd: scope.projectPath, cols: 80, rows: 24 }); }
    catch (error) { this.rows.delete(id); this.brokerTerminalIds.delete(id); throw error; }
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
  async close(id: string): Promise<void> {
    const row = this.rows.get(id);
    if (!row) return;
    try {
      if (this.brokerTerminalIds.has(id)) await this.request({ kind: 'close', terminalId: id });
    }
    catch {
      // The broker answers "unknown terminal" for a shell the user already
      // exited, because opening the next terminal evicts exited handles from
      // its map. The terminal is gone either way, so local teardown must still
      // run: leaving the row would keep the tab alive, count against the
      // terminal limit forever, and abort `closeScope`/`closeProject` mid-loop
      // (which is how archiving a chat reported "unknown terminal" instead).
    }
    this.brokerTerminalIds.delete(id);
    this.rows.delete(id);
    this.emit({ kind: 'closed', terminalId: id });
  }
  async closeScope(scope: TerminalScope): Promise<void> { for (const row of this.list(scope)) await this.close(row.id); }
  async closeProject(projectPath: string): Promise<void> { for (const row of [...this.rows.values()]) if (row.scope.projectPath === projectPath) await this.close(row.id); }
  migrateScope(from: TerminalScope, inputTo: TerminalScope): void {
    const to = validateScope(inputTo);
    if (from.projectPath !== to.projectPath) throw new Error('Cannot migrate terminal across projects');
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
    const child = (this.options.spawnProcess ?? spawn)(resolveServerBin(this.options), ['--desktop-terminal'], { stdio: ['pipe', 'pipe', 'pipe'], env: terminalEnvironment(), windowsHide: true }) as ChildProcessWithoutNullStreams;
    this.child = child;
    child.stderr.resume(); // Diagnostics are deliberately kept out of terminal output and secrets are not logged.
    child.on('error', () => this.fail(child, new Error('Terminal service could not start')));
    child.on('close', () => this.fail(child, new Error('Terminal service stopped')));
    child.stdin.on('error', () => this.fail(child, new Error('Terminal service input closed')));
    void this.read(child);
    return child;
  }
  private request(command: Record<string, unknown>): Promise<void> {
    if (this.pending.size >= MAX_PENDING) return Promise.reject(new Error('Too many pending terminal operations'));
    let child: ChildProcessWithoutNullStreams;
    try { child = this.start(); } catch (error) { return Promise.reject(error); }
    const requestId = randomUUID();
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
    const line = JSON.stringify({ ...command, requestId }) + '\n';
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
  private fail(child: ChildProcessWithoutNullStreams, error: Error): void {
    if (this.child !== child) return;
    this.child = undefined;
    this.releaseOutput();
    this.brokerTerminalIds.clear();
    // EOF gives the broker time to terminate/reap PTY process trees even after a protocol error.
    child.stdin.end();
    const killTimer = setTimeout(() => child.kill(), 6000);
    killTimer.unref?.();
    child.once('close', () => clearTimeout(killTimer));
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
