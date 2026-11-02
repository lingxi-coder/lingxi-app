import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createProviderAccountSnapshotRefresh, providerAccountRecoveryState, providerAccountUnavailableReasons, watchCredentialRecovery } from '../src/renderer/bridge/credentialRecovery';
import { fixtureAccountSnapshot } from './fixtures/provider-accounts-mock';

const settle = async () => { await Promise.resolve(); await Promise.resolve(); await Promise.resolve(); };

test('healthy catalog publication invalidates an older snapshot and coalesces one fresh Host projection', async () => {
  const old = fixtureAccountSnapshot(['openai-chatgpt']);
  assert.equal(providerAccountRecoveryState(old).recovering, false);
  const published = structuredClone(old);
  published.modelAvailability!.push({ ...published.modelAvailability![0]!, modelId: 'new-published-model' });
  const reads: Array<(snapshot: typeof old) => void> = [];
  const applied: typeof old[] = [];
  const pending: boolean[] = [];
  const refresh = createProviderAccountSnapshotRefresh({ read: () => new Promise(resolve => reads.push(resolve)),
    source: () => 'owner', isCurrent: source => source === 'owner', apply: snapshot => applied.push(snapshot), pending: value => pending.push(value) });
  const beforePublication = refresh.refresh();
  await settle();
  const onPublication = refresh.refresh();
  const onModelList = refresh.refresh();
  await settle();
  assert.equal(reads.length, 1);
  reads[0]!(old); await settle();
  assert.equal(reads.length, 2);
  assert.deepEqual(applied, [], 'a pre-publication result cannot flash Connect after the new catalog');
  reads[1]!(published);
  assert.deepEqual(await Promise.all([beforePublication, onPublication, onModelList]), [published, published, published]);
  assert.deepEqual(applied, [published]);
  assert.deepEqual(pending, [true, false]);
});

test('an account snapshot from a previous source is discarded even when its config revision matches', async () => {
  const snapshot = fixtureAccountSnapshot(['openai-chatgpt']);
  let source = 1, finish!: (value: typeof snapshot) => void;
  const applied: number[] = [];
  const refresh = createProviderAccountSnapshotRefresh({ read: () => new Promise(resolve => { finish = resolve; }),
    source: () => source, isCurrent: captured => captured === source, apply: (_value, captured) => applied.push(captured), pending: () => {} });
  const old = refresh.refresh(); await settle(); source = 2; finish(snapshot); await old;
  assert.deepEqual(applied, []);
  const current = refresh.refresh(); await settle(); finish(snapshot); await current;
  assert.deepEqual(applied, [2]);
});

test('a synchronous snapshot transport failure releases the pending read for retry', async () => {
  let fail = true;
  const snapshot = fixtureAccountSnapshot(['openai-chatgpt']);
  const refresh = createProviderAccountSnapshotRefresh({ read: () => { if (fail) throw new Error('transport unavailable'); return Promise.resolve(snapshot); },
    source: () => 'owner', isCurrent: () => true, apply: () => {}, pending: () => {} });
  await assert.rejects(refresh.refresh(), /transport unavailable/);
  fail = false;
  assert.equal(await refresh.refresh(), snapshot);
});

test('a metadata deadline wakes an unavailable account without a business call or changing stable priority', async t => {
  t.mock.timers.enable({ apis: ['setTimeout', 'setInterval', 'Date'], now: 1_000 });
  let snapshot = fixtureAccountSnapshot(['openai-chatgpt', 'openai']);
  snapshot.health[0] = { ...snapshot.health[0]!, status: 'cooling_down', cooldownUntilMs: 1_500 };
  snapshot.modelEntitlements = [{ accountId: snapshot.accounts[1]!.accountId, identityGeneration: 1, modelId: 'fixture-model', status: 'forbidden', observedAtMs: 1_000, retryAtMs: 2_000 }];
  const before = structuredClone(snapshot.accounts);
  const recovery = providerAccountRecoveryState(snapshot);
  assert.equal(recovery.retryAtMs, 1_500);
  assert.equal(recovery.recovering, true);
  let reads = 0;
  const stop = watchCredentialRecovery({ windowTarget: new EventTarget(), documentTarget: new EventTarget(), isVisible: () => true,
    retryAtMs: recovery.retryAtMs, pollIntervalMs: 30_000, refresh: async () => {
      reads++; snapshot = { ...snapshot, health: snapshot.health.map(row => ({ ...row, status: 'ready' })) };
    } });
  t.mock.timers.tick(524); await settle(); assert.equal(reads, 0);
  t.mock.timers.tick(1); await settle(); assert.equal(reads, 1);
  assert.deepEqual(snapshot.accounts, before, 'read-only recovery preserves IDs, bindings and user order');
  stop(); t.mock.timers.tick(60_000); await settle(); assert.equal(reads, 1);
});

