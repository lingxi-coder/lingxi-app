import { buildCredentialEnvelope } from '../src/main/host-utils.ts';
import { parseProviderAccountRouting } from '../src/shared/providerAccounts.ts';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { DesktopProviderAccounts, providerAccountRenewFailureCode, providerAccountSessionStore, runtimeAccountCredential, withFileRefreshIntents, withProviderAccountLock } from '../src/main/provider-accounts.ts';
import { CodexTokenManager } from '../src/main/codex-tokens.ts';
import type { ProviderCredentialBroker } from '../src/main/credential-broker.ts';
import type { ProviderAccountCommand, ProviderAccountSnapshot } from '../src/shared/providerAccounts.ts';

const NOW = 1_000_000_000_000;
const A = 'acct.11111111-1111-4111-8111-111111111111';
const B = 'acct.22222222-2222-4222-8222-222222222222';
function broker() {
  const values = new Map<string, string>();
  const writes: string[] = [];
  let failWrites = false;
  return { values, writes, get failWrites() { return failWrites; }, set failWrites(value) { failWrites = value; },
    health: async () => ({ protocolVersion: 1, buildVersion: 'test' }),
    listStatus: async (ids: readonly string[]) => ids.map(providerId => ({ providerId, configured: values.has(providerId) })),
    resolve: async (id: string) => values.get(id),
    preview: async (providerId: string) => ({ providerId, configured: values.has(providerId) }),
    set: async (id: string, value: string) => { if (failWrites && !id.endsWith('.intent')) throw new Error('mock storage failure'); writes.push(id); values.set(id, value); return { providerId: id, configured: true }; },
    delete: async (id: string) => { writes.push(`delete:${id}`); values.delete(id); },
    listPluginSecrets: async () => [], previewPluginSecret: async () => ({ pluginId: '', key: '', configured: false }),
    resolvePluginSecret: async () => undefined, setPluginSecret: async () => ({ pluginId: '', key: '', configured: false }), deletePluginSecret: async () => {},
  } satisfies ProviderCredentialBroker & { values: Map<string, string>; writes: string[]; failWrites: boolean };
}
const fresh = { access_token: 'access-a', refresh_token: 'refresh-a', expires_at: NOW / 1000 + 3600, account_id: 'upstream-a', subject: 'user-a', fedramp: false };
function fixture(lockRoot?: string, fetch?: typeof globalThis.fetch) {
  const storage = broker();
  const accountStorage = storage;
  let publishHook: (versions: readonly import('../src/main/provider-accounts').ProviderAccountCredentialVersion[]) => Promise<void> = async () => {};
  const tokens = new CodexTokenManager({ store: providerAccountSessionStore(accountStorage, undefined, version => publishHook([version])), now: () => NOW, ...(fetch ? { fetch } : {}) });
  let data: ProviderAccountSnapshot = { storeScope: 'mock-broker', configRevision: 0, accounts: [], health: [] };
  let session = fresh;
  let loginHook: (signal: AbortSignal) => Promise<typeof fresh> = async () => session;
  let changedHook: (snapshot: ProviderAccountSnapshot) => Promise<void> = async () => {};
  const registry = async (request: ProviderAccountCommand) => {
    if (request.action === 'list') return structuredClone(data);
    assert.equal(request.expectedRevision, data.configRevision);
    const revision = ++data.configRevision;
    if (request.action === 'add') data.accounts.push({ accountId: request.accountId!, providerId: request.providerId!, displayName: request.displayName!, authKind: request.authKind!,
      executionBindingId: request.executionBindingId!, credentialRef: { storeScope: data.storeScope, secretRef: `acct.${request.accountId}` }, enabled: true, userPriority: data.accounts.length, configRevision: revision, identityGeneration: 1 });
    else if (request.action === 'set_credential' || request.action === 'logout') { const account = data.accounts.find(a => a.accountId === request.accountId)!; account.identityGeneration++; account.configRevision = revision; }
    else if (request.action === 'remove') data.accounts = data.accounts.filter(a => a.accountId !== request.accountId);
    else if (request.action === 'reorder') for (const account of data.accounts) account.userPriority = request.accountIds!.indexOf(account.accountId);
    else if (request.action === 'enable') data.accounts.find(a => a.accountId === request.accountId)!.enabled = request.enabled!;
    else if (request.action === 'update' && request.displayName) data.accounts.find(a => a.accountId === request.accountId)!.displayName = request.displayName;
    return structuredClone(data);
  };
  const service = new DesktopProviderAccounts({ withAccountMutationLock: lockRoot ? (id, operation) => withProviderAccountLock(lockRoot, id, operation) : undefined, registry, broker: accountStorage, tokens, login: signal => loginHook(signal), changed: snapshot => changedHook(snapshot), publishVersions: versions => publishHook(versions), probe: async () => {} });
  return { service, storage, tokens, registry, set publish(value: typeof publishHook) { publishHook = value; }, set login(value: typeof loginHook) { loginHook = value; }, set changed(value: typeof changedHook) { changedHook = value; }, set session(value: typeof fresh) { session = value; }, mutateRevision() { data.configRevision++; } };
}

