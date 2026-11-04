import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { GoogleAuthProvider, OAuthCredential } from 'firebase/auth';
import { AccountService, type AccountSessionStore } from '../src/main/account.ts';
import { browserSignIn, parseBrowserCredential } from '../src/main/account-browser.ts';
import { parseAccountCommand, type AccountState } from '../src/shared/account.ts';

const config = { projectId: 'demo-lingxi', appId: 'demo-app', apiKey: 'demo-key', authDomain: 'demo-lingxi.firebaseapp.com' };
function store(initial: string | null = null): AccountSessionStore & { value: string | null } {
  return { value: initial, persistent: true, async read() { return this.value; }, async write(v) { this.value = v; }, async remove() { this.value = null; } };
}
const session = () => JSON.stringify({ projectId: config.projectId, refreshToken: 'refresh-secret', user: { uid: 'u1', email: 'a@example.com', displayName: null, emailVerified: false } });
const authResponse = { localId: 'u1', idToken: 'id-secret', refreshToken: 'refresh-secret', expiresIn: '3600', email: 'a@example.com' };
function mockFetch(responder: (url: string, body: Record<string, string>) => Response | Promise<Response>): typeof fetch {
  return (async (input, init) => {
    const body = String(init?.body);
    return responder(String(input), body.startsWith('{') ? JSON.parse(body) : Object.fromEntries(new URLSearchParams(body)));
  }) as typeof fetch;
}

