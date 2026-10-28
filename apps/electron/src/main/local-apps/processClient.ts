import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { EventEmitter } from 'node:events';
import { promises as fs } from 'node:fs';
import { win32 } from 'node:path';
import { StringDecoder } from 'node:string_decoder';

const INPUT_LIMIT = 256 * 1024;
const FRAME_LIMIT = 16 * 1024;
const OUTPUT_LIMIT = 4096;
const START_TIMEOUT_MS = 15_000;
// A stop may spend five seconds waiting on the leader, another five on the
// Job, then time draining output and delivering its receipt.
const STOP_TIMEOUT_MS = 25_000;
const CLOSE_TIMEOUT_MS = 4000;

type OutputStream = 'stdout' | 'stderr';
export type LocalAppProcessExit = { code: number | null; signal: NodeJS.Signals | null };
export interface LocalAppProcessClientOptions {
  executable: string;
  helperArgs: string[];
  command: string;
  args: string[];
  cwd: string;
  env: NodeJS.ProcessEnv;
  helperEnv?: NodeJS.ProcessEnv;
  onOutput: (stream: OutputStream, text: string) => void;
  onDiagnostic?: (text: string) => void;
}
interface Deferred<T> { promise: Promise<T>; resolve(value: T): void; reject(error: Error): void }
function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void, reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  // Some failures arrive before the owner begins waiting. Keep those promises
  // observed while preserving their errors for later consumers.
  void promise.catch(() => undefined);
  return { promise, resolve, reject };
}
function unverified(message: string): Error { return new Error(`Local App process cleanup is unverified: ${message}`); }

/** Match Node's Windows spawn behavior for case-insensitive duplicate keys. */
export function windowsProcessEnvironment(env: NodeJS.ProcessEnv): Record<string, string> {
  const result: Record<string, string> = {}, seen = new Set<string>();
  for (const key of Object.keys(env).sort()) {
    const value = env[key];
    if (value === undefined) continue;
    if (!key || key.includes('=') || key.includes('\0') || value.includes('\0')) throw new Error('Invalid Windows application environment');
    const canonical = key.toUpperCase();
    if (!seen.has(canonical)) { seen.add(canonical); result[key] = value; }
  }
  return result;
}

function windowsPathEntries(value: string): string[] {
  const result: string[] = [];
  for (let start = 0; start < value.length;) {
    let end = start;
    if (value[start] === '"' || value[start] === "'") {
      const closing = value.indexOf(value[start]!, start + 1);
      end = closing < 0 ? value.length : closing + 1;
    }
    const separator = value.indexOf(';', end);
    end = separator < 0 ? value.length : separator;
    let entry = value.slice(start, end);
    if (entry[0] === '"' || entry[0] === "'") entry = entry.slice(1);
    if (entry.endsWith('"') || entry.endsWith("'")) entry = entry.slice(0, -1);
    if (entry) result.push(entry);
    start = end + 1;
  }
  return result;
}

/** Resolve before Rust spawn: Rust otherwise also searches the helper's PATH. */
export async function resolveWindowsProgram(
  program: string, cwd: string, env: Record<string, string>,
  exists: (path: string) => Promise<boolean> = async path => {
    try { return !(await fs.stat(path)).isDirectory(); } catch { return false; }
  },
  searchCurrentDirectory = !Object.keys(process.env).some(key => key.toUpperCase() === 'NODEFAULTCURRENTDIRECTORYINEXEPATH'),
): Promise<string> {
  if (!program || program === '.') throw new Error('Application shell executable was not found');
  const name = win32.basename(program), dot = name.indexOf('.');
  const names = [...(dot >= 0 && dot + 1 < name.length ? [program] : []), `${program}.com`, `${program}.exe`];
  const hasDirectory = /[\\/:]/.test(program);
  const pathValue = Object.entries(env).find(([key]) => key.toUpperCase() === 'PATH')?.[1] ?? '';
  const directories = hasDirectory ? [''] : [...(searchCurrentDirectory ? [cwd] : []), ...windowsPathEntries(pathValue)];
  for (const directory of directories) for (const candidate of names) {
    const absolute = win32.resolve(cwd, directory, candidate);
    if (await exists(absolute)) return absolute;
  }
  throw new Error(`Application shell executable was not found: ${program}`);
}
function withDeadline<T>(promise: Promise<T>, ms: number, message: string, signal?: AbortSignal): Promise<T> {
  return new Promise((resolve, reject) => {
    let settled = false;
    const finish = (error?: Error, value?: T) => {
      if (settled) return;
      settled = true; clearTimeout(timer); signal?.removeEventListener('abort', abort);
      if (error) reject(error); else resolve(value as T);
    };
    const abort = () => finish(new Error('Application operation cancelled'));
    const timer = setTimeout(() => finish(unverified(message)), ms);
    signal?.addEventListener('abort', abort, { once: true });
    void promise.then(value => finish(undefined, value), error => finish(error instanceof Error ? error : new Error(String(error))));
    if (signal?.aborted) abort();
  });
}

