import assert from 'node:assert/strict';
import test from 'node:test';
import { accountSessionDiagnosticScope, brokerAccountSessionStore } from '../src/main/account-broker-storage.js';

const scopeA = '11111111-2222-4333-8444-555555555555';
const scopeB = 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee';
const keyA = 'lingxi-account-diagnostic-11111111222243338444555555555555';
const keyB = 'lingxi-account-diagnostic-aaaaaaaabbbb4ccc8dddeeeeeeeeeeee';

function mapBroker() {
  const values = new Map([
    ['lingxi-account-session', 'personal session sentinel'],
    [keyA, 'diagnostic A sentinel'],
    [keyB, 'diagnostic B sentinel'],
  ]);
  const calls: Array<{ op: string; id: string }> = [];
  const broker = {
    credentialChannel: 'development' as const,
    async resolve(id: string) { calls.push({ op: 'read', id }); return values.get(id); },
    async set(id: string, value: string) { calls.push({ op: 'write', id }); values.set(id, value); return { providerId: id, configured: true }; },
    async delete(id: string) { calls.push({ op: 'remove', id }); values.delete(id); },
  };
  return { broker, values, calls };
}

test('diagnostic account operations and restarts stay in their exact namespace', async () => {
  const fixture = mapBroker();
  const first = brokerAccountSessionStore(fixture.broker, scopeA);
  assert.equal(fixture.calls.length, 0);
  assert.equal(await first.read(), 'diagnostic A sentinel');
  await first.write('diagnostic A rotated');
  const restarted = brokerAccountSessionStore(fixture.broker, scopeA.replaceAll('-', '').toUpperCase());
  assert.equal(await restarted.read(), 'diagnostic A rotated');
  await restarted.remove();
  assert.equal(await first.read(), null);
  assert.deepEqual(fixture.calls.map(call => call.op), ['read', 'write', 'read', 'remove', 'read']);
  assert.ok(fixture.calls.every(call => call.id === keyA));
  assert.match(keyA, /^[a-z0-9][a-z0-9._-]{0,63}$/);
  assert.equal(fixture.values.get('lingxi-account-session'), 'personal session sentinel');
  assert.equal(fixture.values.get(keyB), 'diagnostic B sentinel');
  assert.equal(fixture.values.has(keyA), false);
});

test('a fresh diagnostic scope reads an absent item without adopting or writing another session', async () => {
  const fixture = mapBroker();
  const scope = '0123456789abcdef0123456789abcdef';
  const before = new Map(fixture.values);
  assert.equal(await brokerAccountSessionStore(fixture.broker, scope).read(), null);
  assert.deepEqual(fixture.values, before);
  assert.deepEqual(fixture.calls, [{ op: 'read', id: `lingxi-account-diagnostic-${scope}` }]);
});

test('normal startup retains only the original account key', async () => {
  const fixture = mapBroker();
  const store = brokerAccountSessionStore(fixture.broker);
  assert.equal(await store.read(), 'personal session sentinel');
  await store.write('personal session updated');
  await store.remove();
  assert.ok(fixture.calls.every(call => call.id === 'lingxi-account-session'));
  assert.equal(fixture.values.get(keyA), 'diagnostic A sentinel');
  assert.equal(fixture.values.get(keyB), 'diagnostic B sentinel');
});

test('diagnostic account broker failures propagate without trying the default or another scope', async () => {
  const fixture = mapBroker();
  const before = new Map(fixture.values);
  const failure = new Error('broker unavailable');
  const broker = {
    ...fixture.broker,
    async resolve(id: string) { fixture.calls.push({ op: 'read', id }); throw failure; },
    async set(id: string, _value: string) { fixture.calls.push({ op: 'write', id }); throw failure; },
    async delete(id: string) { fixture.calls.push({ op: 'remove', id }); throw failure; },
  };
  const store = brokerAccountSessionStore(broker, scopeA);
  await assert.rejects(store.read(), error => error === failure);
  await assert.rejects(store.write('diagnostic A update'), error => error === failure);
  await assert.rejects(store.remove(), error => error === failure);
  assert.ok(fixture.calls.every(call => call.id === keyA));
  assert.deepEqual(fixture.values, before);
});