test('account IPC validates input, trims email, never trims password', () => {
  assert.deepEqual(parseAccountCommand({ type: 'signIn', email: ' a@example.com ', password: ' secret ' }), { type: 'signIn', email: 'a@example.com', password: ' secret ' });
  for (const input of [null, { type: 'token' }, { type: 'oauth', provider: 'attacker.com' }, { type: 'register', email: 'invalid', password: 'password' }]) assert.throws(() => parseAccountCommand(input));
});
test('registration persists refresh token only, returns verified profile and signs out cleanly', async () => {
  const saved = store();
  const service = new AccountService(config, saved, async () => {}, mockFetch((url) => Response.json(url.includes('lookup') ? { users: [{ localId: 'u1', email: 'a@example.com', emailVerified: true }] } : authResponse)));
  const events: unknown[] = []; service.onChanged(s => events.push(s));
  const result = await service.execute({ type: 'register', email: 'a@example.com', password: 'password-secret' });
  assert.equal(result.user?.emailVerified, true);
  assert.equal(JSON.parse(saved.value!).refreshToken, 'refresh-secret');
  assert.ok(!saved.value!.includes('id-secret'));
  assert.ok(!JSON.stringify([events, result]).includes('secret'));
  assert.equal((await service.execute({ type: 'signOut' })).user, null);
  assert.equal(saved.value, null);
});
test('restored sessions refresh once for concurrent requests and persist rotated tokens', async () => {
  const saved = store(session()); let requests = 0;
  const service = new AccountService(config, saved, async () => {}, mockFetch(async (_url, body) => {
    requests++; assert.equal(body.refresh_token, 'refresh-secret');
    await new Promise(resolve => setTimeout(resolve, 5));
    return Response.json({ id_token: 'new-id', refresh_token: 'new-refresh', user_id: 'u1', expires_in: '3600' });
  }));
  assert.deepEqual(await Promise.all([service.getIdToken(), service.getIdToken()]), ['new-id', 'new-id']);
  assert.equal(requests, 1); assert.equal(JSON.parse(saved.value!).refreshToken, 'new-refresh');
});
test('forceRefresh skips the cached ID token, and concurrent forced calls share one request', async () => {
  const saved = store(session()); let requests = 0;
  const service = new AccountService(config, saved, async () => {}, mockFetch(async () => {
    requests++;
    await new Promise(resolve => setTimeout(resolve, 5));
    return Response.json({ id_token: `id-${requests}`, refresh_token: `refresh-${requests}`, user_id: 'u1', expires_in: '3600' });
  }));
  assert.equal(await service.getIdToken(), 'id-1');
  assert.equal(await service.getIdToken(), 'id-1'); assert.equal(requests, 1);
  assert.deepEqual(await Promise.all([service.getIdToken(true), service.getIdToken(true)]), ['id-2', 'id-2']);
  assert.equal(requests, 2);
  assert.equal(await service.getIdToken(), 'id-2');
});
test('revoked sessions clear local state; network failures retain recoverable sessions', async () => {
  for (const code of ['INVALID_REFRESH_TOKEN', 'NETWORK_ERROR']) {
    const saved = store(session());
    const service = new AccountService(config, saved, async () => {}, mockFetch(() => Response.json({ error: { message: code } }, { status: 400 })));
    const state = await service.execute({ type: 'reload' });
    assert.ok(state.error); assert.equal(state.user === null, code === 'INVALID_REFRESH_TOKEN');
    if (code === 'INVALID_REFRESH_TOKEN') assert.deepEqual(JSON.parse(saved.value!), { projectId: config.projectId, sessionExpired: true });
    else assert.equal(saved.value, session());
    assert.equal(state.sessionExpired === true, code === 'INVALID_REFRESH_TOKEN');
  }
});
test('background token expiry publishes a sign-in requirement, cleared only by successful authentication', async () => {
  const saved = store(session());
  let authenticate = false;
  const service = new AccountService(config, saved, async () => {}, mockFetch((url) => {
    if (url.includes('securetoken')) return Response.json({ error: { message: 'TOKEN_EXPIRED' } }, { status: 400 });
    if (!authenticate) return Response.json({ error: { message: 'INVALID_LOGIN_CREDENTIALS' } }, { status: 400 });
    return Response.json(url.includes('lookup') ? { users: [{ localId: 'u1' }] } : authResponse);
  }));
  const events: AccountState[] = []; service.onChanged(value => events.push(value));
  await assert.rejects(service.getIdToken(), /TOKEN_EXPIRED/);
  assert.equal(events.at(-1)?.sessionExpired, true);
  assert.match(events.at(-1)!.error!, /重新登录/);
  assert.equal((await service.execute({ type: 'signIn', email: 'a@example.com', password: 'fixture' })).sessionExpired, true);
  authenticate = true;
  const signedIn = await service.execute({ type: 'signIn', email: 'a@example.com', password: 'fixture' });
  assert.equal(signedIn.user?.uid, 'u1');
  assert.equal(signedIn.sessionExpired, undefined);
  assert.ok(!JSON.stringify(events).includes('secret'));
  const restarted = new AccountService(config, saved, async () => {}, mockFetch(() => { throw Error('expired marker must not refresh'); }));
  // Successful sign-in replaced the marker with the newly authenticated session.
  assert.equal((await restarted.execute({ type: 'status' })).sessionExpired, undefined);
  restarted.dispose();
  service.dispose();
});
test('an expired-session marker survives restart without retaining revoked credentials', async () => {
  const saved = store(session());
  const service = new AccountService(config, saved, async () => {}, mockFetch(() => Response.json({ error: { message: 'INVALID_REFRESH_TOKEN' } }, { status: 400 })));
  await assert.rejects(service.getIdToken());
  assert.ok(!saved.value!.includes('secret'));
  service.dispose();
  const restarted = new AccountService(config, saved, async () => {}, mockFetch(() => { throw Error('must not refresh a revoked credential'); }));
  const state = await restarted.execute({ type: 'status' });
  assert.equal(state.user, null);
  assert.equal(state.sessionExpired, true);
  assert.match(state.error!, /重新登录/);
  restarted.dispose();
});
test('sign-out during background refresh cannot resurrect a session', async () => {
  let release!: (r: Response) => void;
  let started!: () => void;
  const ready = new Promise<void>(resolve => { started = resolve; });
  const saved = store(session());
  const service = new AccountService(config, saved, async () => {}, mockFetch(() => { started(); return new Promise(resolve => { release = resolve; }); }));
  const pending = service.getIdToken();
  await ready;
  await service.execute({ type: 'signOut' });
  release(Response.json({ id_token: 'late-id', refresh_token: 'late-refresh', user_id: 'u1', expires_in: '3600' }));
  await assert.rejects(pending);
  assert.equal(saved.value, null); assert.equal(service.snapshot().user, null);
});
test('credentials from another Firebase project are not restored', async () => {
  const saved = store(session().replace('demo-lingxi', 'another-project'));
  const service = new AccountService(config, saved, async () => {}, mockFetch(() => { throw Error('must not request'); }));
  assert.equal((await service.execute({ type: 'status' })).user, null);
});
test('OAuth credential validation rejects wrong provider and missing tokens', () => {
  assert.throws(() => parseBrowserCredential({ providerId: 'apple.com', idToken: 'secret' }, 'google.com'));
  assert.throws(() => parseBrowserCredential({ providerId: 'google.com' }, 'google.com'));
  assert.throws(() => parseBrowserCredential({ providerId: 'google.com', idToken: null, accessToken: null, pendingToken: null }, 'google.com'));
  assert.throws(() => parseBrowserCredential({ providerId: 'google.com', idToken: 'secret', pendingToken: 42 }, 'google.com'));
});
test('Google SDK credentials with null optional fields complete desktop sign-in', { timeout: 10_000 }, async t => {
  const credential = GoogleAuthProvider.credential('google-id', 'google-access').toJSON();
  assert.equal(credential.pendingToken, null);
  assert.deepEqual(parseBrowserCredential(credential, 'google.com'), {
    providerId: 'google.com', idToken: 'google-id', accessToken: 'google-access',
  });
  let opened!: (url: string) => void;
  const started = new Promise<string>(resolve => { opened = resolve; });
  const saved = store();
  const service = new AccountService(config, saved, async url => { opened(url); }, mockFetch((url, body) => {
    assert.ok(url.includes('signInWithIdp'));
    assert.equal(body.pendingToken, undefined);
    assert.deepEqual(Object.fromEntries(new URLSearchParams(body.postBody)), {
      providerId: 'google.com', id_token: 'google-id', access_token: 'google-access',
    });
    return Response.json(authResponse);
  }));
  t.after(() => service.dispose());
  const pending = service.execute({ type: 'oauth', provider: 'google.com' });
  const url = await started;
  const response = await fetch(`${url}/complete`, {
    method: 'POST', headers: { Origin: new URL(url).origin, 'Content-Type': 'application/json' },
    body: JSON.stringify(credential),
  });
  assert.equal(response.status, 200);
  const state = await pending;
  assert.equal(state.user?.uid, 'u1');
  assert.equal(state.busy, false);
  assert.equal(state.error, null);
  assert.equal(JSON.parse(saved.value!).refreshToken, 'refresh-secret');
});
test('browser callback rejects cross-origin requests and consumes only the expected credential', async () => {
  let opened!: (url: string) => void;
  const started = new Promise<string>(resolve => { opened = resolve; });
  const controller = new AbortController();
  const pending = browserSignIn(config, 'google.com', async url => { opened(url); }, controller.signal);
  const url = await started;
  try {
    const html = await (await fetch(url)).text();
    assert.ok(html.includes('signInWithPopup')); assert.ok(!html.includes('refresh-secret'));
    assert.equal((await fetch(`${url}/complete`, { method: 'POST', headers: { Origin: 'https://attacker.example', 'Content-Type': 'application/json' }, body: '{}' })).status, 403);
    assert.equal((await fetch(`${url}/complete`, { method: 'POST', headers: { Origin: new URL(url).origin, 'Content-Type': 'application/json' }, body: JSON.stringify({ providerId: 'apple.com', idToken: 'secret' }) })).status, 400);
    const result = await fetch(`${url}/complete`, { method: 'POST', headers: { Origin: new URL(url).origin, 'Content-Type': 'application/json' }, body: JSON.stringify({ providerId: 'google.com', idToken: 'secret' }) });
    assert.equal(result.status, 200);
    assert.deepEqual(await pending, { providerId: 'google.com', idToken: 'secret' });
  } finally { controller.abort(); }
});
test('cancelling browser login closes its callback server', async () => {
  let opened!: (url: string) => void;
  const started = new Promise<string>(resolve => { opened = resolve; });
  const controller = new AbortController();
  const pending = browserSignIn(config, 'apple.com', async url => { opened(url); }, controller.signal);
  const url = await started; controller.abort();
  await assert.rejects(pending, /取消/);
  await assert.rejects(fetch(url));
});

