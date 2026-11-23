import { createServer } from 'node:http';
import { randomBytes } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { createRequire } from 'node:module';

export interface BrowserCredential {
  providerId: 'google.com' | 'apple.com';
  idToken?: string;
  accessToken?: string;
  nonce?: string;
  pendingToken?: string;
}

export function parseBrowserCredential(value: unknown, provider: BrowserCredential['providerId']): BrowserCredential {
  if (!value || typeof value !== 'object') throw new Error('Invalid sign-in response');
  const input = value as Record<string, unknown>;
  if (input.providerId !== provider) throw new Error('Unexpected sign-in provider');
  const result: BrowserCredential = { providerId: provider };
  for (const key of ['idToken', 'accessToken', 'nonce', 'pendingToken'] as const) {
    // Firebase serializes absent optional OAuth fields (notably Google's
    // pendingToken) as null. Only non-null values carry a credential.
    if (input[key] !== undefined && input[key] !== null) {
      if (typeof input[key] !== 'string' || input[key].length > 32_768) throw new Error('Invalid sign-in response');
      result[key] = input[key];
    }
  }
  if (!result.idToken && !result.accessToken && !result.pendingToken) throw new Error('Missing sign-in credential');
  return result;
}

/**
 * Only the system browser sees the provider UI. The renderer never receives tokens.
 * `reauth` names the signed-in account a re-authentication must be: the provider preselects it where it can (Google's
 * login_hint), and another account is refused in the page (`ACCOUNT_MISMATCH`). The page's sign-in creates a Firebase
 * user for an identity that had none, so such a user, which Firebase reports as new to this very sign-in, is deleted there
 * rather than left behind as an empty account.
 */
