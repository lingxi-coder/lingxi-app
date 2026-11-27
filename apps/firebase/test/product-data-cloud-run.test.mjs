import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { createCoreTokenRefresh } from '../scripts/start-cloud-data-core.mjs';
import { createPrivateDatasetFetch } from '../src/dataset-cloud-run.mjs';
import { createProductDataListener, createProductDataTransport } from '../src/lingxi-data-product.mjs';
import { createTeamsHandler } from '../src/teams.mjs';

test('Cloud Run shares refreshes and atomically replaces a short-lived private Core token', async t => {
  const dir = await mkdtemp(join(tmpdir(), 'data-token-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const path = join(dir, 'token');
  let clock = 1000;
  let calls = 0;
  let failing = false;
  const refresh = createCoreTokenRefresh(path, async (url, options) => {
    assert.equal(url, 'http://169.254.169.254/computeMetadata/v1/instance/service-accounts/default/token');
    assert.equal(options.headers['Metadata-Flavor'], 'Google');
    assert.equal(options.redirect, 'error');
    calls++;
    return Response.json({ access_token: 'token-' + calls, token_type: 'Bearer', expires_in: 3600 },
      { status: failing ? 503 : 200, headers: { 'Metadata-Flavor': 'Google' } });
  }, () => clock);
  await Promise.all([refresh(), refresh()]);
  assert.equal(calls, 1);
  assert.equal(await readFile(path, 'utf8'), 'token-1');
  assert.equal((await stat(path)).mode & 0o777, 0o600);
  clock += 3_539_000;
  await refresh();
  assert.equal(calls, 1);
  clock += 1000;
  failing = true;
  await assert.rejects(refresh(), /workload token/u);
  assert.equal(await readFile(path, 'utf8'), 'token-1');
  failing = false;
  await refresh();
  assert.equal(await readFile(path, 'utf8'), 'token-3');
});

test('Teams sends the caller and workload tokens to the private personal dataset catalog', async () => {
  const origin = 'https://product-data-test.run.app';
  const body = { scope: { kind: 'personal' }, command: { action: 'datasets' } };
  const adapter = createProductDataListener({
    verifyToken: async token => {
      assert.equal(token, 'firebase-fixture');
      return { uid: 'dataset-owner' };
    },
    gateway: async (uid, token, input) => {
      assert.equal(uid, 'dataset-owner');
      assert.equal(token, 'firebase-fixture');
      assert.deepEqual(input, body);
      return { datasets: [{ dataset: 'personal-orders', access: 'manage' }] };
    },
  });
  const workload = {
    async getIdTokenClient(audience) {
      assert.equal(audience, origin);
      return { async getRequestHeaders() { return new Headers({ Authorization: 'Bearer workload-fixture' }); } };
    },
  };
  const forward = createPrivateDatasetFetch(origin, workload, async (url, init) => {
    assert.equal(url.pathname, '/v1/product-data');
    assert.equal(init.headers.get('x-serverless-authorization'), 'Bearer workload-fixture');
    const req = {
      url: url.pathname, method: init.method, headers: Object.fromEntries(init.headers),
      async *iterator() { yield Buffer.from(init.body); },
    };
    let status;
    return new Promise((resolve, reject) => {
      adapter(req, {
        writeHead(code) { status = code; },
        end(value) { resolve(new Response(value, { status })); },
      }).catch(reject);
    });
  });
  const teams = createTeamsHandler({
    store: { db: {} },
    verifyToken: async () => ({ uid: 'dataset-owner' }),
    data: createProductDataTransport(origin, forward),
  });
  const result = await teams(new Request('https://teams.test/v1/teams/data', {
    method: 'POST', headers: { Authorization: 'Bearer firebase-fixture' }, body: JSON.stringify(body),
  }));
  assert.equal(result.status, 200);
  assert.deepEqual(await result.json(), { datasets: [{ dataset: 'personal-orders', access: 'manage' }] });
});
