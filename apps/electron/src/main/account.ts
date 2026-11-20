import { EventEmitter } from 'node:events';
import { ACCOUNT_SIGN_IN_METHODS, parseAccountCommand, type AccountSignInMethod, type AccountState, type AccountUser } from '../shared/account.js';
import { browserSignIn } from './account-browser.js';

export interface AccountSessionStore {
  readonly persistent: boolean;
  read(): Promise<string | null>;
  write(value: string): Promise<void>;
  remove(): Promise<void>;
}
type Session = { projectId: string; refreshToken: string; user: AccountUser };
type LookupUser = { localId: string; email?: string; displayName?: string; emailVerified?: boolean; providerUserInfo?: Array<{ providerId?: string }> };
const signInMethodsOf = (user: LookupUser): AccountSignInMethod[] =>
  ACCOUNT_SIGN_IN_METHODS.filter(method => user.providerUserInfo?.some(info => info.providerId === method));
const methodsOf = (value: unknown): AccountSignInMethod[] | undefined => Array.isArray(value)
  ? ACCOUNT_SIGN_IN_METHODS.filter(method => value.includes(method)) : undefined;
/** The `auth_time` (seconds) of an ID token: when its account last entered credentials, kept across token refreshes. */
function authTimeOf(idToken: string): number | null {
  try {
    const payload = JSON.parse(Buffer.from(idToken.split('.')[1] ?? '', 'base64url').toString('utf8')) as { auth_time?: unknown };
    return typeof payload.auth_time === 'number' && Number.isFinite(payload.auth_time) ? payload.auth_time : null;
  } catch { return null; }
}
type AuthResponse = { localId: string; idToken: string; refreshToken: string; expiresIn: string; email?: string; displayName?: string; emailVerified?: boolean; needConfirmation?: boolean };

export function accountError(error: unknown): string {
  const code = error instanceof Error ? error.message.split(' : ')[0] : '';
  const messages: Record<string, string> = {
    INVALID_LOGIN_CREDENTIALS: '邮箱或密码不正确。', EMAIL_NOT_FOUND: '邮箱或密码不正确。', INVALID_PASSWORD: '邮箱或密码不正确。',
    EMAIL_EXISTS: '此邮箱已注册，请登录或重置密码。', WEAK_PASSWORD: '密码强度不足，请使用更长的密码。',
    TOO_MANY_ATTEMPTS_TRY_LATER: '尝试次数过多，请稍后重试。', USER_DISABLED: '此账号已停用。',
    OPERATION_NOT_ALLOWED: '此登录方式尚未启用。', INVALID_IDP_RESPONSE: '登录凭据无效，请重试。',
    FEDERATED_USER_ID_ALREADY_LINKED: '此登录方式已关联其他账号。', NEED_CONFIRMATION: '请先使用原有登录方式登录此邮箱。',
    ACCOUNT_MISMATCH: '请使用当前登录的灵犀账号验证身份。',
    INVALID_REFRESH_TOKEN: '登录已失效，请重新登录。', INVALID_ID_TOKEN: '登录已失效，请重新登录。', TOKEN_EXPIRED: '登录已失效，请重新登录。', USER_NOT_FOUND: '登录已失效，请重新登录。',
  };
  if (messages[code]) return messages[code];
  if (['登录已取消。', '登录超时，请重试。', '无法打开系统浏览器。', '无法启动本机登录回调。', '请先登录。'].includes(code)) return code;
  return '操作未完成，请检查网络后重试。';
}

/** Firebase REST owns tokens in the Electron main process; IPC exposes profile data only. */
export class AccountService {
  private session: Session | null = null;
  private idToken: string | null = null;
  private expiresAt = 0;
  private busy = false;
  private error: string | null = null;
  private sessionExpired = false;
  private abort?: AbortController;
  private readonly events = new EventEmitter();
  private readonly ready: Promise<void>;
  private tokenRequest?: Promise<string>;
  private authenticationEpoch = 0;
  /** When the credentials were last entered in this process; restored and refreshed sessions do not count. */
  private authenticatedAt = 0;

