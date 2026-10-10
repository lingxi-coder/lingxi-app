/**
 * Correlates an event-replied request without relying on the WebSocket frame
 * id. Product runtimes are session-scoped, so the key always includes both
 * the session and request id even when each caller currently owns one session.
 */
export class SessionEventCorrelator<T> {
  private readonly pending = new Map<string, {
    sessionId: string;
    requestId: string;
    timer: ReturnType<typeof setTimeout>;
    resolve(value: T): void;
    reject(error: Error): void;
  }>();

  request(sessionId: string, requestId: string, timeoutMs: number): Promise<T> {
    if (!sessionId || !requestId || !Number.isSafeInteger(timeoutMs) || timeoutMs < 1) {
      return Promise.reject(new Error('invalid correlated event request'));
    }
    const key = this.key(sessionId, requestId);
    if (this.pending.has(key)) return Promise.reject(new Error('correlated event request is already pending'));

    return new Promise<T>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(key);
        reject(new Error('correlated event request timed out'));
      }, timeoutMs);
      this.pending.set(key, { sessionId, requestId, timer, resolve, reject });
    });
  }

  resolve(sessionId: string, requestId: string, value: T): boolean {
    const pending = this.take(sessionId, requestId);
    if (!pending) return false;
    pending.resolve(value);
    return true;
  }

  reject(sessionId: string, requestId: string, error: Error): boolean {
    const pending = this.take(sessionId, requestId);
    if (!pending) return false;
    pending.reject(error);
    return true;
  }

  rejectSession(sessionId: string, error: Error): void {
    for (const [key, pending] of this.pending) {
      if (pending.sessionId !== sessionId) continue;
      this.pending.delete(key);
      clearTimeout(pending.timer);
      pending.reject(error);
    }
  }

  get size(): number {
    return this.pending.size;
  }

  private take(sessionId: string, requestId: string) {
    const key = this.key(sessionId, requestId);
    const pending = this.pending.get(key);
    if (!pending) return undefined;
    this.pending.delete(key);
    clearTimeout(pending.timer);
    return pending;
  }

  private key(sessionId: string, requestId: string): string {
    return JSON.stringify([sessionId, requestId]);
  }
}
