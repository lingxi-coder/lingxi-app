import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { randomUUID } from 'node:crypto';

export interface LocalAppCoreTransport { request<T>(request: Record<string, unknown>): Promise<T>; dispose(): Promise<void>; onDisconnected?(listener: (error: Error) => void): void }
/** A credential-free library process, independent of every conversation engine. */
export class LocalAppCoreClient implements LocalAppCoreTransport {
  private child?: ChildProcessWithoutNullStreams;
  private buffer = '';
  private closed = false;
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
      this.buffer += chunk;
      if (this.buffer.length > 64 * 1024 * 1024) { this.fail(new Error('Local App library response exceeds limit')); child.kill(); return; }
      for (;;) {
        const index = this.buffer.indexOf('\n'); if (index < 0) break;
        const line = this.buffer.slice(0, index); this.buffer = this.buffer.slice(index + 1);
        try {
          const response = JSON.parse(line);
          const pending = this.pending.get(response.id); if (!pending) continue;
          this.pending.delete(response.id); clearTimeout(pending.timer);
          if (response.ok === true) pending.resolve(response.value);
          else pending.reject(Object.assign(new Error(response.error?.message ?? 'Local App library request failed'), { code: response.error?.code }));
        } catch { this.fail(new Error('Invalid Local App library response')); child.kill(); }
      }
    });
    child.stderr.setEncoding('utf8').on('data', (chunk: string) => this.report(chunk.slice(-4096)));
    child.once('error', error => this.fail(error));
    child.once('close', () => {
      if (this.child === child) this.child = undefined;
      this.buffer = '';
      const error = new Error('Local App library exited; restart the application after its running apps stop');
      this.fail(error);
      if (!this.closed) {
        this.disconnected = error;
        for (const listener of this.disconnectListeners) listener(error);
      }
    });
    return child;
  }
  private fail(error: Error): void { for (const p of this.pending.values()) { clearTimeout(p.timer); p.reject(error); } this.pending.clear(); }
  onDisconnected(listener: (error: Error) => void): void {
    this.disconnectListeners.add(listener);
    if (this.disconnected) listener(this.disconnected);
  }
  request<T>(request: Record<string, unknown>): Promise<T> {
    const child = this.start(), id = randomUUID();
    return new Promise<T>((resolve, reject) => {
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error('Local App library request timed out; refresh status before retrying a mutation')); }, 120_000);
      this.pending.set(id, { resolve: value => resolve(value as T), reject, timer });
      child.stdin.write(JSON.stringify({ id, request }) + '\n', error => { if (error) { this.pending.delete(id); clearTimeout(timer); reject(error); } });
    });
  }
  async dispose(): Promise<void> {
    this.closed = true; this.fail(new Error('Local App library is shutting down'));
    const child = this.child; if (!child) return;
    child.stdin.end();
    await new Promise<void>(resolve => { const timer = setTimeout(() => { child.kill(); resolve(); }, 1000); child.once('close', () => { clearTimeout(timer); resolve(); }); });
  }
}
