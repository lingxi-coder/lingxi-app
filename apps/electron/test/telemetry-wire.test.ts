import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { test } from 'node:test';
import { DesktopTelemetry } from '../src/main/telemetry.js';
import { resolveDesktopTelemetry } from '../src/main/telemetryPolicy.js';

test('the actual Node OTLP exporter sends the host root as HTTP protobuf, including chunked transport', async () => {
  const receipts: Array<{ path?: string; encoding?: string; contentType?: string; body: Buffer }> = [];
  const receiver = createServer(async (request, response) => {
    const chunks: Buffer[] = [];
    for await (const chunk of request) chunks.push(Buffer.from(chunk));
    receipts.push({ path: request.url, encoding: request.headers['transfer-encoding'],
      contentType: request.headers['content-type'], body: Buffer.concat(chunks) });
    response.writeHead(200, { 'Content-Type': 'application/x-protobuf', 'Content-Length': '0' });
    response.end();
  });
  await new Promise<void>((resolve, reject) => {
    receiver.once('error', reject);
    receiver.listen(0, '127.0.0.1', resolve);
  });
  const address = receiver.address();
  assert.ok(address && typeof address === 'object');
  const telemetry = new DesktopTelemetry(resolveDesktopTelemetry({
    LINGXI_ENABLE_TELEMETRY: 'true', OTEL_EXPORTER_OTLP_ENDPOINT: `http://127.0.0.1:${address.port}/otel`,
  }), 'wire-test');
  try {
    const parent = telemetry.beginTurn(1)!;
    telemetry.onEvent({ type: 'turn_started', turn_id: 1 });
    telemetry.onEvent({ type: 'text_delta', text: 'PRIVATE_RESPONSE_CANARY_WIRE' });
    telemetry.onEvent({ type: 'turn_ended', outcome: { type: 'end_turn' }, cost: {} } as never);
    await telemetry.forceFlush();
    assert.equal(receipts.length, 1, 'a real SDK span must reach the HTTP receiver');
    const receipt = receipts[0]!;
    assert.equal(receipt.path, '/otel/v1/traces');
    assert.equal(receipt.contentType, 'application/x-protobuf');
    assert.equal(receipt.encoding, 'chunked', 'the receiver must decode chunks, not assume Content-Length');
    assert.ok(receipt.body.includes(Buffer.from('lingxi.desktop.request')));
    assert.ok(receipt.body.includes(Buffer.from(parent.traceparent.split('-')[1]!, 'hex')));
    assert.ok(receipt.body.includes(Buffer.from(parent.traceparent.split('-')[2]!, 'hex')));
    assert.equal(receipt.body.includes(Buffer.from('PRIVATE_RESPONSE_CANARY_WIRE')), false);
    telemetry.disable();
    await telemetry.shutdown();
    assert.equal(telemetry.beginTurn(2), undefined);
    await telemetry.forceFlush();
    assert.equal(receipts.length, 1, 'revocation prevents any later host request');
  } finally {
    await telemetry.shutdown();
    receiver.closeAllConnections();
    await new Promise<void>((resolve, reject) => receiver.close(error => error ? reject(error) : resolve()));
  }
});
