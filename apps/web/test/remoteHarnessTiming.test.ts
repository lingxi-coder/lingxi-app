import assert from 'node:assert/strict';
import test from 'node:test';
import { harness, trackRealWork } from './support/remoteHarness';

test('waiting for real signature verification does not expire the simulated answer deadline', async () => {
  const h = await harness();
  await h.trustDevice();
  const connection = h.connect();
  const subtle = crypto.subtle;
  const verify = subtle.verify;
  let release!: () => void;
  let signalStarted!: () => void;
  const blocked = new Promise<void>((resolve) => { release = resolve; });
  const started = new Promise<void>((resolve) => { signalStarted = resolve; });
  let delayed = false;
  Object.defineProperty(subtle, 'verify', {
    configurable: true,
    writable: true,
    value: (...args: Parameters<SubtleCrypto['verify']>) => {
      if (delayed) return Reflect.apply(verify, subtle, args);
      delayed = true;
      signalStarted();
      // The actual verification still runs, after a deliberately slow host.
      return trackRealWork(blocked.then(() => Reflect.apply(verify, subtle, args) as Promise<boolean>));
    },
  });
  let ready: Promise<void> | undefined;
  try {
    connection.start();
    await started;
    const before = h.clock.now();
    ready = h.until(connection, 'ready');
    void ready.catch(() => undefined);
    await new Promise<void>((resolve) => setTimeout(resolve, 30));
    assert.equal(h.clock.now(), before, 'the fake answer timeout must not race real signature verification');
    release();
    await ready;
    assert.equal(h.host.offers.length, 1);
    assert.equal(connection.state.transport, 'webrtc');
  } finally {
    release();
    Object.defineProperty(subtle, 'verify', { configurable: true, writable: true, value: verify });
    connection.stop();
    await ready?.catch(() => undefined);
    h.host.stop();
  }
});
