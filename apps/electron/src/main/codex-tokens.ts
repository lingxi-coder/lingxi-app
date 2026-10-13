import {
  CODEX_PROVIDER_ID,
  CODEX_CLIENT_ID,
  CODEX_ISSUER,
  parseCodexSession,
  sessionFromTokenResponse,
  type CodexOAuthSession,
} from './codex-auth.js';

/** Refresh this long before the access token expires. */
const REFRESH_LEAD_SECONDS = 5 * 60;
/** An engine that asks this soon after a refresh gets that refresh's token. */
const RECENT_REFRESH_MS = 30_000;
/** After a failed refresh, wait this long before the network is tried again. */
const RETRY_AFTER_FAILURE_MS = 60_000;
const REFRESH_TIMEOUT_MS = 30_000;

/** Token endpoint answers that end the session; anything else may be retried. */
const TERMINAL_REFRESH_ERRORS = new Set([
  'invalid_grant', 'invalid_token', 'invalid_refresh_token',
  'refresh_token_expired', 'refresh_token_reused', 'refresh_token_invalidated',
]);

export const CODEX_SESSION_EXPIRED = 'Codex 登录已过期，请重新登录。';

export interface CodexSessionStore {
  resolve(providerId: string): Promise<string | undefined>;
  set(providerId: string, credential: string): Promise<unknown>;
  delete(providerId: string): Promise<unknown>;
}

/**
 * The one refresher of the Codex (ChatGPT) session.
 *
 * Every refresh rotates the refresh token, and the provider revokes the whole
 * session when an old one is used again, so exactly one party may refresh it.
 * This main-process manager is that party: engines receive only access tokens
 * through `subscribe` and ask for the next one through `renew`.
 *
 * Every read-modify-write runs on one queue, so a sign-in or sign-out never
 * interleaves with a refresh, and a rotation is persisted before anyone sees
 * its access token.
 */
export class CodexTokenManager {
  private session: CodexOAuthSession | undefined;
  private loaded = false;
  private queue: Promise<unknown> = Promise.resolve();
  private lastRefreshAt = 0;
  private lastFailureAt = 0;
  private lastFailure = CODEX_SESSION_EXPIRED;
  private readonly listeners = new Set<(session: CodexOAuthSession | undefined) => void>();
  private timer: ReturnType<typeof setInterval> | undefined;

  constructor(private readonly opts: {
    store?: CodexSessionStore;
    fetch?: typeof fetch;
    /** Wall-clock milliseconds; a sleeping laptop must not delay expiry. */
    now?: () => number;
    onError?: (message: string) => void;
  }) {}

  private now(): number { return (this.opts.now ?? Date.now)(); }

  private run<T>(operation: () => Promise<T>): Promise<T> {
    const next = this.queue.catch(() => undefined).then(operation);
    this.queue = next;
    return next;
  }

  subscribe(listener: (session: CodexOAuthSession | undefined) => void): () => void {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  }

  private publish(session: CodexOAuthSession | undefined): void {
    this.session = session;
    this.loaded = true;
    for (const listener of this.listeners) listener(session);
  }

  private dueSoon(session: CodexOAuthSession): boolean {
    return session.expires_at - this.now() / 1000 < REFRESH_LEAD_SECONDS;
  }

  private async read(): Promise<CodexOAuthSession | undefined> {
    const secret = await this.opts.store?.resolve(CODEX_PROVIDER_ID);
    if (!secret) return undefined;
    try { return parseCodexSession(JSON.parse(secret)); }
    catch { throw new Error('Codex 登录凭据无效，请重新登录。'); }
  }

  /** The session to use now, refreshed first when it is about to expire. */
  current(): Promise<CodexOAuthSession | undefined> {
    return this.run(async () => {
      if (!this.loaded) {
        this.session = await this.read();
        this.loaded = true;
      }
      const session = this.session;
      if (session?.refresh_token && this.dueSoon(session)) {
        return this.refresh().catch((error: unknown) => {
          this.reportFailure(error);
          return this.session;
        });
      }
      return session;
    });
  }

  /**
   * An engine's access token expired or was rejected. Refresh, unless a
   * refresh just happened: engines asking together share one.
   */
  renew(): Promise<CodexOAuthSession | undefined> {
    return this.run(async () => {
      if (!this.loaded) {
        this.session = await this.read();
        this.loaded = true;
      }
      if (!this.session?.refresh_token) return this.session;
      if (this.now() - this.lastRefreshAt < RECENT_REFRESH_MS) return this.session;
      return this.refresh().catch((error: unknown) => {
        this.reportFailure(error);
        throw error;
      });
    });
  }

  /** Store a new sign-in and serve it to every engine. */
  signIn(session: CodexOAuthSession): Promise<void> {
    return this.run(async () => {
      await this.opts.store?.set(CODEX_PROVIDER_ID, JSON.stringify(session));
      this.lastFailureAt = 0;
      this.lastRefreshAt = this.now();
      this.publish(session);
    });
  }