test('two API accounts have independent Broker slots; ordering and deletion preserve the other account', async () => {
  const { service, storage } = fixture();
  let snapshot = await service.command({ action: 'add', expectedRevision: 0, providerId: 'openai', executionBindingId: 'openai', authKind: 'api_key', displayName: 'A' }, 'key-a');
  snapshot = await service.command({ action: 'add', expectedRevision: snapshot.configRevision, providerId: 'openai', executionBindingId: 'openai', authKind: 'api_key', displayName: 'B' }, 'key-b');
  const [a, b] = snapshot.accounts;
  assert.notEqual(a.credentialRef.secretRef, b.credentialRef.secretRef);
  snapshot = await service.command({ action: 'reorder', expectedRevision: snapshot.configRevision, providerId: 'openai', accountIds: [b.accountId, a.accountId] });
  assert.equal(snapshot.accounts.find(row => row.accountId === b.accountId)?.userPriority, 0);
  snapshot = await service.command({ action: 'remove', expectedRevision: snapshot.configRevision, accountId: a.accountId });
  assert.equal(storage.values.has(a.credentialRef.secretRef), false);
  assert.equal(JSON.parse(storage.values.get(b.credentialRef.secretRef)!).kind.key, 'key-b');
  assert.equal(snapshot.accounts.length, 1);
});

test('status reads do not write Broker credentials and snapshots contain no secrets', async () => {
  const { service, storage } = fixture();
  await service.command({ action: 'add', expectedRevision: 0, providerId: 'openai', executionBindingId: 'openai', authKind: 'api_key' }, 'private-key-a');
  storage.writes.length = 0;
  const snapshot = await service.snapshot();
  assert.equal(storage.writes.length, 0);
  assert.equal(JSON.stringify(snapshot).includes('private-key-a'), false);
});

test('two OAuth identities coexist; duplicate login cannot overwrite an existing grant', async () => {
  const f = fixture();
  let snapshot = await f.service.command({ action: 'add', expectedRevision: 0, providerId: 'openai', executionBindingId: 'openai-chatgpt', authKind: 'oauth', displayName: 'A' });
  f.session = { ...fresh, access_token: 'access-b', refresh_token: 'refresh-b', account_id: 'upstream-b', subject: 'user-b' };
  snapshot = await f.service.command({ action: 'add', expectedRevision: snapshot.configRevision, providerId: 'openai', executionBindingId: 'openai-chatgpt', authKind: 'oauth', displayName: 'B' });
  const saved = new Map(f.storage.values);
  await assert.rejects(f.service.command({ action: 'add', expectedRevision: snapshot.configRevision, providerId: 'openai', executionBindingId: 'openai-chatgpt', authKind: 'oauth' }), /already added/);
  assert.deepEqual(f.storage.values, saved);
  f.tokens.dispose();
});

test('reconnecting as another OAuth subject refuses to replace the original account', async () => {
  const f = fixture();
  const snapshot = await f.service.command({ action: 'add', expectedRevision: 0, providerId: 'openai', executionBindingId: 'openai-chatgpt', authKind: 'oauth' });
  f.session = { ...fresh, account_id: 'different', subject: 'different' };
  const saved = new Map(f.storage.values);
  await assert.rejects(f.service.command({ action: 'reconnect', expectedRevision: snapshot.configRevision, accountId: snapshot.accounts[0].accountId }), /different account/);
  assert.deepEqual(f.storage.values, saved);
  f.tokens.dispose();
});