/** One private helper owns one process tree. Its PID is never a cleanup token. */
export class LocalAppProcessClient extends EventEmitter {
  readonly id = randomUUID();
  private readonly helper: ChildProcessWithoutNullStreams;
  private readonly startResult = deferred<void>();
  private readonly completion = deferred<LocalAppProcessExit>();
  private readonly closed = deferred<void>();
  private readonly decoders: Record<OutputStream, StringDecoder> = { stdout: new StringDecoder('utf8'), stderr: new StringDecoder('utf8') };
  private readonly attempts = new Map<string, { result: Deferred<void> }>();
  private attemptSequence = 0;
  private buffer = Buffer.alloc(0);
  private parsing = false;
  private startIssued = false;
  private cancelBeforeStart = false;
  private noChildStarted = false;
  private startupError?: Error;
  private startupCleanupVerified = false;
  private failure?: Error;
  private cleanupVerified = false;
  private activeAttempt?: string;
  private stopInFlight?: Promise<void>;
  private closeInFlight?: Promise<void>;
  private exitTimer?: ReturnType<typeof setTimeout>;
  private outputFinished = false;
  private helperClosed = false;
  private released = false;
  private exit?: LocalAppProcessExit;
  started = false;
  hasExited = false;
  exitCode: number | null = null;
  signalCode: NodeJS.Signals | null = null;

  constructor(private readonly options: LocalAppProcessClientOptions) {
    super();
    const startFrame = { version: 1, type: 'start', id: this.id, command: options.command, args: options.args, cwd: options.cwd, env: options.env };
    if (Buffer.byteLength(`${JSON.stringify(startFrame)}\n`) > INPUT_LIMIT) throw new Error('Local App process request exceeds limit');
    this.helper = spawn(options.executable, options.helperArgs, { stdio: 'pipe', windowsHide: true, env: options.helperEnv });
    this.helper.stdout.on('data', (chunk: Buffer) => this.receive(chunk));
    this.helper.stdout.on('error', error => this.fail(unverified(`helper output failed: ${error.message}`)));
    const diagnostic = (text: string) => { try { options.onDiagnostic?.(text.slice(-4096)); } catch { /* diagnostics are not a cleanup receipt */ } };
    this.helper.stderr.setEncoding('utf8').on('data', (text: string) => diagnostic(text));
    // This pipe carries helper diagnostics only, never payload output or a
    // cleanup receipt. A read error must not crash the Electron main process.
    this.helper.stderr.on('error', error => diagnostic(`Local App helper diagnostics failed: ${error.message}`));
    this.helper.stdin.on('error', error => this.controlFailure(error));
    this.helper.once('spawn', () => {
      if (this.cancelBeforeStart) return;
      this.startIssued = true;
      this.write(startFrame);
    });
    this.helper.on('error', error => {
      if (!this.startIssued) {
        this.noChildStarted = true;
        this.startResult.reject(new Error(`Local App process helper could not start: ${error.message}`));
      } else this.fail(unverified(`helper failed: ${error.message}`));
    });
    this.helper.once('close', () => {
      this.helperClosed = true;
      if (!this.parsing) this.finishClosed();
    });
  }

  private write(frame: Record<string, unknown>): void {
    let data: string;
    try { data = `${JSON.stringify(frame)}\n`; }
    catch (error) { this.fail(unverified(`cannot encode helper request: ${String(error)}`)); return; }
    if (Buffer.byteLength(data) > INPUT_LIMIT) { this.fail(unverified('helper request exceeds limit')); return; }
    try { this.helper.stdin.write(data, error => { if (error) this.controlFailure(error); }); }
    catch (error) { this.controlFailure(error); }
  }