  constructor(
    private readonly config: { apiKey: string; authDomain: string; projectId: string; appId: string },
    private readonly store: AccountSessionStore,
    private readonly openExternal: (url: string) => Promise<void>,
    private readonly fetcher: typeof fetch = fetch,
  ) { this.ready = this.restore(); }

  private async restore(): Promise<void> {
    try {
      const saved = await this.store.read();
      if (!saved) return;
      const value = JSON.parse(saved) as Session & { sessionExpired?: boolean };
      if (value.projectId === this.config.projectId && value.sessionExpired === true) {
        this.sessionExpired = true;
        this.error = accountError(new Error('TOKEN_EXPIRED'));
        return;
      }
      if (value.projectId !== this.config.projectId || typeof value.refreshToken !== 'string' || !value.refreshToken || typeof value.user?.uid !== 'string') throw new Error('Invalid session');
      this.session = { projectId: value.projectId, refreshToken: value.refreshToken, user: {
        uid: value.user.uid, email: typeof value.user.email === 'string' ? value.user.email : null,
        displayName: typeof value.user.displayName === 'string' ? value.user.displayName : null, emailVerified: value.user.emailVerified === true,
        ...(methodsOf(value.user.signInMethods) ? { signInMethods: methodsOf(value.user.signInMethods) } : {}),
      } };
      this.authenticationEpoch++;
    } catch { this.sessionExpired = true; this.error = '无法恢复已保存的登录状态，请重新登录。'; }
  }