test('named accounts refresh independently and never send the refresh token to the runtime', async () => {
  const storage = broker();
  const writes = providerAccountSessionStore(storage);
  await writes.set(A, JSON.stringify({ ...fresh, expires_at: NOW / 1000 + 1 }));
  await writes.set(B, JSON.stringify({ ...fresh, account_id: 'b', expires_at: NOW / 1000 + 1, refresh_token: 'refresh-b' }));
  const exchanged: string[] = [];
  const tokens = new CodexTokenManager({ store: writes, now: () => NOW, fetch: (async (_url, init) => {
    const request = JSON.parse(String(init?.body)); exchanged.push(request.refresh_token);
    return new Response(JSON.stringify({ access_token: `new-${request.refresh_token}`, refresh_token: `rotated-${request.refresh_token}`, expires_in: 3600 }));
  }) as typeof fetch });
  await Promise.all([tokens.current(A), tokens.current(A), tokens.current(B), tokens.current(B)]);
  assert.deepEqual(exchanged.sort(), ['refresh-a', 'refresh-b']);
  const wire = runtimeAccountCredential(JSON.parse(storage.values.get(A)!));
  assert.equal(wire.kind.kind, 'oauth');
  assert.equal(JSON.stringify(wire).includes('rotated-'), false);
  assert.equal(wire.kind.kind === 'oauth' && wire.kind.access_token, 'new-refresh-a');
  if (wire.kind.kind === 'oauth') assert.equal(wire.kind.refresh_token, null);
});

test('failed rotation persistence publishes nothing and retries saving the new grant without exchanging again', async () => {
  const storage = broker(); const writes = providerAccountSessionStore(storage);
  await writes.set(A, JSON.stringify({ ...fresh, expires_at: NOW / 1000 + 1 }));
  let exchanges = 0; const published: string[] = [];
  const tokens = new CodexTokenManager({ store: writes, now: () => NOW, fetch: (async () => { exchanges++; storage.failWrites = true;
    return new Response(JSON.stringify({ access_token: 'new-access', refresh_token: 'rotated-secret', expires_in: 3600 })); }) as typeof fetch });
  tokens.subscribe(session => { if (session) published.push(session.access_token); });
  await assert.rejects(tokens.current(A), /安全存储/);
  assert.deepEqual(published, []);
  assert.equal(storage.values.has(`${A}.intent`), true);
  storage.failWrites = false;
  assert.equal((await tokens.current(A))?.access_token, 'new-access');
  assert.equal(exchanges, 1);
  assert.equal(storage.values.has(`${A}.intent`), false);
  assert.equal(JSON.parse(storage.values.get(A)!).kind.refresh_token, 'rotated-secret');
});

test('an abandoned refresh intent prevents a second authority from reusing the old refresh token', async () => {
  const storage = broker(); const writes = providerAccountSessionStore(storage);
  await writes.set(A, JSON.stringify({ ...fresh, expires_at: NOW / 1000 + 1 }));
  await storage.set(`${A}.intent`, JSON.stringify({ owner: 'crashed-process' }));
  let exchanges = 0;
  const tokens = new CodexTokenManager({ store: writes, now: () => NOW, fetch: (async () => { exchanges++; throw new Error(); }) as typeof fetch });
  await assert.rejects(tokens.current(A), /未确认/);
  await assert.rejects(tokens.renew(A), /未确认/);
  assert.equal(exchanges, 0);
});


test('API key replacement increments identity generation and credential revision without changing account identity', async () => {
  const f = fixture();
  const first = await f.service.command({ action: 'add', expectedRevision: 0, providerId: 'openai', executionBindingId: 'openai', authKind: 'api_key' }, 'original');
  const a = first.accounts[0];
  const replaced = await f.service.command({ action: 'reconnect', expectedRevision: first.configRevision, accountId: a.accountId }, 'replacement');
  assert.equal(replaced.accounts[0].accountId, a.accountId);
  assert.equal(replaced.accounts[0].identityGeneration, 2);
  const record = JSON.parse(f.storage.values.get(a.credentialRef.secretRef)!);
  assert.equal(record.credential_revision, 2);
  assert.equal(record.identity_generation, 2);
  assert.equal(record.kind.key, 'replacement');
  assert.equal(f.storage.values.has(`${a.credentialRef.secretRef}.mutation`), false);
});

test('logout targets only its account and leaves the account visible for reconnection', async () => {
  const f = fixture();
  let snapshot = await f.service.command({ action: 'add', expectedRevision: 0, providerId: 'openai', executionBindingId: 'openai', authKind: 'api_key' }, 'first');
  snapshot = await f.service.command({ action: 'add', expectedRevision: snapshot.configRevision, providerId: 'openai', executionBindingId: 'openai', authKind: 'api_key' }, 'second');
  const [a,b] = snapshot.accounts;
  await assert.rejects(f.service.command({ action: 'logout', expectedRevision: snapshot.configRevision - 1, accountId: a.accountId }), /changed/);
  assert.equal(f.storage.values.has(a.credentialRef.secretRef), true, 'a stale revision cannot clear the credential');
  snapshot = await f.service.command({ action: 'logout', expectedRevision: snapshot.configRevision, accountId: a.accountId });
  assert.equal(snapshot.accounts.length, 2);
  assert.equal(f.storage.values.has(a.credentialRef.secretRef), false);
  assert.equal(f.storage.values.has(b.credentialRef.secretRef), true);
  assert.equal(snapshot.health.find(h => h.accountId === a.accountId)?.status, 'needs_auth');
});

