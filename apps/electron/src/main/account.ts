import { EventEmitter } from 'node:events';
import { parseAccountCommand, type AccountState, type AccountUser } from '../shared/account.js';
import { browserSignIn } from './account-browser.js';

export interface AccountSessionStore {
  readonly persistent: boolean;
  read(): Promise<string | null>;
  write(value: string): Promise<void>;
  remove(): Promise<void>;
}
type Session = { projectId: string; refreshToken: string; user: AccountUser };
type AuthResponse = { localId: string; idToken: string; refreshToken: string; expiresIn: string; email?: string; displayName?: string; emailVerified?: boolean; needConfirmation?: boolean };

export function accountError(error: unknown): string {
  const code = error instanceof Error ? error.message.split(' : ')[0] : '';
  const messages: Record<string, string> = {
    INVALID_LOGIN_CREDENTIALS: '邮箱或密码不正确。', EMAIL_NOT_FOUND: '邮箱或密码不正确。', INVALID_PASSWORD: '邮箱或密码不正确。',
    EMAIL_EXISTS: '此邮箱已注册，请登录或重置密码。', WEAK_PASSWORD: '密码强度不足，请使用更长的密码。',
    TOO_MANY_ATTEMPTS_TRY_LATER: '尝试次数过多，请稍后重试。', USER_DISABLED: '此账号已停用。',
    OPERATION_NOT_ALLOWED: '此登录方式尚未启用。', INVALID_IDP_RESPONSE: '登录凭据无效，请重试。',
    FEDERATED_USER_ID_ALREADY_LINKED: '此登录方式已关联其他账号。', NEED_CONFIRMATION: '请先使用原有登录方式登录此邮箱。',
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
  private abort?: AbortController;
  private readonly events = new EventEmitter();
  private readonly ready: Promise<void>;
  private tokenRequest?: Promise<string>;

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
      const value = JSON.parse(saved) as Session;
      if (value.projectId !== this.config.projectId || typeof value.refreshToken !== 'string' || !value.refreshToken || typeof value.user?.uid !== 'string') throw new Error('Invalid session');
      this.session = { projectId: value.projectId, refreshToken: value.refreshToken, user: {
        uid: value.user.uid, email: typeof value.user.email === 'string' ? value.user.email : null,
        displayName: typeof value.user.displayName === 'string' ? value.user.displayName : null, emailVerified: value.user.emailVerified === true,
      } };
    } catch { this.error = '无法恢复已保存的登录状态，请重新登录。'; }
  }

  snapshot(): AccountState { return { user: this.session?.user ?? null, busy: this.busy, persistence: this.store.persistent ? 'secure' : 'session', error: this.error }; }
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
    if (this.session) await this.store.write(JSON.stringify(this.session)); else await this.store.remove();
  }
  private async invalidateSession(error: unknown, session: Session | null): Promise<void> {
    if (!session || this.session !== session || !(error instanceof Error)
      || !['INVALID_REFRESH_TOKEN', 'INVALID_ID_TOKEN', 'TOKEN_EXPIRED', 'USER_DISABLED', 'USER_NOT_FOUND'].includes(error.message.split(' : ')[0])) return;
    this.session = null;
    this.idToken = null;
    this.expiresAt = 0;
    try { await this.save(); } finally { this.publish(); }
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
  private async accept(value: AuthResponse): Promise<void> {
    if (value.needConfirmation) throw new Error('NEED_CONFIRMATION');
    if (!value.localId || !value.refreshToken || !value.idToken) throw new Error('INVALID_IDP_RESPONSE');
    const previous = this.session;
    this.session = { projectId: this.config.projectId, refreshToken: value.refreshToken, user: {
      uid: value.localId, email: value.email ?? null, displayName: value.displayName ?? null, emailVerified: value.emailVerified === true,
    } };
    try { await this.save(); } catch (error) { this.session = previous; throw error; }
    this.idToken = value.idToken;
    this.expiresAt = Date.now() + Number(value.expiresIn) * 1000;
  }

  /** For host-side authenticated requests only. Never forward this through generic renderer IPC. */
  async getIdToken(): Promise<string> {
    await this.ready;
    if (!this.session) throw new Error('请先登录。');
    if (this.idToken && this.expiresAt > Date.now() + 60_000) return this.idToken;
    if (this.tokenRequest) return this.tokenRequest;
    this.tokenRequest = this.refreshToken().finally(() => { this.tokenRequest = undefined; });
    return this.tokenRequest;
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
    const response = await this.authenticatedRequest<{ users?: Array<{ localId: string; email?: string; displayName?: string; emailVerified?: boolean }> }>('lookup');
    const user = response.users?.[0];
    if (session && this.session === session && user?.localId === session.user.uid) {
      session.user = { uid: user.localId, email: user.email ?? null, displayName: user.displayName ?? null, emailVerified: user.emailVerified === true };
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
          await this.accept(await this.request<AuthResponse>(command.type === 'register' ? 'signUp' : 'signInWithPassword', { email: command.email, password: command.password, returnSecureToken: true }));
          await this.reload();
          break;
        case 'oauth': {
          this.abort = new AbortController();
          const credential = await browserSignIn(this.config, command.provider, this.openExternal, this.abort.signal);
          const postBody = new URLSearchParams({ providerId: credential.providerId });
          if (credential.idToken) postBody.set('id_token', credential.idToken);
          if (credential.accessToken) postBody.set('access_token', credential.accessToken);
          if (credential.nonce) postBody.set('nonce', credential.nonce);
          const response = await this.request<AuthResponse>('signInWithIdp', {
            requestUri: 'http://127.0.0.1', returnSecureToken: true,
            ...(credential.pendingToken ? { pendingToken: credential.pendingToken } : { postBody: postBody.toString() }),
          });
          if (this.abort.signal.aborted) throw new Error('登录已取消。');
          await this.accept(response);
          break;
        }
        case 'signOut':
          await this.store.remove(); this.session = null; this.idToken = null; this.expiresAt = 0;
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
