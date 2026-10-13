import { test } from 'node:test';
import assert from 'node:assert/strict';
import { CODEX_SESSION_EXPIRED, CodexTokenManager, codexAccessToken } from '../src/main/codex-tokens.ts';

const NOW = 1_000_000_000_000;
const seconds = (ms: number) => Math.floor(ms / 1000);

function store(initial?: object) {
  const log: string[] = [];
  let value = initial ? JSON.stringify(initial) : undefined;
  return {
    log,
    get value() { return value ? JSON.parse(value) : undefined; },
    set value(next: any) { value = next ? JSON.stringify(next) : undefined; },
    resolve: async () => { log.push('read'); return value; },
    set: async (_id: string, secret: string) => { log.push(`write:${JSON.parse(secret).access_token}`); value = secret; return { configured: true }; },
    delete: async () => { log.push('delete'); value = undefined; },
  };
}

function tokenResponse(access: string, refresh?: string) {
  return new Response(JSON.stringify({ access_token: access, ...(refresh ? { refresh_token: refresh } : {}), expires_in: 3600 }), { status: 200 });
}

function manager(storage: ReturnType<typeof store>, fetch: (body: any) => Promise<Response>, now = () => NOW) {
  const errors: string[] = [];
  const published: Array<string | undefined> = [];
  const requests: any[] = [];
  const tokens = new CodexTokenManager({
    store: storage,
    now,
    onError: (message) => errors.push(message),
    fetch: (async (_url: string, init: RequestInit) => {
      const body = JSON.parse(String(init.body));
      requests.push(body);
      return fetch(body);
    }) as typeof fetch,
  });
  tokens.subscribe((session) => {
    published.push(session?.access_token);
    storage.log.push(`publish:${session?.access_token}`);
  });
  return { tokens, errors, published, requests };
}

const fresh = { access_token: 'a1', refresh_token: 'r1', expires_at: seconds(NOW) + 3600, fedramp: false, email: 'user@example.com', account_id: 'acct' };
const expiring = { ...fresh, expires_at: seconds(NOW) + 60 };

test('a fresh session is served without touching the network', async () => {
  const storage = store(fresh);
  const { tokens, requests } = manager(storage, async () => { throw new Error('no refresh'); });
  assert.equal((await tokens.current())?.access_token, 'a1');
  assert.equal((await tokens.current())?.access_token, 'a1');
  assert.deepEqual(requests, []);
  assert.deepEqual(storage.log, ['read'], 'the store is read once');
});

test('callers of an expiring session share one refresh, persisted before anyone sees it', async () => {
  const storage = store(expiring);
  const { tokens, requests } = manager(storage, async () => tokenResponse('a2', 'r2'));
  const results = await Promise.all([tokens.current(), tokens.current(), tokens.current()]);
  assert.deepEqual(results.map((session) => session?.access_token), ['a2', 'a2', 'a2']);
  assert.equal(requests.length, 1);
  assert.deepEqual(requests[0], { grant_type: 'refresh_token', refresh_token: 'r1', client_id: 'app_EMoamEEZ73f0CkXaXp7hrann' });
  assert.deepEqual(storage.log.filter((entry) => entry !== 'read'), ['write:a2', 'publish:a2']);
  assert.equal(storage.value.refresh_token, 'r2');
  // Identity the token response left out is kept.
  assert.equal(storage.value.email, 'user@example.com');
  assert.equal(storage.value.account_id, 'acct');
});

test('a token response without a refresh token keeps the old one', async () => {
  const storage = store(expiring);
  const { tokens } = manager(storage, async () => tokenResponse('a2'));
  await tokens.current();
  assert.equal(storage.value.refresh_token, 'r1');
});

test('a newer session in the store is adopted instead of refreshed', async () => {
  const storage = store(expiring);
  const { tokens, requests, published } = manager(storage, async () => { throw new Error('no refresh'); });
  await tokens.current().catch(() => undefined);
  storage.value = { ...fresh, access_token: 'from-store' };
  const renewed = await tokens.renew();
  assert.equal(renewed?.access_token, 'from-store');
  assert.equal(requests.length, 1, 'only the first, failed attempt reached the network');
  assert.equal(published.at(-1), 'from-store');
});