export async function browserSignIn(
  config: { apiKey: string; authDomain: string; projectId: string; appId: string },
  provider: BrowserCredential['providerId'],
  openExternal: (url: string) => Promise<void>,
  signal: AbortSignal,
  reauth?: { uid: string; email: string | null },
  complete?: (credential: BrowserCredential) => Promise<void>,
): Promise<BrowserCredential> {
  const secret = randomBytes(32).toString('hex');
  const path = `/${secret}`;
  const sdkRoot = dirname(createRequire(import.meta.url).resolve('firebase/package.json'));
  const version = JSON.parse(readFileSync(join(sdkRoot, 'package.json'), 'utf8')).version as string;
  const sdkApp = readFileSync(join(sdkRoot, 'firebase-app.js'), 'utf8');
  const sdkAuth = readFileSync(join(sdkRoot, 'firebase-auth.js'), 'utf8')
    .replaceAll(`https://www.gstatic.com/firebasejs/${version}/firebase-app.js`, '/firebase-app.js');
  return new Promise((resolve, reject) => {
    let origin = '';
    let settled = false;
    let completing = false;
    const finish = (error?: Error, credential?: BrowserCredential) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      signal.removeEventListener('abort', abort);
      server.close();
      server.closeAllConnections();
      if (error) reject(error); else resolve(credential!);
    };
    const abort = () => { if (!completing) finish(new Error('登录已取消。')); };
    const timer = setTimeout(() => finish(new Error('登录超时，请重试。')), 180_000);
    const server = createServer(async (req, res) => {
      res.setHeader('Cache-Control', 'no-store');
      res.setHeader('Referrer-Policy', 'no-referrer');
      res.setHeader('X-Content-Type-Options', 'nosniff');
      if (req.headers.host !== new URL(origin).host) { res.writeHead(403).end(); return; }
      if (req.method === 'GET' && ['/firebase-app.js', '/firebase-auth.js'].includes(req.url ?? '')) {
        res.setHeader('Content-Type', 'text/javascript; charset=utf-8');
        res.end(req.url === '/firebase-app.js' ? sdkApp : sdkAuth);
        return;
      }
      if (req.method === 'GET' && req.url === path) {
        res.setHeader('Content-Type', 'text/html; charset=utf-8');
        res.setHeader('Content-Security-Policy', `default-src 'none'; script-src 'self' 'nonce-${secret}' https://apis.google.com; connect-src 'self' https://identitytoolkit.googleapis.com https://securetoken.googleapis.com https://${config.authDomain}; frame-src https://${config.authDomain}; style-src 'nonce-${secret}'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'`);
        const label = provider === 'google.com' ? 'Google' : 'Apple';
        // Dynamic imports keep electron-vite's ESM shim scanner from treating
        // browser imports inside this HTML string as imports of the Node bundle.
        res.end(`<!doctype html>
<html lang="zh-CN">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>LingXi 登录</title>
<style nonce="${secret}">
  :root { color-scheme: light dark; }
  body { margin: 0; background: light-dark(#fdfdfb, #191a1b); color: light-dark(#202421, #f0f2ef); font: 1rem/1.5 system-ui, sans-serif; }
  main { box-sizing: border-box; min-height: 100vh; min-height: 100dvh; display: grid; place-items: center; padding: 1.5rem; text-align: center; }
  h1 { margin: 0 0 .5rem; font-size: 1.375rem; font-weight: 625; letter-spacing: -.025em; }
  p { margin: 0; color: light-dark(#5e675f, #adb5af); }
  button { margin: 1rem 0 .5rem; font: inherit; padding: .625rem 1.125rem; border-radius: .625rem; border: 1px solid light-dark(#cbd1cc, #5b655d); cursor: pointer; }
  button:disabled { cursor: default; }
  #status { font-size: .875rem; }
  .success p { font-size: .9375rem; }
  .success h1:focus { outline: none; }
  .success-mark { display: grid; place-items: center; width: 2.25rem; height: 2.25rem; margin: 0 auto .875rem; border-radius: 50%; background: light-dark(#e1eee5, #25372c); color: light-dark(#286342, #acd9bb); font-weight: 650; }
</style>
<main>
  <section id="sign-in">
    <h1>登录 LingXi</h1>
    <p>使用 ${label} 账号继续。</p>
    <button id="login">通过 ${label} 登录</button>
    <p id="status" role="status"></p>
  </section>
  <section id="success" class="success" hidden>
    <span class="success-mark" aria-hidden="true">✓</span>
    <h1 tabindex="-1">已登录 LingXi</h1>
    <p>可以关闭此网页，返回 LingXi 继续。</p>
  </section>
</main>
<script type="module" nonce="${secret}">
const { initializeApp } = await import('/firebase-app.js');
const { initializeAuth, inMemoryPersistence, browserPopupRedirectResolver, GoogleAuthProvider, OAuthProvider, signInWithPopup, signOut, getAdditionalUserInfo, deleteUser } = await import('/firebase-auth.js');
const reauth = ${JSON.stringify(reauth ?? null).replaceAll('<', '\\u003c')};
const auth = initializeAuth(initializeApp(${JSON.stringify(config).replaceAll('<', '\\u003c')}), { persistence: inMemoryPersistence, popupRedirectResolver: browserPopupRedirectResolver });
const provider = ${provider === 'google.com' ? 'new GoogleAuthProvider()' : "new OAuthProvider('apple.com')"};
provider.addScope('email');
if (reauth?.email && ${provider === 'google.com'}) provider.setCustomParameters({ login_hint: reauth.email });
const button = document.getElementById('login'), status = document.getElementById('status');
button.onclick = async () => {
 button.disabled = true; status.textContent = '正在登录…';
 try {
  const result = await signInWithPopup(auth, provider);
  if (reauth && result.user.uid !== reauth.uid) {
   // Created by this sign-in only: nothing else can be in it yet.
   if (getAdditionalUserInfo(result)?.isNewUser) await deleteUser(result.user).catch(() => {});
   await fetch('${path}/mismatch', {method:'POST',headers:{'Content-Type':'application/json'},body:'{}'}).catch(() => {});
   status.textContent = '这不是当前登录的灵犀账号，请返回 LingXi 使用该账号验证。'; button.disabled = false;
   return;
  }
  const credential = ${provider === 'google.com' ? 'GoogleAuthProvider' : 'OAuthProvider'}.credentialFromResult(result);
  if (!credential) throw new Error('没有收到登录凭据。');
  const response = await fetch('${path}/complete', {method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(credential.toJSON())});
  if (!response.ok) {
   status.textContent = 'LingXi 未能完成登录，请切回应用查看原因。';
   return;
  }
  document.title = reauth ? '已验证 LingXi 账号' : '已登录 LingXi';
  document.getElementById('sign-in').hidden = true;
  const success = document.getElementById('success');
  const heading = success.querySelector('h1');
  if (reauth) heading.textContent = document.title;
  success.hidden = false;
  heading.focus();
 } catch(error) {
  if (error.code === 'auth/popup-closed-by-user') {
   try {
    const response = await fetch('${path}/cancel', {method:'POST',headers:{'Content-Type':'application/json'},body:'{}'});
    if (!response.ok) throw new Error('callback failed');
    status.textContent = '登录已取消，请返回 LingXi。';
   } catch { status.textContent = '无法连接 LingXi，请返回应用取消登录。'; }
  } else { status.textContent = '登录未完成，请确认网络或登录服务配置后重试。'; button.disabled = false; }
 }
 finally { await signOut(auth).catch(() => {}); }
};
</script></html>`);
        return;
      }
      if (req.method !== 'POST' || ![`${path}/complete`, `${path}/cancel`, `${path}/mismatch`].includes(req.url ?? '') || req.headers.origin !== origin || req.headers['content-type'] !== 'application/json') {
        res.writeHead(403).end(); return;
      }
      if (completing) { res.writeHead(409).end(); return; }
      let credential: BrowserCredential;
      try {
        let body = '';
        for await (const chunk of req) {
          body += chunk.toString();
          if (Buffer.byteLength(body) > 96 * 1024) { res.writeHead(413).end(); return; }
        }
        const value: unknown = JSON.parse(body);
        if (settled || completing) { res.writeHead(409).end(); return; }
        if (req.url === `${path}/cancel`) {
          res.writeHead(200).end('{}', () => finish(new Error('登录已取消。')));
          return;
        }
        if (req.url === `${path}/mismatch`) {
          res.writeHead(200).end('{}', () => finish(new Error('ACCOUNT_MISMATCH')));
          return;
        }
        credential = parseBrowserCredential(value, provider);
      } catch { res.writeHead(400).end(); return; }

      completing = true;
      clearTimeout(timer);
      let failure: Error | undefined;
      try { await complete?.(credential); }
      catch (error) { failure = error instanceof Error ? error : new Error('INVALID_IDP_RESPONSE'); }

      // If the browser tab closes mid-exchange, still settle the app's request.
      const sent = () => finish(failure, credential);
      if (res.destroyed) sent();
      else {
        res.once('close', sent);
        res.writeHead(failure ? 502 : 200).end('{}', sent);
      }
    });
    server.requestTimeout = 15_000;
    server.on('error', () => finish(new Error('无法启动本机登录回调。')));
    signal.addEventListener('abort', abort, { once: true });
    if (signal.aborted) { abort(); return; }
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      if (!address || typeof address === 'string' || settled) return;
      origin = `http://127.0.0.1:${address.port}`;
      void openExternal(`${origin}${path}`).catch(() => finish(new Error('无法打开系统浏览器。')));
    });
  });
}
