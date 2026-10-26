import assert from 'node:assert/strict';
import { test } from 'node:test';
import { BridgeClient } from '@lingxi/bridge-client';
import { SessionRuntime } from '../src/main/bridge.js';
import { resolveDesktopTelemetry } from '../src/main/telemetryPolicy.js';

test('live revoke during hello waits for the bridge ACK before the runtime becomes ready', async () => {
  let enabled = true;
  let release!: () => void;
  const handshake = new Promise<void>(resolve => { release = resolve; });
  let revoked = 0;
  const originalConnect = BridgeClient.prototype.connect;
  BridgeClient.prototype.connect = async function () {
    this.sendCommand = () => {};
    this.disableTelemetry = async () => { revoked += 1; };
    await handshake;
    return { server_name: 'fixture', protocol_version: '0.2.0', capabilities: { client_protocol_version: '11.0.0' } } as never;
  };
  const runtime = new SessionRuntime({
    launchConfig: () => ({ workspace: '/fixture', trusted: true }),
    getTelemetryConfig: () => resolveDesktopTelemetry({}, { telemetryEnabled: enabled }),
  });
  const internal = runtime as unknown as {
    connectBridgeClient(path: string, generation: number, restorePermission: boolean): Promise<void>;
    restartChain: Promise<void>;
    client: BridgeClient | null;
  };
  try {
    const connecting = internal.connectBridgeClient('/unused/4318.lock', 0, false);
    internal.restartChain = connecting;
    enabled = false;
    let confirmed = false;
    const disabling = runtime.disableTelemetry().then(() => { confirmed = true; });
    await Promise.resolve();
    assert.equal(confirmed, false, 'the spawned process has not acknowledged off yet');
    release();
    await connecting;
    assert.ok(revoked > 0, 'revocation is sent after hello before connected state is published');
    await disabling;
    assert.equal(confirmed, true);
    assert.equal(runtime.connectionState.status, 'connected', 'diagnostics revocation does not cancel the conversation');
  } finally {
    BridgeClient.prototype.connect = originalConnect;
    internal.client = null;
    await runtime.dispose();
  }
});

test('live revoke confirms only after already-dispatched host transport settles', async () => {
  let release!: () => void;
  const transport = new Promise<void>(resolve => { release = resolve; });
  let hostRevoked = false;
  let bridgeRevoked = false;
  const runtime = new SessionRuntime({ launchConfig: () => ({ workspace: '/fixture', trusted: true }) });
  const internal = runtime as unknown as { state: unknown; telemetry: unknown; client: unknown };
  internal.state = { status: 'connected' };
  internal.telemetry = { disable: () => { hostRevoked = true; }, shutdown: () => transport };
  internal.client = { disableTelemetry: async () => { bridgeRevoked = true; } };
  let confirmed = false;
  const disabling = runtime.disableTelemetry().then(() => { confirmed = true; });
  assert.equal(hostRevoked, true, 'new host exports are blocked synchronously');
  await Promise.resolve();
  assert.equal(bridgeRevoked, true, 'Rust revocation proceeds while host transport settles');
  assert.equal(confirmed, false);
  release();
  await disabling;
  assert.equal(confirmed, true);
  internal.client = null;
  internal.telemetry = undefined;
  await runtime.dispose();
});
