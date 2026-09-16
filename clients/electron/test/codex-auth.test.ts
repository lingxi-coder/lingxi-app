import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createServer } from 'node:http';
import test from 'node:test';
import { loginCodex, parseCodexSession, sessionFromTokenResponse } from '../src/main/codex-auth.ts';
import { resolveCodexOAuthSession, resolveProviderCredential, resolveSessionLaunchCredentials } from '../src/main/credential-broker.ts';

const jwt = (value: unknown) => `header.${Buffer.from(JSON.stringify(value)).toString('base64url')}.signature`;

test('Codex token response retains expiry, account and refresh without minting an API key', () => {
  assert.deepEqual(sessionFromTokenResponse({
    access_token: 'access-secret', refresh_token: 'refresh-secret', expires_in: 120,
    id_token: jwt({ 'https://api.openai.com/auth': { chatgpt_account_id: 'account-1', chatgpt_account_is_fedramp: true }, email: 'user@example.com' }),
  }, 1_000_000), {
    access_token: 'access-secret', refresh_token: 'refresh-secret', expires_at: 1120, account_id: 'account-1', fedramp: true, email: 'user@example.com',
  });
  // An id_token without an `email` claim stays valid; the field is optional.
  assert.deepEqual(sessionFromTokenResponse({
    access_token: 'access-secret', expires_in: 120,
    id_token: jwt({ 'https://api.openai.com/auth': { chatgpt_account_id: 'account-1' } }),
  }, 1_000_000), {
    access_token: 'access-secret', expires_at: 1120, account_id: 'account-1', fedramp: false,
  });
  assert.throws(() => sessionFromTokenResponse({ access_token: 'secret', expires_in: -1 }));
  assert.throws(() => parseCodexSession({ access_token: 'secret', expires_at: 'later', fedramp: false }));
});

test('a malformed display email is rejected rather than stored', () => {
  const base = { access_token: 'secret', expires_at: 1_000, fedramp: false };
  assert.equal(parseCodexSession({ ...base, email: 'user@example.com' }).email, 'user@example.com');
  assert.equal(parseCodexSession(base).email, undefined);
  for (const email of ['', 'a\nb@example.com', 'x'.repeat(321)]) {
    assert.throws(() => parseCodexSession({ ...base, email }), /Codex 登录凭据无效/);
  }
});

test('OAuth secrets never enter the ordinary API-key credential path', async () => {
  const session = { access_token: 'access', refresh_token: 'refresh', expires_at: 1234, fedramp: false };
  let calls = 0;
  const broker = { resolve: async () => { calls++; return JSON.stringify(session); } };
  assert.equal(await resolveProviderCredential('openai-chatgpt', { credentialBroker: broker }), undefined);
  assert.deepEqual(await resolveSessionLaunchCredentials('openai-chatgpt/gpt-5.6-sol', { credentialBroker: broker }), {});
  assert.equal(calls, 0);
  assert.deepEqual(await resolveCodexOAuthSession(broker), session);
});

test('PKCE validates state, exchanges only the matching callback, and releases the listener', async () => {
  let authorize!: URL;
  let redirect!: URL;
  const result = await loginCodex({
    signal: new AbortController().signal, port: 0,
    openExternal: async (value) => {
      authorize = new URL(value);
      redirect = new URL(authorize.searchParams.get('redirect_uri')!);
      const local = new URL(redirect); local.hostname = '127.0.0.1';
      local.search = new URLSearchParams({ state: 'wrong', code: 'attack' }).toString();
      assert.equal((await fetch(local)).status, 400);
      local.search = new URLSearchParams({ state: authorize.searchParams.get('state')!, code: 'valid-code' }).toString();
      assert.equal((await fetch(local)).status, 200);
    },
    fetch: async (url, init) => {
      assert.equal(url, 'https://auth.openai.com/oauth/token');
      const body = init!.body as URLSearchParams;
      assert.equal(body.get('code'), 'valid-code');
      assert.equal(body.get('redirect_uri'), redirect.toString());
      assert.equal(createHash('sha256').update(body.get('code_verifier')!).digest('base64url'), authorize.searchParams.get('code_challenge'));
      return new Response(JSON.stringify({ access_token: 'access', refresh_token: 'refresh', expires_in: 3600 }), { status: 200 });
    },
  });
  assert.equal(result.access_token, 'access');
  const server = createServer();
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(Number(redirect.port), '127.0.0.1', resolve);
  });
  await new Promise<void>((resolve) => server.close(() => resolve()));
});

test('cancel aborts a pending login without exchanging credentials', async () => {
  const abort = new AbortController();
  await assert.rejects(loginCodex({ signal: abort.signal, port: 0,
    openExternal: async () => { abort.abort(); },
    fetch: async () => { throw new Error('must not exchange'); },
  }), /取消/);
});

test('token endpoint failure does not expose response secrets', async () => {
  await assert.rejects(loginCodex({ signal: new AbortController().signal, port: 0,
    openExternal: async (value) => {
      const authorize = new URL(value);
      const callback = new URL(authorize.searchParams.get('redirect_uri')!);
      callback.hostname = '127.0.0.1';
      callback.search = new URLSearchParams({ state: authorize.searchParams.get('state')!, code: 'private-code' }).toString();
      await fetch(callback);
    },
    fetch: async () => new Response('private-refresh-token', { status: 401 }),
  }), (error: Error) => error.message.includes('401') && !error.message.includes('private'));
});