test('engines asking together for a renewal share one refresh', async () => {
  const storage = store(fresh);
  let refreshes = 0;
  const { tokens } = manager(storage, async () => tokenResponse(`a${++refreshes + 1}`, `r${refreshes + 1}`));
  const results = await Promise.all([tokens.renew(), tokens.renew(), tokens.renew()]);
  assert.equal(refreshes, 1);
  assert.deepEqual(results.map((session) => session?.access_token), ['a2', 'a2', 'a2']);
});

test('a rejected refresh token ends the session without retrying or leaking the response', async () => {
  const storage = store(fresh);
  const { tokens, errors, requests } = manager(storage, async () => new Response(
    JSON.stringify({ error: 'refresh_token_reused', error_description: 'r1 was already used' }), { status: 401 }));
  await assert.rejects(tokens.renew(), new RegExp(CODEX_SESSION_EXPIRED));
  await assert.rejects(tokens.renew(), new RegExp(CODEX_SESSION_EXPIRED));
  assert.equal(requests.length, 1, 'a failed refresh is not retried at once');
  assert.ok(!JSON.stringify(errors).includes('r1'));
  assert.equal(storage.value.access_token, 'a1', 'the stored session is left for the user to replace');
});

test('a transient failure keeps serving the current token and retries later', async () => {
  let now = NOW;
  const storage = store(expiring);
  let fail = true;
  const { tokens, requests } = manager(storage, async () => (fail ? new Response('{}', { status: 503 }) : tokenResponse('a2', 'r2')), () => now);
  assert.equal((await tokens.current())?.access_token, 'a1');
  assert.equal((await tokens.current())?.access_token, 'a1');
  assert.equal(requests.length, 1);
  fail = false;
  now += 61_000;
  assert.equal((await tokens.current())?.access_token, 'a2');
});

test('sign-out waits for a refresh in flight, and the refresh never restores the session', async () => {
  const storage = store(fresh);
  let release!: () => void;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  const { tokens, published } = manager(storage, async () => { await gate; return tokenResponse('a2', 'r2'); });
  const renewing = tokens.renew();
  const signingOut = tokens.signOut();
  release();
  await renewing;
  await signingOut;
  assert.equal(storage.value, undefined);
  assert.deepEqual(published, ['a2', undefined]);
  assert.equal(await tokens.current(), undefined);
});

test('an engine sign-in is adopted only with a refresh token', async () => {
  const storage = store(fresh);
  const { tokens, published } = manager(storage, async () => { throw new Error('no refresh'); });
  await tokens.adopt({ access_token: 'engine-access', expires_at: seconds(NOW) + 3600, fedramp: false });
  assert.deepEqual(published, []);
  await tokens.adopt({ access_token: 'engine-access', refresh_token: 'engine-refresh', expires_at: seconds(NOW) + 3600, fedramp: false });
  assert.deepEqual(published, ['engine-access']);
  assert.equal(storage.value.refresh_token, 'engine-refresh');
});

test('check refreshes ahead of expiry by the wall clock', async () => {
  let now = NOW;
  const storage = store(fresh);
  const { tokens, requests } = manager(storage, async () => tokenResponse('a2', 'r2'), () => now);
  await tokens.current();
  tokens.check();
  await tokens.current();
  assert.equal(requests.length, 0);
  now += (3600 - 4 * 60) * 1000;
  tokens.check();
  assert.equal((await tokens.current())?.access_token, 'a2');
  assert.equal(requests.length, 1);
});

test('an engine never receives the refresh token or the account email', () => {
  assert.deepEqual(codexAccessToken(fresh), { access_token: 'a1', expires_at: fresh.expires_at, account_id: 'acct', fedramp: false });
});
