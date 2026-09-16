import { createHash, randomBytes, timingSafeEqual } from 'node:crypto';
import { createServer, type Server } from 'node:http';

export const CODEX_PROVIDER_ID = 'openai-chatgpt';
const CLIENT_ID = 'app_EMoamEEZ73f0CkXaXp7hrann';
const ISSUER = 'https://auth.openai.com';

/** Main-process only: never expose this value through renderer IPC. */
export interface CodexOAuthSession {
  access_token: string;
  refresh_token?: string;
  expires_at: number;
  account_id?: string;
  fedramp: boolean;
  /** Display-only; never used to authorize a request. */
  email?: string;
}

export function parseCodexSession(value: unknown): CodexOAuthSession {
  if (!value || typeof value !== 'object') throw new Error('Codex 登录凭据无效，请重新登录。');
  const row = value as Record<string, unknown>;
  const token = (input: unknown): input is string => typeof input === 'string' && input.length > 0
    && input.length <= 16_384 && !/[\u0000-\u001f\u007f]/.test(input);
  if (!token(row.access_token) || !Number.isSafeInteger(row.expires_at) || (row.expires_at as number) <= 0
    || (row.refresh_token != null && !token(row.refresh_token))
    || (row.account_id != null && (typeof row.account_id !== 'string' || row.account_id.length > 512))
    || (row.email != null && (typeof row.email !== 'string' || row.email.length === 0
      || row.email.length > 320 || /[\u0000-\u001f\u007f]/.test(row.email)))
    || typeof row.fedramp !== 'boolean') throw new Error('Codex 登录凭据无效，请重新登录。');
  return {
    access_token: row.access_token,
    ...(row.refresh_token ? { refresh_token: row.refresh_token as string } : {}),
    expires_at: row.expires_at as number,
    ...(row.account_id ? { account_id: row.account_id as string } : {}),
    fedramp: row.fedramp,
    ...(row.email ? { email: row.email as string } : {}),
  };
}

function claims(token: unknown): Record<string, unknown> {
  if (typeof token !== 'string') return {};
  try {
    const value: unknown = JSON.parse(Buffer.from(token.split('.')[1], 'base64url').toString());
    return value && typeof value === 'object' ? value as Record<string, unknown> : {};
  } catch { return {}; }
}

/** Claims here are metadata from the TLS token response, never an authorization check. */
export function sessionFromTokenResponse(value: unknown, now = Date.now()): CodexOAuthSession {
  if (!value || typeof value !== 'object') throw new Error('Codex 登录响应无效。');
  const row = value as Record<string, unknown>;
  const identity = { ...claims(row.access_token), ...claims(row.id_token) };
  const auth = identity['https://api.openai.com/auth'] as Record<string, unknown> | undefined;
  const expiresIn = row.expires_in === undefined ? 3600 : row.expires_in;
  if (typeof expiresIn !== 'number' || !Number.isFinite(expiresIn) || expiresIn <= 0) throw new Error('Codex 登录响应有效期无效。');
  return parseCodexSession({
    access_token: row.access_token,
    refresh_token: row.refresh_token,
    expires_at: Math.floor(now / 1000 + expiresIn),
    account_id: auth?.chatgpt_account_id ?? identity.chatgpt_account_id,
    fedramp: auth?.chatgpt_account_is_fedramp === true,
    email: identity.email,
  });
}

/**
 * The ports the provider will redirect to, in order.
 *
 * 🚨 The fallback is not belt-and-braces: the callback server closes the
 * connection first, so its accepted socket lingers in TIME_WAIT holding
 * 127.0.0.1:1455 — 60 seconds on macOS — and a rebind of that port fails with
 * EADDRINUSE for the whole window. A user who cancels a Codex sign-in and
 * immediately retries lands in exactly that window, as does anyone running Codex
 * CLI's own flow. The engine's equivalent listener already falls back to 1457
 * (`llm-client/src/oauth/openai/callback.rs`), and the redirect URI is built from
 * whichever port actually bound, so both are registered with the provider.
 */
const CALLBACK_PORTS = [1455, 1457] as const;

/** Bind the first callback port that is free, preserving EADDRINUSE if none is. */
function bindCallbackPort(server: Server, preferred?: number): Promise<void> {
  const ports = preferred === undefined ? CALLBACK_PORTS : [preferred];
  const attempt = (index: number): Promise<void> => new Promise<void>((resolve, reject) => {
    const onError = (error: NodeJS.ErrnoException) => {
      if (error.code === 'EADDRINUSE' && index + 1 < ports.length) { resolve(attempt(index + 1)); return; }
      reject(error);
    };
    server.once('error', onError);
    server.listen(ports[index], '127.0.0.1', () => { server.removeListener('error', onError); resolve(); });
  });
  return attempt(0);
}

