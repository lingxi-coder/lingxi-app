import { createHash, randomUUID } from 'node:crypto';
import {
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
  set(providerId: string, credential: string, identityGeneration?: number): Promise<unknown>;
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
class AccountTokenAuthority {
  private session: CodexOAuthSession | undefined;
  private loaded = false;
  private pendingPersistence?: CodexOAuthSession;
  private readonly refreshOwner = randomUUID();
  private queue: Promise<unknown> = Promise.resolve();
  private stopped = false;
  private disposed = false;
  private readonly pending = new Set<Promise<unknown>>();
  private lastRefreshAt = 0;
  private lastFailureAt = 0;
  private lastFailure = CODEX_SESSION_EXPIRED;
  private readonly listeners = new Set<(session: CodexOAuthSession | undefined) => void>();
  private timer: ReturnType<typeof setInterval> | undefined;

  constructor(private readonly opts: {
    store: CodexSessionStore;
    credentialId: string;
    /** Durable refresh intent prevents reuse after an unknown exchange outcome. */
    intentId: string;
    withLock?: <T>(operation: () => Promise<T>) => Promise<T>;
    fetch?: typeof fetch;
    /** Wall-clock milliseconds; a sleeping laptop must not delay expiry. */
    now?: () => number;
    onError?: (message: string) => void;
  }) {}

  private now(): number { return (this.opts.now ?? Date.now)(); }

  private run<T>(operation: () => Promise<T>, lockHeld = false): Promise<T> {
    if (this.disposed) return Promise.reject(new Error('Provider tokens are shutting down.'));
    const next = lockHeld ? Promise.resolve().then(operation)
      : this.queue.catch(() => undefined).then(() => this.opts.withLock ? this.opts.withLock(operation) : operation());
    if (!lockHeld) this.queue = next;
    this.pending.add(next);
    void next.then(() => this.pending.delete(next), () => this.pending.delete(next));
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
    const secret = await this.opts.store.resolve(this.opts.credentialId);
    if (!secret) return undefined;
    try { return parseCodexSession(JSON.parse(secret)); }
    catch { throw new Error('Codex 登录凭据无效，请重新登录。'); }
  }

  /** The session to use now, refreshed first when it is about to expire. */
  current(lockHeld = false): Promise<CodexOAuthSession | undefined> {
    return this.run(async () => {
      if (this.pendingPersistence) return this.refresh();
      if (this.opts.intentId && await this.opts.store.resolve(this.opts.intentId)) throw this.failed('Codex 刷新结果未确认，请重新登录。');
      if (!this.loaded || this.opts.intentId) {
        this.session = await this.read();
        this.loaded = true;
      }
      const session = this.session;
      if (session?.refresh_token && this.dueSoon(session)) {
        return this.refresh().catch(async (error: unknown) => {
          this.reportFailure(error);
          if (this.pendingPersistence || (this.opts.intentId && await this.opts.store.resolve(this.opts.intentId))
            || (error instanceof Error && error.message === CODEX_SESSION_EXPIRED)
            || !this.session || this.session.expires_at <= this.now() / 1000) throw error;
          return this.session;
        });
      }
      return session;
    }, lockHeld);
  }

  /**
   * An engine's access token expired or was rejected. Refresh, unless a
   * refresh just happened: engines asking together share one.
   */
  renew(rejectedAccessToken?: string, lockHeld = false): Promise<CodexOAuthSession | undefined> {
    return this.run(async () => {
      if (this.pendingPersistence) return this.refresh();
      if (this.opts.intentId && await this.opts.store.resolve(this.opts.intentId)) throw this.failed('Codex 刷新结果未确认，请重新登录。');
      if (!this.loaded || this.opts.intentId) {
        this.session = await this.read();
        this.loaded = true;
      }
      if (!this.session?.refresh_token) return this.session;
      if (rejectedAccessToken && this.session.access_token !== rejectedAccessToken) return this.session;
      if (!rejectedAccessToken && this.now() - this.lastRefreshAt < RECENT_REFRESH_MS) return this.session;
      return this.refresh().catch((error: unknown) => {
        this.reportFailure(error);
        throw error;
      });
    }, lockHeld);
  }

  /** Store a new sign-in and serve it to every engine. */
  signIn(session: CodexOAuthSession, identityGeneration?: number, lockHeld = false, commit?: () => Promise<void>): Promise<void> {
    return this.run(async () => {
      await this.opts.store.set(this.opts.credentialId, JSON.stringify(session), identityGeneration);
      await commit?.();
      if (this.opts.intentId) await this.opts.store.delete(this.opts.intentId);
      this.pendingPersistence = undefined;
      this.lastFailureAt = 0;
      this.lastRefreshAt = this.now();
      this.publish(session);
    }, lockHeld);
  }

  /** Forget the session and take it away from every engine. */
  signOut(lockHeld = false): Promise<void> {
    return this.run(async () => {
      await this.opts.store.delete(this.opts.credentialId);
      if (this.opts.intentId) await this.opts.store.delete(this.opts.intentId);
      this.pendingPersistence = undefined;
      this.lastFailureAt = 0;
      this.publish(undefined);
    }, lockHeld);
  }

  invalidate(): void { this.loaded = false; this.session = undefined; this.pendingPersistence = undefined; }

  /** Refresh ahead of expiry. Safe to call often. */
  check(): void {
    if (this.stopped) return;
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
    if (this.timer || this.stopped) return;
    void this.current().catch((error: unknown) => this.reportFailure(error));
    this.timer = setInterval(() => this.check(), intervalMs);
    this.timer.unref?.();
  }

  stop(): void {
    this.stopped = true;
    if (this.timer) clearInterval(this.timer);
    this.timer = undefined;
  }
  async dispose(): Promise<void> {
    this.disposed = true;
    this.stop();
    this.listeners.clear();
    await Promise.allSettled([...this.pending]);
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
    if (this.pendingPersistence) return this.persistRotation(this.pendingPersistence);
    if (this.opts.intentId && await this.opts.store.resolve(this.opts.intentId)) {
      throw this.failed('Codex 刷新结果未确认，请重新登录。');
    }
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
    if (this.opts.intentId) {
      if (!this.opts.store) throw new Error('Codex 安全凭据存储不可用。');
      await this.opts.store.set(this.opts.intentId, JSON.stringify({ owner: this.refreshOwner,
        refreshTokenDigest: createHash('sha256').update(stored.refresh_token).digest('hex'), startedAt: this.now() }));
    }
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
      if (this.opts.intentId) await this.opts.store.delete(this.opts.intentId);
      if (typeof code === 'string' && TERMINAL_REFRESH_ERRORS.has(code)) throw this.failed(CODEX_SESSION_EXPIRED);
      throw this.failed(`Codex 登录刷新失败（HTTP ${response.status}）。`);
    }
    const renewed = sessionFromTokenResponse(await response.json(), this.now());
    if ((stored.subject && renewed.subject && stored.subject !== renewed.subject)
      || (stored.account_id && renewed.account_id && stored.account_id !== renewed.account_id)) throw this.failed('Codex 账号身份发生变化，请重新登录。');
    const next: CodexOAuthSession = {
      ...renewed,
      refresh_token: renewed.refresh_token ?? stored.refresh_token,
      ...(renewed.account_id ?? stored.account_id ? { account_id: renewed.account_id ?? stored.account_id } : {}),
      ...(renewed.subject ?? stored.subject ? { subject: renewed.subject ?? stored.subject } : {}),
      ...(renewed.email ?? stored.email ? { email: renewed.email ?? stored.email } : {}),
    };
    this.pendingPersistence = next;
    return this.persistRotation(next);
  }

  private async persistRotation(next: CodexOAuthSession): Promise<CodexOAuthSession> {
    if (this.opts.intentId) {
      const intent = await this.opts.store.resolve(this.opts.intentId);
      if (!intent || JSON.parse(intent).owner !== this.refreshOwner) {
        this.pendingPersistence = undefined; this.loaded = false; this.session = undefined;
        throw this.failed('Codex 登录凭据已更改，请重试。');
      }
    }
    try {
      await this.opts.store.set(this.opts.credentialId, JSON.stringify(next));
      if (this.opts.intentId) await this.opts.store.delete(this.opts.intentId);
    } catch {
      // Never publish a rotation until durable commit. Keep only the new grant
      // in memory and retry its persistence, never exchange the spent token.
      throw this.failed('Codex 登录已刷新，但无法写入安全存储；请重试保存或重新登录。');
    }
    this.pendingPersistence = undefined;
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

/** One independent refresh authority and queue per stable local account. */
export class CodexTokenManager {
  private readonly accounts = new Map<string, AccountTokenAuthority>();
  private stopped = false;
  private disposed = false;
  private timer?: ReturnType<typeof setInterval>;
  private readonly listeners = new Set<(session: CodexOAuthSession | undefined, accountId: string) => void>();
  constructor(private readonly opts: {
    store: CodexSessionStore;
    fetch?: typeof fetch;
    now?: () => number;
    onError?: (message: string) => void;
    withAccountLock?: <T>(accountId: string, operation: () => Promise<T>) => Promise<T>;
  }) {}
  private authority(accountId: string): AccountTokenAuthority {
    if (this.disposed) throw new Error('Provider tokens are shutting down.');
    let authority = this.accounts.get(accountId);
    if (!authority) {
      if (!/^acct\.[a-f0-9-]{36}$/.test(accountId)) throw new Error('invalid OAuth account reference');
      authority = new AccountTokenAuthority({ ...this.opts, credentialId: accountId,
        intentId: `${accountId}.intent`,
        ...(this.opts.withAccountLock ? { withLock: <T>(operation: () => Promise<T>) => this.opts.withAccountLock!(accountId, operation) } : {}),
      });
      authority.subscribe(session => { for (const listener of this.listeners) listener(session, accountId); });
      if (this.stopped) authority.stop();
      this.accounts.set(accountId, authority);
    }
    return authority;
  }
  subscribe(listener: (session: CodexOAuthSession | undefined, accountId: string) => void): () => void {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  }
  current(accountId: string, lockHeld = false): Promise<CodexOAuthSession | undefined> { return this.authority(accountId).current(lockHeld); }
  renew(accountId: string, rejectedAccessToken?: string, lockHeld = false): Promise<CodexOAuthSession | undefined> { return this.authority(accountId).renew(rejectedAccessToken, lockHeld); }
  signIn(session: CodexOAuthSession, accountId: string, identityGeneration?: number, lockHeld = false, commit?: () => Promise<void>): Promise<void> { return this.authority(accountId).signIn(session, identityGeneration, lockHeld, commit); }
  async hasPendingCommit(accountId: string): Promise<boolean> { return Boolean(await this.opts.store.resolve(`${accountId}.intent`)); }
  signOut(accountId: string, lockHeld = false): Promise<void> { return this.authority(accountId).signOut(lockHeld); }
  invalidate(accountId: string): void { this.accounts.get(accountId)?.invalidate(); }
  check(): void { if (!this.stopped) for (const authority of this.accounts.values()) authority.check(); }
  start(intervalMs = 60_000): void {
    if (this.timer || this.stopped) return;
    this.timer = setInterval(() => this.check(), intervalMs);
    this.timer.unref?.();
  }
  watch(accountId: string, intervalMs?: number): void {
    if (this.stopped) return;
    const authority = this.authority(accountId);
    if (!this.timer) authority.start(intervalMs);
  }
  stop(): void {
    this.stopped = true;
    if (this.timer) clearInterval(this.timer);
    this.timer = undefined;
    for (const authority of this.accounts.values()) authority.stop();
  }
  async dispose(): Promise<void> {
    this.disposed = true;
    this.stop();
    this.listeners.clear();
    await Promise.all([...this.accounts.values()].map(authority => authority.dispose()));
  }
}