  private controlFailure(error: unknown): void {
    const message = error instanceof Error ? error.message : String(error);
    if (this.cleanupVerified || this.noChildStarted) {
      // A later control-pipe error cannot revoke an already validated proof;
      // finish() still waits for the owned helper process to close.
      try { this.options.onDiagnostic?.(`Local App helper control closed after cleanup: ${message}`); } catch { /* diagnostics are not a cleanup receipt */ }
    } else if (this.startIssued) this.fail(unverified(`helper control failed: ${message}`));
  }

  private receive(chunk: Buffer): void {
    if (this.failure) return;
    this.buffer = Buffer.concat([this.buffer, chunk]);
    if (!this.parsing) this.parseFrames();
  }

  private parseFrames(): void {
    this.parsing = true;
    let processed = 0;
    for (;;) {
      if (this.failure) { this.parsing = false; if (this.helperClosed) this.finishClosed(); return; }
      const newline = this.buffer.indexOf(10);
      if (newline < 0) {
        if (this.buffer.length >= FRAME_LIMIT) this.fail(unverified('helper response exceeds limit'));
        this.parsing = false;
        if (this.helperClosed) this.finishClosed(); else this.helper.stdout.resume();
        return;
      }
      if (newline + 1 > FRAME_LIMIT) { this.fail(unverified('helper response exceeds limit')); this.parsing = false; if (this.helperClosed) this.finishClosed(); return; }
      const line = this.buffer.subarray(0, newline); this.buffer = this.buffer.subarray(newline + 1);
      try {
        const text = new TextDecoder('utf-8', { fatal: true }).decode(line);
        this.frame(JSON.parse(text) as unknown);
      } catch (error) { this.fail(unverified(`invalid helper response: ${String(error)}`)); this.parsing = false; if (this.helperClosed) this.finishClosed(); return; }
      if (++processed === 32 && this.buffer.includes(10)) {
        // Yield between bounded batches so a busy output stream cannot keep
        // the Electron event loop from sending a stop request.
        this.helper.stdout.pause();
        setImmediate(() => { this.parsing = false; this.parseFrames(); });
        return;
      }
    }
  }

  private finishClosed(): void {
    if (this.buffer.includes(10) && !this.failure) { this.parseFrames(); return; }
    if (this.buffer.length) this.fail(unverified('incomplete helper response'));
    if (!this.failure && !this.cleanupVerified && !this.noChildStarted) this.fail(unverified('helper exited without a verified receipt'));
    this.closed.resolve();
  }