test('an interrupted credential mutation is visible and is excluded from runtime injection', async () => {
  const f = fixture();
  const snapshot = await f.service.command({ action: 'add', expectedRevision: 0, providerId: 'openai', executionBindingId: 'openai', authKind: 'api_key' }, 'private');
  const account = snapshot.accounts[0];
  await f.storage.set(`${account.credentialRef.secretRef}.mutation`, JSON.stringify({ operation: 'crashed' }));
  const launch = await f.service.launch();
  assert.equal(launch.providerAccounts.health.find(h => h.accountId === account.accountId)?.reason, 'credential_update_pending');
  assert.deepEqual(Object.keys(launch.providerAccountCredentials), []);
});

test('atomic file intents allow exactly one of two independent refresh authorities to exchange', async () => {
  const root = await mkdtemp(join(tmpdir(), 'provider-refresh-'));
  try {
    const storage = broker();
    const sessions = providerAccountSessionStore(storage);
    await sessions.set(A, JSON.stringify({ ...fresh, expires_at: NOW / 1000 + 1 }));
    const store = withFileRefreshIntents(sessions, async () => root);
    let exchanges = 0;
    const create = () => new CodexTokenManager({ store, now: () => NOW,
      withAccountLock: (id, op) => withProviderAccountLock(root, id, op),
      fetch: (async () => { exchanges++; await new Promise(resolve => setTimeout(resolve, 10));
        return new Response(JSON.stringify({ access_token: 'new', refresh_token: 'new-refresh', expires_in: 3600 })); }) as typeof fetch });
    const one = create(), two = create();
    await Promise.all([one.current(A), two.current(A)]);
    assert.equal(exchanges, 1);
    await store.set(`${A}.intent`, JSON.stringify({ owner: 'first' }));
    await assert.rejects(store.set(`${A}.intent`, JSON.stringify({ owner: 'second' })), { code: 'EEXIST' });
    const afterCrash = create();
    await assert.rejects(afterCrash.renew(A), /未确认/);
    assert.equal(exchanges, 1);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('a cached OAuth authority adopts another owner’s newer persisted credential before serving it', async () => {
  const storage = broker(); const store = providerAccountSessionStore(storage);
  await store.set(A, JSON.stringify(fresh));
  const tokens = new CodexTokenManager({ store, now: () => NOW });
  assert.equal((await tokens.current(A))?.access_token, fresh.access_token);
  await store.set(A, JSON.stringify({ ...fresh, access_token: 'new-owner-access', refresh_token: 'new-owner-refresh' }));
  assert.equal((await tokens.current(A))?.access_token, 'new-owner-access');
});

test('a pending rotation cannot overwrite a newer authorization after its intent has been cleared', async () => {
  const storage = broker(); const store = providerAccountSessionStore(storage);
  await store.set(A, JSON.stringify({ ...fresh, expires_at: NOW / 1000 + 1 }));
  const tokens = new CodexTokenManager({ store, now: () => NOW, fetch: (async () => { storage.failWrites = true;
    return new Response(JSON.stringify({ access_token: 'pending', refresh_token: 'pending-refresh', expires_in: 3600 })); }) as typeof fetch });
  await assert.rejects(tokens.current(A));
  storage.failWrites = false;
  await store.set(A, JSON.stringify({ ...fresh, access_token: 'new-sign-in', refresh_token: 'new-sign-in-refresh' }));
  await store.delete(`${A}.intent`);
  await assert.rejects(tokens.current(A), /已更改/);
  assert.equal(JSON.parse(storage.values.get(A)!).kind.access_token, 'new-sign-in');
});


test('two independent Desktop lifecycle services cannot run the rejected concurrent API key replacement', async () => {
  const root = await mkdtemp(join(tmpdir(), 'provider-mutation-'));
  try {
    const f = fixture(root);
    const snapshot = await f.service.command({ action: 'add', expectedRevision: 0, providerId: 'openai', executionBindingId: 'openai', authKind: 'api_key' }, 'initial');
    const account = snapshot.accounts[0];
    const other = new DesktopProviderAccounts({ registry: f.registry, broker: f.storage,
      tokens: new CodexTokenManager({ store: providerAccountSessionStore(f.storage), now: () => NOW }),
      withAccountMutationLock: (id, operation) => withProviderAccountLock(root, id, operation),
      login: async () => fresh, changed: async () => {}, publishVersions: async () => {}, probe: async () => {} });
    const request = { action: 'reconnect' as const, expectedRevision: snapshot.configRevision, accountId: account.accountId };
    const results = await Promise.allSettled([f.service.command(request, 'key-a'), other.command(request, 'key-b')]);
    assert.equal(results.filter(result => result.status === 'fulfilled').length, 1);
    assert.equal(results.filter(result => result.status === 'rejected').length, 1);
    const accepted = results[0].status === 'fulfilled' ? 'key-a' : 'key-b';
    const record = JSON.parse(f.storage.values.get(account.credentialRef.secretRef)!);
    assert.equal(record.kind.key, accepted);
    assert.equal(record.identity_generation, 2);
    assert.equal(record.credential_revision, 2);
    assert.equal(f.storage.values.has(`${account.credentialRef.secretRef}.mutation`), false);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('same-subject OAuth reconnect advances identity generation while normal rotation retains it', async () => {
  const f = fixture();
  const first = await f.service.command({ action: 'add', expectedRevision: 0, providerId: 'openai', executionBindingId: 'openai-chatgpt', authKind: 'oauth' });
  const account = first.accounts[0];
  f.session = { ...fresh, access_token: 'new-authorization', refresh_token: 'new-grant' };
  const next = await f.service.command({ action: 'reconnect', expectedRevision: first.configRevision, accountId: account.accountId });
  assert.equal(next.accounts[0].identityGeneration, 2);
  const record = JSON.parse(f.storage.values.get(account.credentialRef.secretRef)!);
  assert.equal(record.identity_generation, 2);
  assert.equal(record.credential_revision, 2);
  f.tokens.dispose();
});


test('OAuth duplicate identity is the subject plus workspace, allowing different users in one workspace', async () => {
  const f = fixture();
  let snapshot = await f.service.command({ action: 'add', expectedRevision: 0, providerId: 'openai', executionBindingId: 'openai-chatgpt', authKind: 'oauth' });
  f.session = { ...fresh, subject: 'second-user' };
  snapshot = await f.service.command({ action: 'add', expectedRevision: snapshot.configRevision, providerId: 'openai', executionBindingId: 'openai-chatgpt', authKind: 'oauth' });
  assert.equal(snapshot.accounts.length, 2);
  f.session = { ...fresh, account_id: 'another-workspace' };
  snapshot = await f.service.command({ action: 'add', expectedRevision: snapshot.configRevision, providerId: 'openai', executionBindingId: 'openai-chatgpt', authKind: 'oauth' });
  assert.equal(snapshot.accounts.length, 3);
  f.tokens.dispose();
});

test('routing events display only a known current account identity and drop unrecognized fields', async () => {
  const f = fixture();
  const snapshot = await f.service.command({ action: 'add', expectedRevision: 0, providerId: 'openai', executionBindingId: 'openai', authKind: 'api_key' }, 'key');
  const account = snapshot.accounts[0];
  const event = parseProviderAccountRouting({ logicalCallId: 'call-1', model: 'gpt', attempt: 1, status: 'selected', selectedAccountId: account.accountId,
    identityGeneration: account.identityGeneration, configRevision: account.configRevision, access_token: 'must-not-forward' });
  assert.equal(JSON.stringify(event).includes('must-not-forward'), false);
  assert.equal(f.service.recordRouting(event), true);
  assert.equal(f.service.recordRouting({ ...event, selectedAccountId: 'unknown' }), false);
  await f.service.command({ action: 'reconnect', expectedRevision: snapshot.configRevision, accountId: account.accountId }, 'replacement');
  assert.equal(f.service.recordRouting(event), false);
});


test('ordinary OAuth token rotation preserves the authoritative stored generation ahead of stale metadata', async () => {
  const storage = broker();
  const store = providerAccountSessionStore(storage, () => 1);
  await store.set(A, JSON.stringify(fresh), 2);
  await store.set(A, JSON.stringify({ ...fresh, access_token: 'rotated', refresh_token: 'rotated-refresh' }));
  const record = JSON.parse(storage.values.get(A)!);
  assert.equal(record.identity_generation, 2);
  assert.equal(record.credential_revision, 2);
});


test('OAuth cancellation is correlated to its operation and a stale cancel cannot abort the next flow', async () => {
  const f = fixture();
  let began!: () => void;
  let seenSignal: AbortSignal | undefined;
  const started = new Promise<void>(resolve => { began = resolve; });
  f.login = signal => { seenSignal = signal; began(); return new Promise((_resolve, reject) => signal.addEventListener('abort', () => reject(new Error('Sign-in cancelled.')), { once: true })); };
  const first = f.service.command({ action: 'add', expectedRevision: 0, providerId: 'openai', executionBindingId: 'openai-chatgpt', authKind: 'oauth', operationId: 11 });
  await started;
  f.service.cancel(99);
  assert.equal(seenSignal?.aborted, false);
  f.service.cancel(11);
  await assert.rejects(first, /cancelled/);
  assert.equal(f.storage.values.size, 0);
  f.login = async signal => { f.service.cancel(11); assert.equal(signal.aborted, false); return fresh; };
  const next = await f.service.command({ action: 'add', expectedRevision: 0, providerId: 'openai', executionBindingId: 'openai-chatgpt', authKind: 'oauth', operationId: 22 });
  assert.equal(next.accounts.length, 1);
  f.tokens.dispose();
});

test('runtime delivery failure cannot delete an account and secret after metadata commit', async () => {
  const f = fixture();
  f.changed = async () => { throw new Error('runtime disconnected'); };
  await assert.rejects(f.service.command({ action: 'add', expectedRevision: 0, providerId: 'openai', executionBindingId: 'openai', authKind: 'api_key' }, 'committed-key'), /disconnected/);
  const snapshot = await f.service.snapshot();
  assert.equal(snapshot.accounts.length, 1);
  assert.equal(JSON.parse(f.storage.values.get(snapshot.accounts[0].credentialRef.secretRef)!).kind.key, 'committed-key');
  assert.equal(f.storage.values.has(`${snapshot.accounts[0].credentialRef.secretRef}.mutation`), false);
});


test('runtime handshake reloads a rotated API identity from the canonical registry without OAuth rotation', async () => {
  const f = fixture();
  await f.service.command({ action: 'add', expectedRevision: 0, providerId: 'openai', executionBindingId: 'openai', authKind: 'api_key' }, 'old-key');
  const stale = await f.service.snapshot();
  const account = stale.accounts[0];
  const current = await f.service.command({ action: 'reconnect', expectedRevision: stale.configRevision, accountId: account.accountId }, 'new-key');
  f.service.latestSnapshot = stale;
  const writes = f.storage.writes.length;
  await f.service.renewForRuntime(account.accountId, current.accounts[0].identityGeneration, 0);
  assert.equal(f.storage.writes.length, writes);
  assert.equal(f.service.latestSnapshot?.accounts[0].identityGeneration, 2);
  assert.equal(JSON.parse(f.storage.values.get(account.credentialRef.secretRef)!).kind.key, 'new-key');
});


test('runtime refresh diagnostics classify API key rejection without exposing arbitrary errors', async () => {
  const f = fixture();
  const snapshot = await f.service.command({ action: 'add', expectedRevision: 0, providerId: 'openai', executionBindingId: 'openai', authKind: 'api_key' }, 'synthetic-private-key');
  const account = snapshot.accounts[0];
  await assert.rejects(f.service.renewForRuntime(account.accountId, account.identityGeneration, 1), error => {
    assert.equal(providerAccountRenewFailureCode(error), 'api_key_rejected');
    return true;
  });
  assert.equal(providerAccountRenewFailureCode(new Error('synthetic-private-key')), 'authority_unavailable');
  f.tokens.dispose();
});

test('cold launch and renewal report the failed credential stage without leaking errors', async () => {
  const f = fixture();
  const snapshot = await f.service.command({ action: 'add', expectedRevision: 0, providerId: 'openai', executionBindingId: 'openai', authKind: 'api_key' }, 'synthetic-private-key');
  const account = snapshot.accounts[0];
  const events: any[] = [];
  (f.service as any).options.onCredentialDiagnostic = (event: unknown) => events.push(event);
  const resolve = f.storage.resolve;
  f.storage.resolve = async id => {
    if (id === account.credentialRef.secretRef) throw new Error('synthetic-private-key');
    return resolve(id);
  };
  const launch = await f.service.launch();
  assert.equal(Object.keys(launch.providerAccountCredentials).length, 0);
  assert.equal(events.at(-1)?.stage, 'broker_read');
  assert.equal(events.at(-1)?.outcome, 'failed');
  assert.equal(events.at(-1)?.healthReason, 'credential_unavailable');
  await assert.rejects(f.service.renewForRuntime(account.accountId, account.identityGeneration, 0), error => {
    assert.equal(providerAccountRenewFailureCode(error), 'broker_read_failed');
    assert.doesNotMatch(String(error), /synthetic-private-key/);
    return true;
  });
  assert.doesNotMatch(JSON.stringify(events), /synthetic-private-key/);
  f.storage.resolve = resolve;
  f.storage.values.set(account.credentialRef.secretRef, 'synthetic-private-key');
  await assert.rejects(f.service.renewForRuntime(account.accountId, account.identityGeneration, 0), error => providerAccountRenewFailureCode(error) === 'decode_failed');
  f.tokens.dispose();
});

test('malformed secure-store payloads cannot echo credential bytes through an account error', async () => {
  const f = fixture();
  const snapshot = await f.service.command({ action: 'add', expectedRevision: 0, providerId: 'openai', executionBindingId: 'openai', authKind: 'api_key' }, 'initial');
  const account = snapshot.accounts[0];
  f.storage.values.set(account.credentialRef.secretRef, 'private-broken-secret');
  for (const action of ['test', 'reconnect'] as const) await assert.rejects(f.service.command({ action, expectedRevision: snapshot.configRevision, accountId: account.accountId }, 'replacement'), error => !String(error).includes('private-broken-secret'));
});

test('model availability projection sends only exact Broker presence and the directory revision', async () => {
  const f = fixture();
  const added = await f.service.command({ action: 'add', expectedRevision: 0, providerId: 'openai', executionBindingId: 'openai', authKind: 'api_key' }, 'private-key');
  const account = added.accounts[0]!;
  const calls: Array<{ revision: number; presence?: string[] }> = [];
  const options = (f.service as any).options;
  const registry = options.registry;
  options.registry = async (request: ProviderAccountCommand, presence?: string[]) => {
    calls.push({ revision: request.expectedRevision, presence });
    const snapshot = await registry(request);
    return { ...snapshot, modelAvailability: [{ providerId: 'openai', executionBindingId: 'openai', modelId: 'gpt-per-account', eligibleAccountIds: presence ?? [], confirmedAccountIds: [], unverifiedAccountIds: presence ?? [], forbiddenAccountIds: [] }] };
  };
  f.storage.resolve = async reference => { if (reference.endsWith('.intent')) return f.storage.values.get(reference); throw new Error('Listing models must not read credential bytes'); };
  const snapshot = await f.service.snapshot();
  assert.deepEqual(calls, [{ revision: 0, presence: undefined }, { revision: added.configRevision, presence: [account.accountId] }]);
  assert.deepEqual(snapshot.modelAvailability?.[0].eligibleAccountIds, [account.accountId]);
  assert.doesNotMatch(JSON.stringify(calls), /private-key|access_token|refresh_token/);
  f.storage.values.set(`${account.credentialRef.secretRef}.mutation`, 'pending');
  const pending = await f.service.snapshot();
  assert.deepEqual(pending.modelAvailability?.[0].eligibleAccountIds, []);
  assert.equal(pending.health[0].status, 'storage_unavailable');
});

test('rotation publishes its durable version before the new grant and retries a failed ledger commit without rewriting or exchanging', async () => {
  const storage = broker(); let failVersion = true, exchanges = 0, ledgerRevision = 1;
  const store = providerAccountSessionStore(storage, undefined, async version => {
    assert.equal(JSON.parse(storage.values.get(A)!).credential_revision, version.credentialRevision);
    if (failVersion) throw new Error('version ledger unavailable');
    ledgerRevision = version.credentialRevision;
  });
  await store.set(A, JSON.stringify({ ...fresh, expires_at: NOW / 1000 + 1 }), 1);
  const tokens = new CodexTokenManager({ store, now: () => NOW, fetch: (async () => { exchanges++; return Response.json({ access_token: 'rotated-access', refresh_token: 'rotated-refresh', expires_in: 3600 }); }) as typeof fetch });
  const published: string[] = [];
  tokens.subscribe(session => { if (session) { assert.equal(ledgerRevision, 2); published.push(session.access_token); } });
  await assert.rejects(tokens.current(A), /安全存储/);
  assert.equal(await tokens.hasPendingCommit(A), true);
  assert.equal(JSON.parse(storage.values.get(A)!).credential_revision, 2);
  assert.equal(JSON.parse(storage.values.get(A)!).kind.refresh_token, 'rotated-refresh');
  const writes = storage.writes.filter(reference => reference === A).length;
  await assert.rejects(tokens.current(A), /安全存储/);
  assert.equal(storage.writes.filter(reference => reference === A).length, writes, 'retry reuses the durable new revision');
  assert.deepEqual(published, []); assert.equal(ledgerRevision, 1);
  failVersion = false;
  assert.equal((await tokens.current(A))?.access_token, 'rotated-access');
  assert.equal(exchanges, 1); assert.equal(ledgerRevision, 2);
  assert.equal(await tokens.hasPendingCommit(A), false);
  assert.deepEqual(published, ['rotated-access']);
});

test('a stale sidecar retains revision two while the shared version is already three before token publication', async () => {
  const storage = broker(); let ledgerRevision = 1, exchanges = 0;
  const store = providerAccountSessionStore(storage, undefined, async version => { ledgerRevision = version.credentialRevision; });
  await store.set(A, JSON.stringify(fresh), 1);
  await store.set(A, JSON.stringify({ ...fresh, access_token: 'access-two', refresh_token: 'refresh-two' }));
  const staleSidecar = runtimeAccountCredential(JSON.parse(storage.values.get(A)!));
  const rotating = new CodexTokenManager({ store, now: () => NOW, fetch: (async () => { exchanges++; return Response.json({ access_token: 'access-three', refresh_token: 'refresh-three', expires_in: 3600 }); }) as typeof fetch });
  rotating.subscribe(session => { if (session) { assert.equal(staleSidecar.credential_revision, 2); assert.equal(ledgerRevision, 3); } });
  await rotating.renew(A, 'access-two');
  const staleAuthority = new CodexTokenManager({ store, now: () => NOW, fetch: (async () => { throw new Error('spent refresh token must not be exchanged'); }) as typeof fetch });
  assert.equal((await staleAuthority.renew(A, staleSidecar.kind.kind === 'oauth' ? staleSidecar.kind.access_token : ''))?.access_token, 'access-three');
  assert.equal(ledgerRevision, 3); assert.equal(exchanges, 1);
});

for (const authKind of ['api_key', 'oauth'] as const) {
  test(`${authKind} explicit authorization commits metadata and version before delivery; ledger failure remains storage unavailable`, async () => {
    const f = fixture(); const options = (f.service as any).options; let fail = true, delivered = 0;
    options.publishVersions = async ([version]: any[]) => {
      const snapshot = await f.registry({ action: 'list', expectedRevision: 0 });
      assert.equal(snapshot.accounts[0].identityGeneration, version.identityGeneration);
      const stored = JSON.parse(f.storage.values.get(snapshot.accounts[0].credentialRef.secretRef)!);
      assert.equal(stored.credential_revision, version.credentialRevision);
      if (fail) throw new Error('ledger unavailable');
    };
    f.tokens.subscribe(session => { if (session) delivered++; });
    await assert.rejects(f.service.command({ action: 'add', expectedRevision: 0, providerId: 'openai', executionBindingId: authKind === 'oauth' ? 'openai-chatgpt' : 'openai', authKind }, authKind === 'api_key' ? 'new-key' : undefined), /ledger unavailable/);
    const snapshot = await f.service.snapshot(), account = snapshot.accounts[0];
    assert.ok(f.storage.values.has(account.credentialRef.secretRef));
    assert.ok(f.storage.values.has(`${account.credentialRef.secretRef}.mutation`));
    assert.equal(snapshot.health[0].status, 'storage_unavailable'); assert.equal(delivered, 0);
    assert.deepEqual(Object.keys((await f.service.launch()).providerAccountCredentials), []);
    fail = false;
    await f.service.command({ action: 'reconnect', expectedRevision: snapshot.configRevision, accountId: account.accountId }, authKind === 'api_key' ? 'replacement-key' : undefined);
    assert.equal((await f.service.snapshot()).health[0].status, 'ready');
    assert.equal(f.storage.values.has(`${account.credentialRef.secretRef}.mutation`), false);
    assert.equal(delivered, authKind === 'oauth' ? 1 : 0);
  });
}

test('an unpublished rotated Broker grant reports storage unavailable and cannot reach any child', async () => {
  let exchanges = 0;
  const f = fixture(undefined, (async () => { exchanges++; return Response.json({ access_token: 'pending-ledger-access', refresh_token: 'pending-ledger-refresh', expires_in: 3600 }); }) as typeof fetch);
  const added = await f.service.command({ action: 'add', expectedRevision: 0, providerId: 'openai', executionBindingId: 'openai-chatgpt', authKind: 'oauth' });
  const account = added.accounts[0], reference = account.credentialRef.secretRef;
  f.publish = async () => { throw new Error('ledger write failed'); };
  await assert.rejects(f.tokens.renew(reference, fresh.access_token), /安全存储/);
  const snapshot = await f.service.snapshot();
  assert.equal(snapshot.health[0].status, 'storage_unavailable');
  assert.equal(snapshot.health[0].reason, 'credential_update_pending');
  assert.deepEqual(Object.keys((await f.service.launch()).providerAccountCredentials), []);
  assert.equal(JSON.parse(f.storage.values.get(reference)!).kind.access_token, 'pending-ledger-access');
  f.publish = async () => {};
  await f.tokens.current(reference);
  assert.equal(exchanges, 1); assert.equal((await f.service.snapshot()).health[0].status, 'ready');
  assert.equal((await f.service.launch()).providerAccountCredentials[account.accountId].credential_revision, 2);
});
