import assert from 'node:assert/strict';
import test from 'node:test';
import { createPrivateDatasetFetch } from '../src/dataset-cloud-run.mjs';

test('managed Dataset transport preserves the application credential and pins the workload audience', async () => {
  const origin = 'https://test-replica.run.app';
  const audiences = [];
  const auth = { async getIdTokenClient(audience) {
    audiences.push(audience);
    return { async getRequestHeaders() { return new Headers({ Authorization: 'Bearer workload-fixture' }); } };
  } };
  const requests = [];
  const send = createPrivateDatasetFetch(origin, auth, async (url, init) => {
    requests.push({ url: url.href, headers: Object.fromEntries(init.headers), redirect: init.redirect });
    return new Response('{}');
  });
  await send(new URL('/v2/contract', origin));
  await send(new URL('/v2/datasets', origin), { headers: { Authorization: 'Bearer user-fixture' } });
  assert.deepEqual(audiences, [origin]);
  assert.deepEqual(requests[1].headers, { authorization: 'Bearer user-fixture', 'x-serverless-authorization': 'Bearer workload-fixture' });
  assert.equal(requests[1].redirect, 'error');
  await assert.rejects(send('https://other.run.app/v2/datasets'));
  assert.equal(requests.length, 2);
});