  private frame(value: unknown): void {
    if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('frame is not an object');
    const row = value as Record<string, unknown>;
    if (row.version !== 1 || row.id !== this.id || typeof row.type !== 'string') throw new Error('wrong protocol version or invocation');
    if (!this.startIssued) throw new Error('response before start');
    const keys = (...allowed: string[]) => {
      const permitted = new Set(['version', 'type', 'id', ...allowed]);
      if (Object.keys(row).some(key => !permitted.has(key))) throw new Error('unexpected frame field');
    };
    if (row.type === 'started') {
      keys();
      if (this.started || this.startupError || this.cleanupVerified) throw new Error('duplicate or late start');
      this.started = true; this.startResult.resolve();
      return;
    }
    if (row.type === 'start-error') {
      keys('message', 'noChildStarted');
      if (this.started || this.startupError || typeof row.message !== 'string' || typeof row.noChildStarted !== 'boolean') throw new Error('invalid start error');
      this.startupError = new Error(`Local App process start failed: ${row.message}`);
      this.noChildStarted = row.noChildStarted;
      this.startResult.reject(this.startupError);
      this.completion.reject(this.startupError);
      if (this.noChildStarted) {
        const active = this.activeAttempt ? this.attempts.get(this.activeAttempt) : undefined;
        this.activeAttempt = undefined;
        void this.finish().then(() => active?.result.resolve(), error => active?.result.reject(error));
      }
      return;
    }
    if (row.type === 'startup-cleanup') {
      keys('verified');
      if (row.verified !== true || !this.startupError || this.noChildStarted || this.started || this.hasExited || this.cleanupVerified || this.startupCleanupVerified) throw new Error('invalid startup cleanup proof');
      // The helper has independently proved its outer Job empty after the
      // failed spawn returned. This proves cleanup, never command execution.
      this.startupCleanupVerified = true;
      this.cleanupVerified = true;
      this.activeAttempt = undefined;
      const pending = [...this.attempts.values()];
      void this.finish().then(
        () => { for (const attempt of pending) attempt.result.resolve(); },
        error => { for (const attempt of pending) attempt.result.reject(error); },
      );
      return;
    }
    if (row.type === 'output') {
      keys('stream', 'data');
      if (!this.started || this.cleanupVerified || (row.stream !== 'stdout' && row.stream !== 'stderr') || typeof row.data !== 'string') throw new Error('invalid output');
      if (row.data.length > Math.ceil(OUTPUT_LIMIT / 3) * 4 || !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(row.data)) throw new Error('invalid base64 output');
      const bytes = Buffer.from(row.data, 'base64');
      if (bytes.length > OUTPUT_LIMIT || bytes.toString('base64') !== row.data) throw new Error('invalid output size');
      const text = this.decoders[row.stream].write(bytes);
      if (text) this.options.onOutput(row.stream, text);
      return;
    }
    if (row.type === 'leader-exit') {
      keys('code', 'signal');
      if ((!this.started && !this.startupError) || this.noChildStarted || this.hasExited || this.cleanupVerified || !(row.code === null || Number.isSafeInteger(row.code)) || row.signal !== null) throw new Error('invalid leader exit');
      this.exit = { code: row.code as number | null, signal: null };
      this.hasExited = true; this.exitCode = this.exit.code;
      this.exitTimer = setTimeout(() => this.completion.reject(unverified('helper did not confirm cleanup after leader exit')), STOP_TIMEOUT_MS);
      this.emit('exit', this.exit.code, this.exit.signal);
      return;
    }
    if (row.type === 'cleanup-result') {
      keys('attemptId', 'verified', 'error');
      if (!(row.attemptId === null || typeof row.attemptId === 'string') || typeof row.verified !== 'boolean' || (row.error !== undefined && typeof row.error !== 'string')) throw new Error('invalid cleanup result');
      let attempt: { result: Deferred<void> } | undefined;
      if (row.attemptId !== null) {
        const attemptId = row.attemptId as string, prefix = `${this.id}:`;
        if (!attemptId.startsWith(prefix)) throw new Error('unissued cleanup attempt');
        const suffix = attemptId.slice(prefix.length);
        if (!/^[1-9][0-9]*$/.test(suffix)) throw new Error('invalid cleanup attempt');
        const sequence = Number(suffix);
        if (!Number.isSafeInteger(sequence) || sequence > this.attemptSequence) throw new Error('unissued cleanup attempt');
        attempt = this.attempts.get(attemptId);
        // A reply to an already settled or timed-out attempt can never prove
        // a newer stop. Ignore it without retaining an unbounded ID history.
        if (!attempt) return;
      }
      if (!row.verified) {
        const error = unverified(typeof row.error === 'string' ? row.error : 'helper could not confirm cleanup');
        if (attempt) {
          this.attempts.delete(row.attemptId as string);
          if (this.activeAttempt === row.attemptId) this.activeAttempt = undefined;
          attempt.result.reject(error);
        } else this.completion.reject(error);
        this.completeIfReady();
        return;
      }
      if (!this.hasExited && !this.noChildStarted) throw new Error('cleanup receipt precedes leader wait');
      if (!this.outputFinished) {
        this.outputFinished = true;
        for (const stream of ['stdout', 'stderr'] as const) {
          const text = this.decoders[stream].end(); if (text) this.options.onOutput(stream, text);
        }
      }
      this.cleanupVerified = true;
      if (this.exitTimer) { clearTimeout(this.exitTimer); this.exitTimer = undefined; }
      if (attempt) {
        this.attempts.delete(row.attemptId as string);
        if (this.activeAttempt === row.attemptId) this.activeAttempt = undefined;
      } else {
        // A natural receipt is bound to this invocation and proves the same
        // owned Job empty, even if a stop request was already in flight.
        this.activeAttempt = undefined;
        const pending = [...this.attempts.values()];
        if (pending.length) void this.finish().then(
          () => { for (const waiter of pending) waiter.result.resolve(); },
          error => { for (const waiter of pending) waiter.result.reject(error); },
        );
      }
      this.completeIfReady();
      if (attempt && !this.activeAttempt) void this.finish().then(() => attempt.result.resolve(), error => attempt.result.reject(error));
      return;
    }
    throw new Error('unknown frame type');
  }

