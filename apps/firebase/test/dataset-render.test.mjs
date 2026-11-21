import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import test from 'node:test';
import { createRenderer } from '../../dataset-renderer/supervisor.mjs';
import { createDatasetRender } from '../src/dataset-render.mjs';

const resource = { page: 'orders', version: 'a'.repeat(64) };
const owner = { uid: 'reader-one' };
const otherReader = { uid: 'reader-two' };
const frame = { png: 'test-frame', accessibility: [] };

function fixture(t, workerFrame = frame) {
  const secret = randomBytes(32).toString('base64url');
  const view = { tenant: 'orders-team', actions: {}, bundle: { html: '<p>Orders</p>', assets: {}, actions: {} }, state: null };
  let active = true;
  let onFrame = () => {};
  let stopped = 0;
  let now = 0;
  function authorize() {
    if (!active) throw Object.assign(new Error('DATA_ACCESS_REQUIRED'), { status: 403 });
    return view;
  }
  const supervisor = createRenderer({ secret, clock: () => now, worker: () => ({
    async request() {
      onFrame();
      return workerFrame;
    },
    stop() { stopped++; },
  }) });
  t.after(() => supervisor.dispose());
  const run = createDatasetRender({
    origin: 'https://renderer.example',
    secret,
    pages: { openForRenderer: authorize, verifyRenderer: authorize },
    db: { collection: () => ({ limit: () => ({ get: async () => ({ docs: [] }) }) }) },
    fetchRequest: (url, options) => supervisor.fetch(new Request(url, options)),
  });
  return {
    mount: () => run(owner, 'viewer-token', { action: 'mount', ...resource, dimensions: { width: 620, height: 480 } }),
    run: (user, request) => run(user, 'viewer-token', { ...resource, ...request }),
    revoke: () => { active = false; },
    afterFrame: hook => { onFrame = hook; },
    advance: milliseconds => { now += milliseconds; },
    stopped: () => stopped,
  };
}

test('the next render request after revocation stops the scoped worker', async t => {
  const render = fixture(t);
  const { session } = await render.mount();
  await assert.rejects(render.run(otherReader, { action: 'refresh', session }), { status: 410 });
  render.revoke();
  await assert.rejects(render.run(otherReader, { action: 'close', session }), { status: 410 });
  await assert.rejects(render.run(owner, { action: 'close', session, page: 'another-page' }), { status: 410 });
  await assert.rejects(render.run(owner, { action: 'refresh', session }), { status: 403 });
  assert.equal(render.stopped(), 1);
  await assert.rejects(render.run(owner, { action: 'close', session }), { status: 410 });
});

test('reader can close a worker after losing page access', async t => {
  const render = fixture(t);
  const { session } = await render.mount();
  render.revoke();
  assert.deepEqual(await render.run(owner, { action: 'close', session }), { closed: true });
  assert.equal(render.stopped(), 1);
});

test('access revoked while rendering discards the frame and releases the worker', async t => {
  const render = fixture(t);
  const { session } = await render.mount();
  render.afterFrame(render.revoke);
  await assert.rejects(render.run(owner, { action: 'refresh', session }), { status: 403 });
  assert.equal(render.stopped(), 1);
});

test('two workers per reader and expired sessions release capacity', async t => {
  const render = fixture(t);
  const { session } = await render.mount();
  await render.mount();
  await assert.rejects(render.mount(), { status: 429 });
  render.advance(5 * 60_000 + 1);
  await assert.rejects(render.run(owner, { action: 'refresh', session }), { status: 410 });
  assert.equal(render.stopped(), 2);
  await render.mount();
});

test('selected text is returned only in response to an authorized explicit request', async t => {
  const render = fixture(t, { ...frame, selectedText: 'private report total' });
  const mounted = await render.mount();
  assert.equal(mounted.selectedText, undefined);
  const { session } = mounted;
  assert.equal((await render.run(owner, { action: 'refresh', session })).selectedText, undefined);
  const copy = { action: 'input', session, event: { kind: 'selection' } };
  await assert.rejects(render.run(otherReader, copy), { status: 410 });
  assert.equal((await render.run(owner, copy)).selectedText, 'private report total');
  render.revoke();
  await assert.rejects(render.run(owner, copy), { status: 403 });
});
