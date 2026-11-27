import assert from 'node:assert/strict';
import test from 'node:test';
import {
  openVisualization, prepareVisualizationPackage, uploadVisualizationParts, visualizationPartBytes,
} from '../../../packages/teams/visualization-transfer.js';

test('visualization chunks preserve a UTF-8 character across the part boundary and keep packaged assets separate', async () => {
  const html = '<p>' + ' '.repeat(visualizationPartBytes - 4) + '订单</p>';
  const bundle = { title: 'Orders', html, assets: { 'style.css': { mime: 'text/css', data: 'cCB7IGNvbG9yOiByZWQgfQ==' } } };
  const transfer = await prepareVisualizationPackage(bundle);
  assert.equal(transfer.package.parts.length, 2);
  const parts = new Map<number, string>();
  const request = async <T>(body: Record<string, unknown>): Promise<T> => {
    if (body.action === 'putPart') {
      parts.set(Number(body.index), String(body.data));
      return { ok: true } as T;
    }
    if (body.action === 'open') return { package: transfer.package, state: { version: 0 } } as T;
    assert.equal(body.action, 'readPart');
    return { data: parts.get(Number(body.index)) } as T;
  };
  await uploadVisualizationParts(request, transfer, {
    status: 'uploading', teamId: 'team', conversationId: 'orders', generation: 1,
    message: 'viz_test', version: 'a'.repeat(64), missing: [0, 1],
  });
  assert.deepEqual(await openVisualization(request), { bundle, state: { version: 0 } });
});
