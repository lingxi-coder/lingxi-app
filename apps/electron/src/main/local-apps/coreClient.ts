import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { randomUUID } from 'node:crypto';

export interface LocalAppCoreTransport { request<T>(request: Record<string, unknown>): Promise<T>; dispose(): Promise<void>; onDisconnected?(listener: (error: Error) => void): void }
/** A credential-free library process, independent of every conversation engine. */
export class LocalAppCoreClient implements LocalAppCoreTransport {
  private child?: ChildProcessWithoutNullStreams;
  private buffer = '';
  private closed = false;
  private closing?: Promise<void>;
  private disconnected?: Error;
  private readonly disconnectListeners = new Set<(error: Error) => void>();
  private readonly pending = new Map<string, { resolve(value: unknown): void; reject(error: Error): void; timer: ReturnType<typeof setTimeout> }>();
  constructor(private readonly executable: string, private readonly configHome: string, private readonly report: (message: string) => void = () => undefined) {}
  private start(): ChildProcessWithoutNullStreams {
    if (this.closed) throw new Error('Local App library is closed');
    if (this.disconnected) throw this.disconnected;
    if (this.child) return this.child;
    const child = spawn(this.executable, ['--local-app-service-json', '--config-home', this.configHome], { stdio: 'pipe', windowsHide: true });
    this.child = child;
    child.stdout.setEncoding('utf8').on('data', (chunk: string) => {
      if (this.closed || this.disconnected || this.child !== child) return;
      const hasNewline = chunk.includes('\n');
      this.buffer += chunk;
      if (this.buffer.length > 64 * 1024 * 1024) {
        this.buffer = '';
        this.inputFailure(child, new Error('Local App library response exceeds limit'));
        return;
      }
      // The saved partial line has no newline; avoid repeatedly scanning it
      // until the new chunk can complete a frame.
      if (!hasNewline) return;
      for (;;) {
        const index = this.buffer.indexOf('\n'); if (index < 0) break;
        const line = this.buffer.slice(0, index); this.buffer = this.buffer.slice(index + 1);
        try {
          const response = JSON.parse(line);
          const pending = this.pending.get(response.id); if (!pending) continue;
          this.pending.delete(response.id); clearTimeout(pending.timer);
          if (response.ok === true) pending.resolve(response.value);
          else pending.reject(Object.assign(new Error(response.error?.message ?? 'Local App library request failed'), { code: response.error?.code }));
        } catch {
          this.buffer = '';
          this.inputFailure(child, new Error('Invalid Local App library response'));
          return;
        }
      }
    });
    child.stderr.setEncoding('utf8').on('data', (chunk: string) => this.report(chunk.slice(-4096)));
    child.on('error', error => this.inputFailure(child, error));
    child.stdin.on('error', error => this.inputFailure(child, error));
    child.once('close', () => {
      if (this.child === child) this.child = undefined;
      this.buffer = '';
      const error = new Error('Local App library exited; restart the application after its running apps stop');
      this.disconnect(error);
    });
    return child;
  }
  private fail(error: Error): void { for (const p of this.pending.values()) { clearTimeout(p.timer); p.reject(error); } this.pending.clear(); }
  private disconnect(error: Error): void {
    this.fail(error);
    if (this.closed || this.disconnected) return;
    this.disconnected = error;
    for (const listener of this.disconnectListeners) listener(error);
  }
  private inputFailure(child: ChildProcessWithoutNullStreams, error: Error): void {
    const stop = !this.closed && !this.disconnected && this.child === child;
    this.disconnect(error);
    if (!stop || this.child !== child || this.closing) return;
    // Do not wait for the host to dispose: a child can ignore EOF and TERM.
    // Retain it until close, and let dispose retry if cleanup is unverified.
    const closing = this.closeChild(child);
    this.closing = closing;
    void closing.then(
      () => { if (this.closing === closing) this.closing = undefined; },
      failure => {
        if (this.closing === closing) this.closing = undefined;
        try { this.report(`Local App library cleanup failed: ${String(failure)}`); } catch { /* diagnostics must not hide cleanup failure */ }
      },
    );
  }
  onDisconnected(listener: (error: Error) => void): void {
    this.disconnectListeners.add(listener);
    if (this.disconnected) listener(this.disconnected);
  }
  request<T>(request: Record<string, unknown>): Promise<T> {
    const child = this.start(), id = randomUUID();
    return new Promise<T>((resolve, reject) => {
      let payload: string;
      try { payload = JSON.stringify({ id, request }) + '\n'; }
      catch (error) { reject(error); return; }
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error('Local App library request timed out; refresh status before retrying a mutation')); }, 120_000);
      this.pending.set(id, { resolve: value => resolve(value as T), reject, timer });
      try { child.stdin.write(payload, error => { if (error) this.inputFailure(child, error); }); }
      catch (error) { this.inputFailure(child, error instanceof Error ? error : new Error(String(error))); }
    });
  }
  async dispose(): Promise<void> {
    this.closed = true; this.fail(new Error('Local App library is shutting down'));
    if (this.closing) return this.closing;
    const child = this.child; if (!child) return;
    const closing = this.closeChild(child); this.closing = closing;
    try { await closing; } finally { if (this.closing === closing) this.closing = undefined; }
  }
  private async closeChild(child: ChildProcessWithoutNullStreams): Promise<void> {
    let onClose!: () => void;
    const closed = new Promise<void>(resolve => { onClose = resolve; child.once('close', onClose); });
    const wait = (ms: number) => new Promise<boolean>(resolve => {
      const timer = setTimeout(() => resolve(false), ms);
      void closed.then(() => { clearTimeout(timer); resolve(true); });
    });
    const failures: string[] = [];
    try {
      try { child.stdin.end(); } catch (error) { failures.push(`stdin: ${String(error)}`); }
      if (await wait(1000)) return;
      for (const signal of ['SIGTERM', 'SIGKILL'] as const) {
        try { if (!child.kill(signal)) failures.push(`${signal} was not sent`); }
        catch (error) { failures.push(`${signal}: ${String(error)}`); }
        if (await wait(1000)) return;
      }
      throw new Error(`Local App library exit could not be confirmed${failures.length ? `: ${failures.join('; ')}` : ''}`);
    } finally { child.removeListener('close', onClose); }
  }
}
