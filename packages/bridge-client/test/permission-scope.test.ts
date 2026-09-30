import assert from 'node:assert/strict';
import { test } from 'node:test';
import { BridgeClient } from '../src/client.js';
import { validatePermissionScope } from '../src/validation.js';
import type { Frame } from '../src/protocol.js';

function wire(reply: unknown) {
  const client = new BridgeClient();
  const internal = client as unknown as {
    ws: unknown; onMessage(data: Buffer): void; onClose(code: number, reason: string): void;
    pendingResponses: Map<number, unknown>;
  };
  let sent: Extract<Frame, { type: 'request' }> | undefined;
  internal.ws = { readyState: 1, send(value: string) {
    sent = JSON.parse(value);
    if (reply !== undefined) internal.onMessage(Buffer.from(JSON.stringify({
      type: 'response', payload: { id: sent!.payload.id, result: reply },
    })));
  } };
  return { client, internal, sent: () => sent };
}

test('permission scope uses a correlated product RPC and validates foreground/background', async () => {
  for (const background_owned of [false, true]) {
    const scope = { request_id: 19, background_owned };
    const fixture = wire(scope);
    assert.deepEqual(await fixture.client.requestPermissionScope(19), scope);
    assert.equal(fixture.sent()?.payload.method, 'permission_request_scope');
    assert.deepEqual(fixture.sent()?.payload.params, { request_id: 19 });
    assert.equal(fixture.internal.pendingResponses.size, 0);
  }
  assert.equal(await wire(null).client.requestPermissionScope(19), null);
});

test('permission scope rejects wrong identities and malformed ownership', async () => {
  for (const reply of [
    {}, { request_id: 20, background_owned: true },
    { request_id: 19, background_owned: 'true' },
    { request_id: 19, background_owned: true, worker_name: 'guess' },
  ]) await assert.rejects(wire(reply).client.requestPermissionScope(19));
  for (const id of [-1, 0.5, Number.MAX_SAFE_INTEGER + 1]) {
    const fixture = wire(null);
    await assert.rejects(fixture.client.requestPermissionScope(id), /invalid permission request id/);
    assert.equal(fixture.sent(), undefined);
  }
  assert.throws(() => validatePermissionScope(undefined, 19));
});

test('disconnect rejects a pending scope lookup', async () => {
  const fixture = wire(undefined);
  const pending = fixture.client.requestPermissionScope(19);
  const rejected = assert.rejects(pending, /connection closed/);
  fixture.internal.onClose(1006, 'disconnected');
  await rejected;
  assert.equal(fixture.internal.pendingResponses.size, 0);
});