test('cached-token terminal failures clear the session; transient and unrelated login failures do not', async () => {
  for (const operation of ['reload', 'verifyEmail', 'signIn'] as const) {
    for (const code of ['USER_NOT_FOUND', 'USER_DISABLED', 'INVALID_ID_TOKEN', 'TOKEN_EXPIRED', 'NETWORK_ERROR']) {
      const saved = store();
      let failure = false;
      const service = new AccountService(config, saved, async () => {}, mockFetch(url => {
        assert.ok(!url.includes('securetoken'), 'cached ID token should be used');
        if (failure) return Response.json({ error: { message: code } }, { status: 400 });
        return Response.json(url.includes('lookup') ? { users: [{ localId: 'u1' }] } : authResponse);
      }));
      await service.execute({ type: 'signIn', email: 'a@example.com', password: 'secret' });
      failure = true;
      const state = await service.execute(operation === 'signIn'
        ? { type: operation, email: 'other@example.com', password: 'secret' } : { type: operation });
      const cleared = operation !== 'signIn' && code !== 'NETWORK_ERROR';
      assert.equal(state.user === null, cleared, `${operation}: ${code}`);
      if (cleared) assert.deepEqual(JSON.parse(saved.value!), { projectId: config.projectId, sessionExpired: true });
      else assert.ok(saved.value!.includes('refresh-secret'));
      assert.equal(state.busy, false);
      assert.ok(state.error);
      if (cleared) await assert.rejects(service.getIdToken(), /请先登录/);
    }
  }
});

