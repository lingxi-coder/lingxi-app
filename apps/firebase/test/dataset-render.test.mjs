import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import test from 'node:test';
import { createRenderer } from '../../dataset-renderer/supervisor.mjs';
import { createDatasetRender } from '../src/dataset-render.mjs';

const resource = { page: 'orders', version: 'a'.repeat(64) };
const owner = { uid: 'reader-one' };
const otherReader = { uid: 'reader-two' };
const frame = { png: 'test-frame', accessibility: [] };

function fixture(t, workerFrame = frame, { actions = {}, getApprovals, capacity, stopWorker } = {}) {
  const secret = randomBytes(32).toString('base64url');
  const view = { tenant: 'orders-team', actions, bundle: { html: '<p>Orders</p>', assets: {}, actions: {} }, state: null };
  let active = true;
  let onFrame = () => {};
  let stopped = 0;
  let now = 0;
  const requests = [];
  function authorize() {
    if (!active) throw Object.assign(new Error('DATA_ACCESS_REQUIRED'), { status: 403 });
    return view;
  }
  const supervisor = createRenderer({ secret, capacity, clock: () => now, worker: () => ({
    async request(request) {
      requests.push(request);
      onFrame();
      return typeof workerFrame === 'function' ? workerFrame(request) : workerFrame;
    },
    stop() { stopped++; return stopWorker?.(); },
  }) });
  t.after(() => supervisor.dispose());
  const approvals = {
    where() { return this; },
    limit() { return this; },
    async get() { return getApprovals?.() ?? { docs: [] }; },
  };
  const run = createDatasetRender({
    origin: 'https://renderer.example',
    secret,
    pages: { openForRenderer: authorize, verifyRenderer: authorize, execute: async () => ({ value: 3 }) },
    db: { collection: () => approvals },
    fetchRequest: (url, options) => supervisor.fetch(new Request(url, options)),
  });
  return {
    mount: () => run(owner, 'viewer-token', { action: 'mount', ...resource, dimensions: { width: 620, height: 480 } }),
    run: (user, request) => run(user, 'viewer-token', { ...resource, ...request }),
    revoke: () => { active = false; },
    afterFrame: hook => { onFrame = hook; },
    advance: milliseconds => { now += milliseconds; },
    stopped: () => stopped,
    requests,
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

test('capacity is shared across readers and remains reserved until cleanup completes', async t => {
  let release;
  const removing = new Promise(resolve => { release = resolve; });
  t.after(release);
  const render = fixture(t, frame, { capacity: 1, stopWorker: () => removing });
  const dimensions = { width: 620, height: 480 };
  const opened = await render.mount();
  await assert.rejects(render.run(otherReader, { action: 'mount', dimensions }), { status: 429 });
  const closing = render.run(owner, { action: 'close', session: opened.session });
  while (!render.stopped()) await new Promise(resolve => setImmediate(resolve));
  await assert.rejects(render.run(otherReader, { action: 'mount', dimensions }), { status: 429 });
  release();
  await closing;
  assert.ok((await render.run(otherReader, { action: 'mount', dimensions })).session);
});

test('failed mount releases the worker before returning the error', async t => {
  const failure = new Error('approvals unavailable');
  const render = fixture(t, frame, { getApprovals: () => { throw failure; } });
  await assert.rejects(render.mount(), failure);
  assert.equal(render.stopped(), 1);
});

test('render action outcomes drain without intermediate captures', async t => {
  const options = { actions: { total: { mode: 'latestRecord' } } };
  const render = fixture(t, request => {
    if (request.action === 'mount') return { actions: [
      { id: 'action-1', alias: 'total' }, { id: 'action-2', alias: 'total' },
    ] };
    if (request.action === 'result') return { actions: [] };
    return frame;
  }, options);
  assert.equal((await render.mount()).png, frame.png);
  assert.deepEqual(render.requests.map(({ action, actionsOnly, deferFrame }) => [action, !!actionsOnly, !!deferFrame]), [
    ['mount', false, true], ['result', true, false], ['result', true, false], ['frame', false, true],
  ]);
});

test('a transient cleanup error preserves the worker handle and retries without over-admitting', async t => {
  let attempts = 0;
  const render = fixture(t, frame, { capacity: 1, stopWorker: async () => {
    if (++attempts === 1) throw new Error('temporary fixture failure');
  } });
  const mounted = await render.mount();
  await assert.rejects(render.run(owner, { action: 'close', session: mounted.session }), { status: 503 });
  await assert.rejects(render.mount(), { status: 429 });
  render.advance(5000);
  await assert.rejects(render.mount(), { status: 429 });
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(attempts, 2);
  assert.ok((await render.mount()).session);
});

test('renderer actions beyond a batch are retained for the next request', async t => {
  const action = id => ({ id: 'action-' + id, alias: 'total' });
  let deferred = [];
  const settled = [];
  const render = fixture(t, request => {
    if (request.action === 'mount') return { actions: [action(1)] };
    if (request.action === 'result') {
      settled.push(request.id);
      const id = Number(request.id.slice('action-'.length));
      return { actions: id < 33 ? [action(id + 1)] : [] };
    }
    if (request.captureOnly) {
      deferred.push(...(request.deferredActions ?? []));
      return frame;
    }
    if (!deferred.length) return frame;
    const actions = deferred;
    deferred = [];
    return { actions };
  }, { actions: { total: { mode: 'latestRecord' } } });
  const { session, moreActions } = await render.mount();
  assert.equal(moreActions, true);
  assert.equal(settled.length, 32);
  await render.run(owner, { action: 'refresh', session });
  assert.equal(settled.at(-1), 'action-33');
  assert.equal(settled.length, 33);
});
