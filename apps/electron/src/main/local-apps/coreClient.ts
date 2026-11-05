import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { randomUUID } from 'node:crypto';

export interface LocalAppCoreTransport {
  request<T>(request: Record<string, unknown>, options?: { waitForCompletion?: boolean }): Promise<T>;
  dispose(): Promise<void>;
  onDisconnected?(listener: (error: Error) => void): void;
}
/** A credential-free library process, independent of every conversation engine. */
export class LocalAppCoreClient implements LocalAppCoreTransport {
  private child?: ChildProcessWithoutNullStreams;
  private buffer = '';
  private closed = false;
  private disconnected?: Error;
  private readonly disconnectListeners = new Set<(error: Error) => void>();
  private readonly pending = new Map<string, { resolve(value: unknown): void; reject(error: Error): void; timer?: ReturnType<typeof setTimeout>; waitForCompletion: boolean }>();
  constructor(private readonly executable: string, private readonly configHome: string, private readonly report: (message: string) => void = () => undefined,
    private readonly timeoutMs = 120_000) {}
  private start(): ChildProcessWithoutNullStreams {
    if (this.closed) throw new Error('Local App library is closed');
    if (this.disconnected) throw this.disconnected;
    if (this.child) return this.child;
    const child = spawn(this.executable, ['--local-app-service-json', '--config-home', this.configHome], { stdio: 'pipe', windowsHide: true });
    this.child = child;
    let transportError: Error | undefined;
    let invalidOutput = false;
    child.stdin.on('error', error => { transportError ??= error; });
    child.stdout.setEncoding('utf8').on('data', (chunk: string) => {
      if (invalidOutput) return;
      this.buffer += chunk;
      if (this.buffer.length > 64 * 1024 * 1024) { invalidOutput = true; transportError = new Error('Local App library response exceeds limit'); child.kill(); return; }
      for (;;) {
        const index = this.buffer.indexOf('\n'); if (index < 0) break;
        const line = this.buffer.slice(0, index); this.buffer = this.buffer.slice(index + 1);
        try {
          const response = JSON.parse(line);
          const pending = this.pending.get(response.id); if (!pending) continue;
          this.pending.delete(response.id); if (pending.timer) clearTimeout(pending.timer);
          if (response.ok === true) pending.resolve(response.value);
          else pending.reject(Object.assign(new Error(response.error?.message ?? 'Local App library request failed'), { code: response.error?.code }));
        } catch { invalidOutput = true; transportError = new Error('Invalid Local App library response'); child.kill(); return; }
      }
    });
    child.stderr.setEncoding('utf8').on('data', (chunk: string) => this.report(chunk.slice(-4096)));
    child.once('error', error => { transportError ??= error; });
    child.once('close', (code, signal) => {
      if (this.child === child) this.child = undefined;
      this.buffer = '';
      if (!this.closed || code !== 0 || signal || transportError) {
        this.report(`Local App library closed (code=${code}, signal=${signal})${transportError ? `: ${transportError.message}` : ''}\n`);
      }
      const error = transportError ?? new Error('Local App library exited; restart the application after its running apps stop');
      this.fail(error);
      if (!this.closed) {
        this.disconnected = error;
        for (const listener of this.disconnectListeners) listener(error);
      }
    });
    return child;
  }
  private fail(error: Error, includeMutations = true): void {
    for (const [id, p] of this.pending) {
      if (!includeMutations && p.waitForCompletion) continue;
      if (p.timer) clearTimeout(p.timer);
      this.pending.delete(id);
      p.reject(p.waitForCompletion ? Object.assign(new Error(`Local App mutation outcome is unknown: ${error.message}`, { cause: error }), { code: 'outcomeUnknown' }) : error);
    }
  }
  onDisconnected(listener: (error: Error) => void): void {
    this.disconnectListeners.add(listener);
    if (this.disconnected) listener(this.disconnected);
  }
  request<T>(request: Record<string, unknown>, options?: { waitForCompletion?: boolean }): Promise<T> {
    const child = this.start(), id = randomUUID();
    return new Promise<T>((resolve, reject) => {
      const waitForCompletion = options?.waitForCompletion === true;
      const timer = waitForCompletion ? undefined : setTimeout(() => { this.pending.delete(id); reject(new Error('Local App library request timed out; refresh status before retrying a mutation')); }, this.timeoutMs);
      this.pending.set(id, { resolve: value => resolve(value as T), reject, timer, waitForCompletion });
      child.stdin.write(JSON.stringify({ id, request }) + '\n', error => {
        // A pipe error does not prove whether the library already accepted a
        // mutation. Hold its owner until the response or actual process exit.
        if (error && !waitForCompletion) { this.pending.delete(id); if (timer) clearTimeout(timer); reject(error); }
      });
    });
  }
  async dispose(): Promise<void> {
    this.closed = true; this.fail(new Error('Local App library is shutting down'), false);
    const child = this.child; if (!child) return;
    child.stdin.end();
    await new Promise<void>(resolve => { const timer = setTimeout(() => { child.kill(); }, 1000); child.once('close', () => { clearTimeout(timer); resolve(); }); });
  }
}
