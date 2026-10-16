import assert from 'node:assert/strict';
import { test } from 'node:test';
import { watchCredentialRecovery } from '../src/renderer/bridge/credentialRecovery.js';
import { HostController } from '../src/main/host.js';
import { DiagnosticBuffer } from '../src/main/host-utils.js';

test('bootstrap recovers persisted Codex status after a keychain error without relogin', async () => {
  let locked = true;
  const settings = {
    getWorkspace: () => '/workspace',
    getTrust: () => ({ trusted: true, fingerprint: 'fingerprint' }),
    getPublic: () => ({ version: 1, activeProject: '/workspace', projects: ['/workspace'], pinnedSessions: [] }),
    credentialMetadata: () => ({ configured: false, encryptionAvailable: false }),
    providerCredentialMetadataFor: () => [],
  };
  const broker = {
    listStatus: async () => {
      if (locked) throw new Error('list secure storage accounts failed (-25308): User interaction is not allowed.');
      return [{ providerId: 'openai-chatgpt', configured: true }];
    },
    resolve: async () => undefined,
  };
  const host = new HostController(settings as any, {
    get: () => undefined, runtimeSummaries: [],
  } as any, new DiagnosticBuffer(), undefined, undefined, undefined, broker as any);
  const read = async () => (await (host as any).bootstrap()).providerCredentials
    .find((entry: { providerId: string }) => entry.providerId === 'openai-chatgpt');
  const unavailable = await read();
  assert.equal(unavailable.configured, false);
  assert.equal(unavailable.encryptionAvailable, false);
  assert.ok(unavailable.storageError);
  locked = false;
  assert.deepEqual(await read(), {
    providerId: 'openai-chatgpt', configured: true, encryptionAvailable: true,
  }, 'the failed query must not poison the status cache or preserve the storage error');
});

test('locked storage retries on focus and visibility, serializes requests and stops on cleanup', async (t) => {
  t.mock.timers.enable({ apis: ['setInterval'] });
  const windowTarget = new EventTarget();
  const documentTarget = new EventTarget();
  let visible = false;
  let calls = 0;
  let release: (() => void) | undefined;
  const stop = watchCredentialRecovery({
    windowTarget, documentTarget, isVisible: () => visible,
    refresh: () => { calls += 1; return new Promise<void>((resolve) => { release = resolve; }); },
  });
  t.mock.timers.tick(5_000);
  assert.equal(calls, 0, 'hidden windows must not poll the keychain');
  visible = true;
  documentTarget.dispatchEvent(new Event('visibilitychange'));
  windowTarget.dispatchEvent(new Event('focus'));
  t.mock.timers.tick(5_000);
  assert.equal(calls, 1, 'concurrent triggers must share one recovery request');
  release?.();
  await new Promise<void>((resolve) => setImmediate(resolve));
  t.mock.timers.tick(5_000);
  assert.equal(calls, 2, 'visible locked window retries even without a focus event');
  stop();
  release?.();
  await new Promise<void>((resolve) => setImmediate(resolve));
  windowTarget.dispatchEvent(new Event('focus'));
  documentTarget.dispatchEvent(new Event('visibilitychange'));
  t.mock.timers.tick(10_000);
  assert.equal(calls, 2, 'recovered or unmounted window must stop all retries');
});

test('a failed recovery request remains retryable', async (t) => {
  t.mock.timers.enable({ apis: ['setInterval'] });
  const target = new EventTarget();
  let calls = 0;
  const stop = watchCredentialRecovery({
    windowTarget: target, documentTarget: target, isVisible: () => true,
    refresh: async () => { calls += 1; if (calls === 1) throw new Error('keychain still locked'); },
  });
  try {
    t.mock.timers.tick(5_000);
    await new Promise<void>((resolve) => setImmediate(resolve));
    t.mock.timers.tick(5_000);
    await new Promise<void>((resolve) => setImmediate(resolve));
    assert.equal(calls, 2);
  } finally { stop(); }
});