  private completeIfReady(): void {
    if (!this.cleanupVerified || this.activeAttempt) return;
    void this.finish().then(() => {
      if (this.exit) this.completion.resolve(this.exit);
    }, error => this.completion.reject(error));
  }
  private fail(error: Error): void {
    if (this.failure) return;
    this.failure = error; this.startResult.reject(error); this.completion.reject(error);
    if (this.exitTimer) { clearTimeout(this.exitTimer); this.exitTimer = undefined; }
    for (const attempt of this.attempts.values()) attempt.result.reject(error);
    try { this.helper.stdin.end(); } catch { /* parent must retain unverified ownership */ }
    this.emit('failure', error);
  }
  private finish(): Promise<void> {
    if (!this.closeInFlight) {
      const close = (async () => {
        if (!this.helperClosed) try { this.helper.stdin.end(); } catch (error) {
          this.controlFailure(error);
          if (this.failure) throw this.failure;
        }
        try { await withDeadline(this.closed.promise, CLOSE_TIMEOUT_MS, 'helper did not exit'); }
        catch (error) {
          // The tree is already proven empty (or no child was created); only
          // the held helper handle is used to bound its own shutdown.
          if (!this.helperClosed) try { this.helper.kill('SIGKILL'); } catch { /* close remains unverified */ }
          await withDeadline(this.closed.promise, CLOSE_TIMEOUT_MS, 'helper did not exit after termination');
        }
        if (this.failure) throw this.failure;
        if (!this.released) { this.released = true; this.emit('released'); }
      })();
      this.closeInFlight = close.catch(error => { this.closeInFlight = undefined; throw error; });
    }
    return this.closeInFlight;
  }

  waitStarted(signal?: AbortSignal): Promise<void> {
    return withDeadline(this.startResult.promise, START_TIMEOUT_MS, 'helper did not confirm startup', signal);
  }
  onExit(listener: (code: number | null, signal: NodeJS.Signals | null) => void): void {
    if (this.exit) queueMicrotask(() => listener(this.exit!.code, this.exit!.signal));
    else this.once('exit', listener);
  }
  onFailure(listener: (error: Error) => void): void {
    if (this.failure) queueMicrotask(() => listener(this.failure!));
    else this.once('failure', listener);
  }
  waitExit(signal?: AbortSignal): Promise<LocalAppProcessExit> {
    if (this.failure) return Promise.reject(this.failure);
    if (this.startupError) return Promise.reject(this.startupError);
    if (this.cleanupVerified && !this.activeAttempt && this.exit) return this.finish().then(() => this.exit!);
    if (!signal) return this.completion.promise;
    return new Promise((resolve, reject) => {
      const abort = () => { signal.removeEventListener('abort', abort); reject(new Error('Application operation cancelled')); };
      signal.addEventListener('abort', abort, { once: true });
      void this.completion.promise.then(value => { signal.removeEventListener('abort', abort); resolve(value); }, error => { signal.removeEventListener('abort', abort); reject(error); });
      if (signal.aborted) abort();
    });
  }
  stop(): Promise<void> {
    if (this.failure) return Promise.reject(this.failure);
    if (this.stopInFlight) return this.stopInFlight;
    if (this.cleanupVerified || this.noChildStarted) return this.finish();
    if (!this.startIssued) {
      this.cancelBeforeStart = true; this.noChildStarted = true;
      this.startResult.reject(new Error('Application operation cancelled'));
      return this.finish();
    }
    if (this.attemptSequence >= Number.MAX_SAFE_INTEGER) return Promise.reject(unverified('cleanup attempt sequence exhausted'));
    const attemptId = `${this.id}:${++this.attemptSequence}`, result = deferred<void>();
    this.attempts.set(attemptId, { result }); this.activeAttempt = attemptId;
    this.write({ version: 1, type: 'stop', id: this.id, attemptId });
    const pending = withDeadline(result.promise, STOP_TIMEOUT_MS, 'helper did not confirm stop').finally(() => {
      if (this.activeAttempt === attemptId) this.activeAttempt = undefined;
      this.attempts.delete(attemptId);
      if (this.stopInFlight === pending) this.stopInFlight = undefined;
      this.completeIfReady();
    });
    this.stopInFlight = pending;
    return pending;
  }
}