test('Apple SDK pending credentials survive the callback and use the pending-token exchange', { timeout: 10_000 }, async t => {
  const sdkCredential = OAuthCredential.fromJSON({ providerId: 'apple.com', signInMethod: 'apple.com', idToken: 'apple-id', pendingToken: 'apple-pending' });
  assert.ok(sdkCredential);
  const credential = sdkCredential.toJSON();
  assert.equal(parseBrowserCredential(credential, 'apple.com').pendingToken, 'apple-pending');
  assert.deepEqual(parseBrowserCredential({ providerId: 'apple.com', pendingToken: 'pending-only' }, 'apple.com'), { providerId: 'apple.com', pendingToken: 'pending-only' });
  for (const pendingToken of [42, 'x'.repeat(32_769)]) {
    assert.throws(() => parseBrowserCredential({ providerId: 'apple.com', pendingToken }, 'apple.com'));
  }
  let opened!: (url: string) => void;
  const started = new Promise<string>(resolve => { opened = resolve; });
  let exchanges = 0;
  const service = new AccountService(config, store(), async url => { opened(url); }, mockFetch((url, body) => {
    assert.ok(url.includes('signInWithIdp'));
    assert.equal(body.pendingToken, 'apple-pending');
    assert.equal(body.postBody, undefined);
    exchanges++;
    return Response.json(authResponse);
  }));
  t.after(() => service.dispose());
  const pending = service.execute({ type: 'oauth', provider: 'apple.com' });
  const url = await started;
  const response = await fetch(`${url}/complete`, { method: 'POST', headers: { Origin: new URL(url).origin, 'Content-Type': 'application/json' }, body: JSON.stringify(credential) });
  assert.equal(response.status, 200);
  assert.equal((await pending).user?.uid, 'u1');
  assert.equal(exchanges, 1);
});

test('provider popup cancellation settles the service and closes the origin-checked callback', { timeout: 10_000 }, async t => {
  let opened!: (url: string) => void;
  const started = new Promise<string>(resolve => { opened = resolve; });
  const service = new AccountService(config, store(), async url => { opened(url); }, mockFetch(() => { throw Error('cancel must not exchange credentials'); }));
  t.after(() => service.dispose());
  const pending = service.execute({ type: 'oauth', provider: 'google.com' });
  const url = await started;
  assert.equal((await fetch(`${url}/cancel`, { method: 'POST', headers: { Origin: 'https://attacker.example', 'Content-Type': 'application/json' }, body: '{}' })).status, 403);
  assert.equal(service.snapshot().busy, true);
  const html = await (await fetch(url)).text();
  const script = html.match(/<script type="module"[^>]*>([\s\S]*?)<\/script>/)?.[1];
  assert.ok(script);
  const button = { disabled: false, onclick: async () => {} };
  const status = { textContent: '' };
  // Execute the actual served handler with only the provider and DOM mocked.
  vm.runInNewContext(script.replace(/^const \{.*\} = await import\('\/firebase-(?:app|auth)\.js'\);$/gm, ''), {
    initializeApp: () => ({}), initializeAuth: () => ({}), inMemoryPersistence: {}, browserPopupRedirectResolver: {},
    GoogleAuthProvider: class { addScope() {} },
    document: { getElementById: (id: string) => id === 'login' ? button : status },
    signInWithPopup: async () => { throw Object.assign(new Error('cancelled'), { code: 'auth/popup-closed-by-user' }); },
    signOut: async () => {},
    fetch: (path: string, init: RequestInit) => fetch(new URL(path, url), { ...init, headers: { ...init.headers, Origin: new URL(url).origin } }),
  });
  await button.onclick();
  const state = await pending;
  assert.equal(state.error, '登录已取消。');
  assert.equal(state.busy, false);
  assert.equal(state.user, null);
  assert.match(status.textContent, /登录已取消/);
  assert.equal(button.disabled, true);
  await assert.rejects(fetch(url));
});
