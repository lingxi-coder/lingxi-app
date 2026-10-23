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
    if (input[key] !== undefined) {
      if (typeof input[key] !== 'string' || input[key].length > 32_768) throw new Error('Invalid sign-in response');
      result[key] = input[key];
    }
  }
  if (!result.idToken && !result.accessToken && !result.pendingToken) throw new Error('Missing sign-in credential');
  return result;
}

// Only the system browser sees the provider UI. The renderer never receives tokens.
export async function browserSignIn(
  config: { apiKey: string; authDomain: string; projectId: string; appId: string },
  provider: BrowserCredential['providerId'],
  openExternal: (url: string) => Promise<void>,
  signal: AbortSignal,
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
    const finish = (error?: Error, credential?: BrowserCredential) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      signal.removeEventListener('abort', abort);
      server.close();
      server.closeAllConnections();
      if (error) reject(error); else resolve(credential!);
    };
    const abort = () => finish(new Error('登录已取消。'));
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
        res.end(`<!doctype html><html lang="zh"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>LingXi 登录</title><style nonce="${secret}">body{font:16px system-ui;max-width:440px;margin:15vh auto;padding:24px;color:#202124}button{font:inherit;padding:12px 24px;border-radius:10px;border:1px solid #ccc;background:#fff;cursor:pointer}p{line-height:1.6}</style><h1>登录 LingXi</h1><p>使用 ${label} 账号继续。</p><button id="login">通过 ${label} 登录</button><p id="status" role="status"></p><script type="module" nonce="${secret}">
const { initializeApp } = await import('/firebase-app.js');
const { initializeAuth, inMemoryPersistence, browserPopupRedirectResolver, GoogleAuthProvider, OAuthProvider, signInWithPopup, signOut } = await import('/firebase-auth.js');
const auth = initializeAuth(initializeApp(${JSON.stringify(config).replaceAll('<', '\\u003c')}), { persistence: inMemoryPersistence, popupRedirectResolver: browserPopupRedirectResolver });
const provider = ${provider === 'google.com' ? 'new GoogleAuthProvider()' : "new OAuthProvider('apple.com')"};
provider.addScope('email');
const button = document.getElementById('login'), status = document.getElementById('status');
button.onclick = async () => {
 button.disabled = true; status.textContent = '正在登录…';
 try {
  const result = await signInWithPopup(auth, provider);
  const credential = ${provider === 'google.com' ? 'GoogleAuthProvider' : 'OAuthProvider'}.credentialFromResult(result);
  if (!credential) throw new Error('没有收到登录凭据。');
  const response = await fetch('${path}/complete', {method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(credential.toJSON())});
  if (!response.ok) throw new Error('无法连接 LingXi，请返回应用重试。');
  status.textContent = '已返回 LingXi 完成验证，请切回应用。';
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
      if (req.method !== 'POST' || ![`${path}/complete`, `${path}/cancel`].includes(req.url ?? '') || req.headers.origin !== origin || req.headers['content-type'] !== 'application/json') {
        res.writeHead(403).end(); return;
      }
      try {
        let body = '';
        for await (const chunk of req) {
          body += chunk.toString();
          if (Buffer.byteLength(body) > 96 * 1024) { res.writeHead(413).end(); return; }
        }
        const value: unknown = JSON.parse(body);
        if (req.url === `${path}/cancel`) {
          res.writeHead(200).end('{}', () => finish(new Error('登录已取消。')));
          return;
        }
        const credential = parseBrowserCredential(value, provider);
        res.writeHead(200).end('{}', () => finish(undefined, credential));
      } catch { res.writeHead(400).end(); }
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