  snapshot(): AccountState { return { user: this.session?.user ?? null, busy: this.busy, persistence: this.store.persistent ? 'secure' : 'session', error: this.error, ...(this.sessionExpired ? { sessionExpired: true } : {}) }; }
  /** Main-only authentication identity; normal ID/refresh-token rotation retains it. */
  sessionEpoch(): number { return this.authenticationEpoch; }
  /**
   * Main-only: whether the account entered its credentials within `maxMs`, the freshness the vault asks of a key export
   * (five minutes, which the server measures from the ID token's `auth_time`; this keeps a margin under it). A sign-in in
   * this process counts at once; otherwise the token's own `auth_time` decides, so a restart right after signing in does too.
   */
  async recentlyAuthenticated(maxMs = 240_000): Promise<boolean> {
    if (!this.session) return false;
    if (Date.now() - this.authenticatedAt < maxMs) return true;
    try { const authTime = authTimeOf(await this.getIdToken()); return authTime !== null && Date.now() - authTime * 1000 < maxMs; }
    catch { return false; }
  }
  onChanged(callback: (state: AccountState) => void): () => void {
    this.events.on('change', callback);
    return () => { this.events.off('change', callback); };
  }
  private publish(): void { this.events.emit('change', this.snapshot()); }
  private async request<T>(method: string, body: unknown): Promise<T> {
    return this.post<T>(`https://identitytoolkit.googleapis.com/v1/accounts:${method}?key=${encodeURIComponent(this.config.apiKey)}`, JSON.stringify(body), 'application/json');
  }
  private async post<T>(url: string, body: string, contentType: string): Promise<T> {
    const response = await this.fetcher(url, { method: 'POST', headers: { 'Content-Type': contentType }, body, signal: AbortSignal.timeout(30_000) });
    const value = await response.json() as { error?: { message?: string } };
    if (!response.ok) throw new Error(value.error?.message ?? 'AUTH_REQUEST_FAILED');
    return value as T;
  }
  private async save(): Promise<void> {
    if (this.session) await this.store.write(JSON.stringify(this.session));
    else if (this.sessionExpired) await this.store.write(JSON.stringify({ projectId: this.config.projectId, sessionExpired: true }));
    else await this.store.remove();
  }
  private async invalidateSession(error: unknown, session: Session | null): Promise<void> {
    if (!session || this.session !== session || !(error instanceof Error)
      || !['INVALID_REFRESH_TOKEN', 'INVALID_ID_TOKEN', 'TOKEN_EXPIRED', 'USER_DISABLED', 'USER_NOT_FOUND'].includes(error.message.split(' : ')[0])) return;
    this.session = null;
    this.authenticationEpoch++;
    this.tokenRequest = undefined;
    this.idToken = null;
    this.expiresAt = 0;
    this.sessionExpired = true;
    this.error = accountError(error);
    this.publish();
    await this.save();
  }
  private async authenticatedRequest<T>(method: string, body: Record<string, unknown> = {}): Promise<T> {
    const session = this.session;
    try {
      return await this.request<T>(method, { ...body, idToken: await this.getIdToken() });
    } catch (error) {
      await this.invalidateSession(error, session);
      throw error;
    }
  }
  /** `expectUid` re-authenticates: the response must be the signed-in account, which then keeps its session epoch. */
  private async accept(value: AuthResponse, expectUid?: string): Promise<void> {
    if (value.needConfirmation) throw new Error('NEED_CONFIRMATION');
    if (!value.localId || !value.refreshToken || !value.idToken) throw new Error('INVALID_IDP_RESPONSE');
    if (expectUid !== undefined && (this.session?.user.uid !== expectUid || value.localId !== expectUid)) throw new Error('ACCOUNT_MISMATCH');
    const previous = this.session;
    // A re-authentication keeps what is known of the account; a new sign-in learns it with `reload`.
    const signInMethods = expectUid !== undefined ? previous?.user.signInMethods : undefined;
    const previousToken = this.idToken, previousExpiry = this.expiresAt, previousExpired = this.sessionExpired, previousAuthenticatedAt = this.authenticatedAt;
    this.session = { projectId: this.config.projectId, refreshToken: value.refreshToken, user: {
      uid: value.localId, email: value.email ?? null, displayName: value.displayName ?? null, emailVerified: value.emailVerified === true,
      ...(signInMethods ? { signInMethods } : {}),
    } };
    if (expectUid === undefined) this.authenticationEpoch++;
    this.tokenRequest = undefined;
    this.sessionExpired = false;
    this.authenticatedAt = Date.now();
    this.idToken = value.idToken;
    this.expiresAt = Date.now() + Number(value.expiresIn) * 1000;
    this.publish();
    try { await this.save(); } catch (error) {
      this.session = previous; this.idToken = previousToken; this.expiresAt = previousExpiry; this.sessionExpired = previousExpired;
      this.authenticatedAt = previousAuthenticatedAt;
      if (expectUid === undefined) this.authenticationEpoch++;
      this.tokenRequest = undefined; this.publish(); throw error;
    }
  }

  /**
   * For host-side authenticated requests only. Never forward this through generic renderer IPC.
   * `forceRefresh` skips the cached token, for a caller whose request was refused with it.
   */
  async getIdToken(forceRefresh = false): Promise<string> {
    await this.ready;
    if (!this.session) throw new Error('请先登录。');
    if (!forceRefresh && this.idToken && this.expiresAt > Date.now() + 60_000) return this.idToken;
    if (this.tokenRequest) return this.tokenRequest;
    const request = this.refreshToken().finally(() => { if (this.tokenRequest === request) this.tokenRequest = undefined; });
    this.tokenRequest = request;
    return request;
  }
  private async refreshToken(): Promise<string> {
    const session = this.session!;
    try {
      const response = await this.post<{ id_token: string; refresh_token: string; expires_in: string; user_id: string }>(
        `https://securetoken.googleapis.com/v1/token?key=${encodeURIComponent(this.config.apiKey)}`,
        new URLSearchParams({ grant_type: 'refresh_token', refresh_token: session.refreshToken }).toString(), 'application/x-www-form-urlencoded',
      );
      if (this.session !== session) throw new Error('登录已取消。');
      if (!response.id_token || !response.refresh_token || response.user_id !== session.user.uid) throw new Error('INVALID_REFRESH_TOKEN');
      session.refreshToken = response.refresh_token;
      this.idToken = response.id_token;
      this.expiresAt = Date.now() + Number(response.expires_in) * 1000;
      await this.save();
      return response.id_token;
    } catch (error) {
      await this.invalidateSession(error, session);
      throw error;
    }
  }
  private async reload(): Promise<void> {
    const session = this.session;
    const response = await this.authenticatedRequest<{ users?: LookupUser[] }>('lookup');
    const user = response.users?.[0];
    if (session && this.session === session && user?.localId === session.user.uid) {
      session.user = { uid: user.localId, email: user.email ?? null, displayName: user.displayName ?? null, emailVerified: user.emailVerified === true,
        signInMethods: signInMethodsOf(user) };
      await this.save();
    }
  }

