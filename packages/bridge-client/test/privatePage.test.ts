import assert from 'node:assert/strict';
import { test } from 'node:test';
import { BridgeClient } from '../src/client.js';
import type { Frame } from '../src/protocol.js';
import { validateClientEvent, validatePrivatePageServe } from '../src/validation.js';

function wire(reply: unknown) {
  const client = new BridgeClient();
  const internal = client as unknown as {
    ws: unknown; onMessage(data: Buffer): void;
  };
  const sent: Array<Extract<Frame, { type: 'request' }>> = [];
  internal.ws = { readyState: 1, send(value: string) {
    const frame = JSON.parse(value) as Extract<Frame, { type: 'request' }>;
    sent.push(frame);
    internal.onMessage(Buffer.from(JSON.stringify({
      type: 'response', payload: { id: frame.payload.id, result: reply },
    })));
  } };
  return { client, sent };
}

test('page_block events carry a reference exactly when ready', () => {
  assert.deepEqual(
    validateClientEvent({ type: 'page_block', status: 'ready', reference: { id: 'notes', revision: 1 } }),
    { type: 'page_block', status: 'ready', reference: { id: 'notes', revision: 1 } },
  );
  for (const status of ['pending', 'unavailable', 'discarded']) {
    assert.deepEqual(validateClientEvent({ type: 'page_block', status }), { type: 'page_block', status });
  }
  for (const bad of [
    { type: 'page_block', status: 'ready' },
    { type: 'page_block', status: 'pending', reference: { id: 'notes', revision: 1 } },
    { type: 'page_block', status: 'ready', reference: { id: '../x', revision: 1 } },
    { type: 'page_block', status: 'ready', reference: { id: 'notes', revision: 0 } },
    { type: 'page_block', status: 'ready', reference: { id: 'notes', revision: 1, extra: 1 } },
    { type: 'page_block', status: 'done' },
    { type: 'page_block', status: 'pending', extra: true },
  ]) {
    assert.throws(() => validateClientEvent(bad), undefined, JSON.stringify(bad));
  }
});

test('private page serve asks this computer and rejects a public status', async () => {
  const served = {
    status: 200,
    headers: [['Content-Type', 'text/html; charset=utf-8'], ['Cache-Control', 'no-store']],
    body_base64: 'PGgxPg==',
  };
  const fixture = wire(served);
  assert.deepEqual(await fixture.client.privatePageServe('notes', 1), served);
  assert.equal(fixture.sent[0]?.payload.method, 'private_page');
  assert.deepEqual(fixture.sent[0]?.payload.params, { op: 'serve', id: 'notes', revision: 1 });
  assert.throws(() => validatePrivatePageServe({ status: 500, headers: [], body_base64: '' }));
  assert.throws(() => validatePrivatePageServe({ status: 200, headers: [['only-name']], body_base64: '' }));
});