test('foreground recovery coalesces in-flight reads, defers hidden deadlines and cleans up listeners', async t => {
  t.mock.timers.enable({ apis: ['setTimeout', 'setInterval', 'Date'], now: 1_000 });
  const windowTarget = new EventTarget(), documentTarget = new EventTarget();
  let visible = false, reads = 0, finish!: () => void;
  const stop = watchCredentialRecovery({ windowTarget, documentTarget, isVisible: () => visible, retryAtMs: 1_200, pollIntervalMs: 30_000,
    refresh: () => { reads++; return new Promise<void>(resolve => { finish = resolve; }); } });
  t.mock.timers.tick(225); await settle(); assert.equal(reads, 0);
  visible = true; documentTarget.dispatchEvent(new Event('visibilitychange')); windowTarget.dispatchEvent(new Event('focus'));
  assert.equal(reads, 1);
  t.mock.timers.tick(30_000); assert.equal(reads, 1);
  finish(); await settle();
  windowTarget.dispatchEvent(new Event('focus')); assert.equal(reads, 2); finish(); await settle();
  stop(); windowTarget.dispatchEvent(new Event('focus')); documentTarget.dispatchEvent(new Event('visibilitychange'));
  t.mock.timers.tick(60_000); assert.equal(reads, 2);
});

test('temporary account and model denial display their canonical reason instead of initial setup', () => {
  const snapshot = fixtureAccountSnapshot(['openai-chatgpt']);
  snapshot.health[0] = { ...snapshot.health[0]!, status: 'quota_exhausted', cooldownUntilMs: 42 };
  assert.deepEqual(providerAccountUnavailableReasons(snapshot), ['provider_accounts_health_quota_exhausted']);
  snapshot.health[0] = { ...snapshot.health[0]!, status: 'ready' };
  snapshot.modelEntitlements = [{ accountId: snapshot.accounts[0]!.accountId, identityGeneration: 1, modelId: 'fixture-model', status: 'forbidden', observedAtMs: 1, retryAtMs: 42 }];
  assert.equal(providerAccountRecoveryState(snapshot).recovering, true);
  assert.deepEqual(providerAccountUnavailableReasons(snapshot), ['provider_accounts_model_forbidden']);
  assert.deepEqual(providerAccountUnavailableReasons(fixtureAccountSnapshot([])), []);
});


test('model and endpoint scope TTLs use live identity and future deadlines with a bounded no-eligibility fallback', async t => {
  t.mock.timers.enable({ apis: ['setTimeout', 'setInterval', 'Date'], now: 1_000 });
  const snapshot = fixtureAccountSnapshot(['openai-chatgpt']);
  const accountId = snapshot.accounts[0]!.accountId;
  snapshot.modelAvailability = snapshot.modelAvailability?.map(row => ({ ...row, eligibleAccountIds: [] }));
  snapshot.scopedHealth = [
    { accountId, identityGeneration: 2, scopeKind: 'model', scopeId: 'stale', unavailableUntilMs: 1_100, reason: 'rate_limited' },
    { accountId, identityGeneration: 1, scopeKind: 'model', scopeId: 'expired', unavailableUntilMs: 900, reason: 'rate_limited' },
    { accountId, identityGeneration: 1, scopeKind: 'endpoint', scopeId: 'test-endpoint', unavailableUntilMs: 1_500, reason: 'quota_exhausted' },
  ];
  assert.equal(providerAccountRecoveryState(snapshot).retryAtMs, 1_500);
  assert.ok(providerAccountUnavailableReasons(snapshot).includes('provider_accounts_health_quota_exhausted'));
  snapshot.scopedHealth = [];
  const fallback = providerAccountRecoveryState(snapshot);
  assert.equal(fallback.recovering, true); assert.equal(fallback.retryAtMs, undefined);
  let reads = 0;
  const stop = watchCredentialRecovery({ windowTarget: new EventTarget(), documentTarget: new EventTarget(), isVisible: () => true,
    retryAtMs: 900, pollIntervalMs: 30_000, refresh: async () => { reads++; } });
  t.mock.timers.tick(29_999); await settle(); assert.equal(reads, 0, 'expired/server-skewed deadlines cannot cause immediate read loops');
  t.mock.timers.tick(1); await settle(); assert.equal(reads, 1); stop();
});