  /** Forget the session and take it away from every engine. */
  signOut(): Promise<void> {
    return this.run(async () => {
      await this.opts.store?.delete(CODEX_PROVIDER_ID);
      this.lastFailureAt = 0;
      this.publish(undefined);
    });
  }

  /**
   * A session an engine signed in to by itself. Only one with a refresh token
   * replaces the stored session; the engine stops refreshing it from here on.
   */
  adopt(session: CodexOAuthSession): Promise<void> {
    if (!session.refresh_token) return Promise.resolve();
    return this.signIn(parseCodexSession(session));
  }

  /** Refresh ahead of expiry. Safe to call often. */
  check(): void {
    const session = this.session;
    if (!session?.refresh_token || !this.dueSoon(session)) return;
    if (this.now() - this.lastFailureAt < RETRY_AFTER_FAILURE_MS) return;
    void this.current().catch((error: unknown) => this.reportFailure(error));
  }

  /**
   * Check every `intervalMs` by the wall clock. A timer alone is not enough:
   * macOS stops the monotonic clock while asleep, so the host also calls
   * `check` on resume.
   */
  start(intervalMs = 60_000): void {
    if (this.timer) return;
    void this.current().catch((error: unknown) => this.reportFailure(error));
    this.timer = setInterval(() => this.check(), intervalMs);
    this.timer.unref?.();
  }

  dispose(): void {
    if (this.timer) clearInterval(this.timer);
    this.timer = undefined;
    this.listeners.clear();
  }

  private reportFailure(error: unknown): void {
    this.opts.onError?.(error instanceof Error && error.message.startsWith('Codex ') ? error.message : 'Codex 登录刷新失败。');
  }

  private failed(message: string): Error {
    this.lastFailureAt = this.now();
    this.lastFailure = message;
    return new Error(message);
  }

  /** Runs on the queue. */
  private async refresh(): Promise<CodexOAuthSession | undefined> {
    // Re-read: the stored session is the authority, and it may be newer than
    // the copy this process holds.
    const stored = await this.read();
    if (!stored) {
      this.publish(undefined);
      return undefined;
    }
    if (stored.access_token !== this.session?.access_token && !this.dueSoon(stored)) {
      this.publish(stored);
      return stored;
    }
    if (!stored.refresh_token) throw new Error(CODEX_SESSION_EXPIRED);
    if (this.now() - this.lastFailureAt < RETRY_AFTER_FAILURE_MS) throw new Error(this.lastFailure);
    let response: Response;
    try {
      response = await (this.opts.fetch ?? fetch)(`${CODEX_ISSUER}/oauth/token`, {
        method: 'POST',
        redirect: 'error',
        signal: AbortSignal.timeout(REFRESH_TIMEOUT_MS),
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ grant_type: 'refresh_token', refresh_token: stored.refresh_token, client_id: CODEX_CLIENT_ID }),
      });
    } catch {
      throw this.failed('Codex 登录刷新失败，请检查网络。');
    }
    if (!response.ok) {
      const body: unknown = await response.json().catch(() => undefined);
      const error = body && typeof body === 'object' ? (body as Record<string, unknown>).error : undefined;
      const code = typeof error === 'string' ? error
        : error && typeof error === 'object' ? (error as Record<string, unknown>).code : undefined;
      // Never relay the response body: it may echo the refresh token.
      if (typeof code === 'string' && TERMINAL_REFRESH_ERRORS.has(code)) throw this.failed(CODEX_SESSION_EXPIRED);
      throw this.failed(`Codex 登录刷新失败（HTTP ${response.status}）。`);
    }
    const renewed = sessionFromTokenResponse(await response.json(), this.now());
    const next: CodexOAuthSession = {
      ...renewed,
      refresh_token: renewed.refresh_token ?? stored.refresh_token,
      ...(renewed.account_id ?? stored.account_id ? { account_id: renewed.account_id ?? stored.account_id } : {}),
      ...(renewed.email ?? stored.email ? { email: renewed.email ?? stored.email } : {}),
    };
    try {
      await this.opts.store?.set(CODEX_PROVIDER_ID, JSON.stringify(next));
    } catch {
      // The old refresh token is spent; keep serving the new session from
      // memory rather than lose it, and say the next launch may need a sign-in.
      this.opts.onError?.('Codex 登录已刷新，但无法写入安全存储；重启后可能需要重新登录。');
    }
    this.lastRefreshAt = this.now();
    this.lastFailureAt = 0;
    this.publish(next);
    return next;
  }
}

/** What an engine receives: the access token, never the refresh token. */
export function codexAccessToken(session: CodexOAuthSession): CodexOAuthSession {
  return {
    access_token: session.access_token,
    expires_at: session.expires_at,
    ...(session.account_id ? { account_id: session.account_id } : {}),
    fedramp: session.fedramp,
  };
}