/** Codex/OpenCode's browser PKCE flow; only the system browser sees the authorization URL. */
export async function loginCodex(options: {
  openExternal: (url: string) => Promise<unknown>;
  signal: AbortSignal;
  fetch?: typeof fetch;
  port?: number;
  timeoutMs?: number;
}): Promise<CodexOAuthSession> {
  const abort = new AbortController();
  const cancel = () => abort.abort();
  options.signal.addEventListener('abort', cancel, { once: true });
  if (options.signal.aborted) abort.abort();
  const timer = setTimeout(cancel, options.timeoutMs ?? 600_000);
  const verifier = randomBytes(32).toString('base64url');
  const state = randomBytes(32).toString('base64url');
  let finish!: (code: string) => void;
  let fail!: (error: Error) => void;
  const callback = new Promise<string>((resolve, reject) => { finish = resolve; fail = reject; });
  // Observe early cancellation even while the listener/browser is starting.
  void callback.catch(() => undefined);
  const onAbort = () => fail(new Error('Codex 登录已取消或超时。'));
  abort.signal.addEventListener('abort', onAbort, { once: true });
  let accepted = false;
  let listening = false;
  const server = createServer((request, response) => {
    response.setHeader('Cache-Control', 'no-store');
    response.setHeader('Content-Type', 'text/plain; charset=utf-8');
    let url: URL;
    try { url = new URL(request.url ?? '/', 'http://localhost'); }
    catch { response.writeHead(400).end('Invalid callback'); return; }
    if (request.method !== 'GET' || url.pathname !== '/auth/callback') {
      response.writeHead(404).end('Not found'); return;
    }
    const supplied = Buffer.from(url.searchParams.get('state') ?? '');
    const expected = Buffer.from(state);
    if (supplied.length !== expected.length || !timingSafeEqual(supplied, expected)) {
      response.writeHead(400).end('Invalid OAuth state'); return;
    }
    if (accepted) { response.writeHead(409).end('Callback already received'); return; }
    accepted = true;
    const code = url.searchParams.get('code');
    if (url.searchParams.has('error') || !code || code.length > 8192) {
      response.writeHead(400).end('Sign-in failed. Return to LingXi.');
      fail(new Error('Codex 授权未完成，请重试。')); return;
    }
    response.end('Authorization received. Return to LingXi to finish signing in.');
    finish(code);
  });
  try {
    if (abort.signal.aborted) throw new Error('Codex 登录已取消。');
    // The server stays bound for the whole sign-in window (up to 10 minutes), so
    // it needs an `'error'` listener for that whole window, not just for `listen`.
    // Node emits `'error'` on the server for accept-level failures (EMFILE and
    // friends, plausible in a main process that also runs the PTY broker and the
    // recursive git watchers), and an `'error'` with no listener is rethrown —
    // which, with no `uncaughtException` handler anywhere in this app, kills the
    // desktop mid-login instead of rejecting this promise.
    server.on('error', (error) => { if (listening) fail(error); });
    await bindCallbackPort(server, options.port);
    listening = true;
    const address = server.address();
    // Prefixed `Codex ` on purpose: the catch below preserves only messages that
    // start with it, so the original spelling could never reach the caller.
    if (!address || typeof address === 'string') throw new Error('Codex 登录回调无法启动。');
    const redirect = `http://localhost:${address.port}/auth/callback`;
    const params = new URLSearchParams({
      response_type: 'code', client_id: CLIENT_ID, redirect_uri: redirect,
      scope: 'openid profile email offline_access',
      code_challenge: createHash('sha256').update(verifier).digest('base64url'),
      code_challenge_method: 'S256', state,
      id_token_add_organizations: 'true', codex_cli_simplified_flow: 'true', originator: 'lingxi',
    });
    if (abort.signal.aborted) throw new Error('Codex 登录已取消。');
    await options.openExternal(`${ISSUER}/oauth/authorize?${params}`);
    const code = await callback;
    const response = await (options.fetch ?? fetch)(`${ISSUER}/oauth/token`, {
      method: 'POST', signal: abort.signal, redirect: 'error',
      headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
      body: new URLSearchParams({ grant_type: 'authorization_code', client_id: CLIENT_ID,
        code, code_verifier: verifier, redirect_uri: redirect }),
    });
    if (!response.ok) throw new Error(`Codex 令牌交换失败（HTTP ${response.status}）。`);
    const session = sessionFromTokenResponse(await response.json());
    if (abort.signal.aborted) throw new Error('Codex 登录已取消。');
    return session;
  } catch (error) {
    if ((error as NodeJS.ErrnoException)?.code === 'EADDRINUSE') throw new Error(`Codex 登录端口 ${CALLBACK_PORTS.join(' 和 ')} 都被占用，请关闭其他登录窗口后重试。`);
    if (abort.signal.aborted) throw new Error('Codex 登录已取消或超时。');
    // Never relay provider response bodies, callback URLs or tokens as errors.
    if (error instanceof Error && error.message.startsWith('Codex ')) throw error;
    throw new Error('Codex 登录失败，请检查网络或重试。');
  } finally {
    clearTimeout(timer);
    options.signal.removeEventListener('abort', cancel);
    abort.signal.removeEventListener('abort', onAbort);
    server.close();
    server.closeAllConnections();
  }
}