test('only an explicit development host diagnostic launch can select an account scope', () => {
  const flags = ['--lingxi-host-diagnostics', `--lingxi-account-session-scope=${scopeA}`];
  assert.equal(accountSessionDiagnosticScope(flags, 'development'), scopeA.replaceAll('-', ''));
  assert.equal(accountSessionDiagnosticScope([
    '--lingxi-host-diagnostics', '--lingxi-account-session-scope=0123456789ABCDEF0123456789ABCDEF',
  ], 'development'), '0123456789abcdef0123456789abcdef');
  for (const channel of ['development', 'production', undefined] as const) {
    assert.equal(accountSessionDiagnosticScope(['LingXi Code', '/workspace'], channel), undefined);
    assert.equal(accountSessionDiagnosticScope(['LingXi Code', '--', ...flags], channel), undefined,
      'workspace arguments cannot select host storage');
  }
  assert.equal(accountSessionDiagnosticScope([
    `/workspace/--lingxi-account-session-scope=${scopeA}`,
  ], 'development'), undefined);
  assert.throws(() => accountSessionDiagnosticScope(flags, 'production'), /development credential channel/);
  assert.throws(() => accountSessionDiagnosticScope(flags, undefined), /development credential channel/);
});

test('missing, malformed, or ambiguous diagnostic scopes fail before any default account access', () => {
  const fixture = mapBroker();
  const mode = '--lingxi-host-diagnostics';
  const scope = `--lingxi-account-session-scope=${scopeA}`;
  for (const argv of [[mode], [scope], [mode, scope, scope], [mode, mode, scope],
    [`${mode}=true`, scope], [mode, '--lingxi-account-session-scope']]) {
    assert.throws(() => accountSessionDiagnosticScope(argv, 'development'), /one explicit mode and account scope/);
  }
  for (const invalid of ['', ' ', '../lingxi-account-session', 'personal', 'a'.repeat(31), 'a'.repeat(33),
    '11111111-2222-4333-8444-55555555555g', `${'a'.repeat(32)}\n`, `${scopeA}\r\n`]) {
    assert.throws(() => accountSessionDiagnosticScope([mode, `--lingxi-account-session-scope=${invalid}`], 'development'), /Invalid host diagnostic account scope/);
    assert.throws(() => brokerAccountSessionStore(fixture.broker, invalid), /Invalid host diagnostic account scope/);
  }
  assert.throws(() => brokerAccountSessionStore({ ...fixture.broker, credentialChannel: 'production' }, scopeA), /development credential channel/);
  const { credentialChannel: _channel, ...unverifiedBroker } = fixture.broker;
  assert.throws(() => brokerAccountSessionStore(unverifiedBroker, scopeA), /development credential channel/);
  assert.deepEqual(fixture.calls, []);
});

test('packaged account storage is lazy and persists through the existing broker', async () => {
  const values = new Map<string, string>();
  let calls = 0;
  const broker = {
    async resolve(id: string) { calls++; return values.get(id); },
    async set(id: string, value: string) { calls++; values.set(id, value); return { providerId: id, configured: true }; },
    async delete(id: string) { calls++; values.delete(id); },
  };
  const first = brokerAccountSessionStore(broker);
  assert.equal(calls, 0, 'constructing the store must not make a synchronous Keychain probe');
  assert.equal(first.persistent, true);
  assert.equal(await first.read(), null);
  await first.write('{"refreshToken":"synthetic-test-token"}');
  const restarted = brokerAccountSessionStore(broker);
  assert.equal(await restarted.read(), '{"refreshToken":"synthetic-test-token"}');
  await restarted.remove();
  assert.equal(await first.read(), null);
});

test('broker failure propagates without an in-memory or plaintext write fallback', async () => {
  const store = brokerAccountSessionStore({
    async resolve() { throw new Error('broker unavailable'); },
    async set() { throw new Error('broker unavailable'); },
    async delete() { throw new Error('broker unavailable'); },
  });
  await assert.rejects(store.read(), /broker unavailable/);
  await assert.rejects(store.write('synthetic-test-token'), /broker unavailable/);
  await assert.rejects(store.remove(), /broker unavailable/);
});