  async execute(value: unknown): Promise<AccountState> {
    const command = parseAccountCommand(value);
    await this.ready;
    if (command.type === 'status') return this.snapshot();
    if (command.type === 'cancel') { this.abort?.abort(); return this.snapshot(); }
    if (this.busy) throw new Error('请等待当前账号操作完成。');
    this.busy = true; this.error = null; this.publish();
    try {
      switch (command.type) {
        case 'signIn': case 'register':
          if (command.type === 'signIn' && command.expectUid !== undefined && this.session?.user.uid !== command.expectUid) throw new Error('ACCOUNT_MISMATCH');
          await this.accept(await this.request<AuthResponse>(command.type === 'register' ? 'signUp' : 'signInWithPassword', { email: command.email, password: command.password, returnSecureToken: true }),
            command.type === 'signIn' ? command.expectUid : undefined);
          await this.reload();
          break;
        case 'oauth': {
          if (command.expectUid !== undefined && this.session?.user.uid !== command.expectUid) throw new Error('ACCOUNT_MISMATCH');
          this.abort = new AbortController();
          // A re-authentication names the account, so the page preselects it and never leaves another account behind.
          const credential = await browserSignIn(this.config, command.provider, this.openExternal, this.abort.signal,
            command.expectUid !== undefined ? { uid: command.expectUid, email: this.session?.user.email ?? null } : undefined);
          const postBody = new URLSearchParams({ providerId: credential.providerId });
          if (credential.idToken) postBody.set('id_token', credential.idToken);
          if (credential.accessToken) postBody.set('access_token', credential.accessToken);
          if (credential.nonce) postBody.set('nonce', credential.nonce);
          const response = await this.request<AuthResponse>('signInWithIdp', {
            requestUri: 'http://127.0.0.1', returnSecureToken: true,
            ...(credential.pendingToken ? { pendingToken: credential.pendingToken } : { postBody: postBody.toString() }),
          });
          if (this.abort.signal.aborted) throw new Error('登录已取消。');
          await this.accept(response, command.expectUid);
          if (command.expectUid === undefined) await this.reload().catch(() => undefined);
          break;
        }
        case 'signOut':
          await this.store.remove(); this.session = null; this.idToken = null; this.expiresAt = 0;
          this.authenticationEpoch++; this.tokenRequest = undefined;
          break;
        case 'resetPassword':
          await this.request('sendOobCode', { requestType: 'PASSWORD_RESET', email: command.email });
          break;
        case 'verifyEmail':
          await this.authenticatedRequest('sendOobCode', { requestType: 'VERIFY_EMAIL' });
          break;
        case 'reload': await this.reload(); break;
      }
    } catch (error) { this.error = accountError(error); }
    finally { this.abort = undefined; this.busy = false; this.publish(); }
    return this.snapshot();
  }
  dispose(): void { this.abort?.abort(); this.events.removeAllListeners(); }
}
