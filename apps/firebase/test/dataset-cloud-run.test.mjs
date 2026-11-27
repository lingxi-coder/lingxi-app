import assert from 'node:assert/strict';
import test from 'node:test';
import { createDatasetGateway } from '../src/dataset-product.mjs';
import { DATA_CONTRACT_SHA256, DATA_PROTOCOL_VERSION } from '../src/dataset-resource.mjs';
import { createPrivateDatasetFetch } from '../src/dataset-cloud-run.mjs';
import { LingxiDataError } from '../src/lingxi-data.mjs';
import { createTeamsHandler } from '../src/teams.mjs';

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

test('Core verifies direct Dataset commands while the product gateway verifies share and render', async () => {
  const events = [];
  const handle = createTeamsHandler({
    store: { db: {} },
    async verifyToken(token) {
      events.push(['product-auth', token]);
      return { uid: 'fixture-uid' };
    },
    async datasetCommand(token, body) {
      events.push(['core-auth', token, body.command.action]);
      if (token === 'revoked') throw new LingxiDataError(401, 'DATA_ACCESS_REQUIRED');
      return { forwarded: true };
    },
    async datasets(_db, user) {
      events.push(['share', user.uid]);
      return { shared: true };
    },
    async datasetRender(user) {
      events.push(['render', user.uid]);
      return { rendered: true };
    },
  });
  function request(body, token = 'active') {
    return handle(new Request('https://gateway.test/v1/teams/datasets', {
      method: 'POST', headers: { Authorization: 'Bearer ' + token }, body: JSON.stringify(body),
    }));
  }
  assert.deepEqual(await (await request({ command: { action: 'list' } })).json(), { forwarded: true });
  assert.equal((await request({ command: { action: 'list' } }, 'revoked')).status, 401);
  assert.deepEqual(await (await request({ action: 'share' })).json(), { shared: true });
  assert.deepEqual(await (await request({ action: 'render' })).json(), { rendered: true });
  assert.deepEqual(events, [
    ['core-auth', 'active', 'list'], ['core-auth', 'revoked', 'list'],
    ['product-auth', 'active'], ['share', 'fixture-uid'],
    ['product-auth', 'active'], ['render', 'fixture-uid'],
  ]);
});

test('a rolled-back Core requires gateway verification for commands and browser hosts', async () => {
  const path = [];
  let current = true;
  let verified = 0;
  const gateway = createDatasetGateway('https://core.test', async url => {
    path.push(url.pathname);
    if (url.pathname === '/v2/contract') return Response.json({ protocol: DATA_PROTOCOL_VERSION, contractSha256: DATA_CONTRACT_SHA256 });
    if (url.pathname.includes('/auth-v1/')) {
      return current ? Response.json({ error: 'business missing' }, { status: 404, headers: { 'x-lingxi-auth-version': '1' } })
        : new Response(null, { status: 404 });
    }
    return Response.json({ ok: true });
  }, async token => {
    verified++;
    if (token === 'revoked') throw new Error('revoked');
    return { uid: 'fixture-user' };
  });
  await assert.rejects(gateway.command('active', {}), { status: 404 });
  assert.equal(verified, 0);
  current = false;
  await assert.rejects(gateway.browser('host', 'Bearer revoked', {}), { status: 401 });
  assert.equal(path.at(-1), '/v2/auth-v1/dataset-browser-host');
  assert.deepEqual(await gateway.command('active', {}), { ok: true });
  assert.deepEqual(path.slice(-2), ['/v2/auth-v1/datasets', '/v2/datasets']);
  assert.equal(verified, 2);
});

test('Dataset gateway requires the exact Core contract before forwarding', async () => {
  const paths = [];
  const gateway = createDatasetGateway('https://core.test', async url => {
    paths.push(url.pathname);
    return Response.json({ protocol: DATA_PROTOCOL_VERSION, contractSha256: '0'.repeat(64) });
  });
  await assert.rejects(gateway.command('fixture', { command: { action: 'describe' } }), { code: 'DATA_CONTRACT_MISMATCH' });
  assert.deepEqual(paths, ['/v2/contract']);
});

test('Dataset limits apply to bytes before JSON parsing', async () => {
  const handle = createTeamsHandler({ store: { db: {} }, datasets: async () => assert.fail(),
    verifyToken: async () => assert.fail() });
  const init = { method: 'POST', headers: { Authorization: 'Bearer active' }, body: 'x'.repeat(131073) };
  assert.equal((await handle(new Request('https://team.test/v1/teams/datasets', init))).status, 413);
  assert.equal((await handle(new Request('https://team.test/v1/teams/dataset-captures', init))).status, 400);
});
