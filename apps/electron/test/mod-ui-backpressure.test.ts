import assert from 'node:assert/strict';
import { test } from 'node:test';
import { EventEmitter } from 'node:events';
import { SessionRuntime } from '../src/main/bridge';
import { ModUiIpcQueue, modUiPayloadBytes } from '../src/shared/modUiIpcQueue';

test('a stalled Mod host cannot retain an unbounded renderer request backlog', async () => {
  const sessionId = '11111111-2222-4333-8444-555555555555';
  const runtime = new SessionRuntime({ sessionId, projectPath: '/fixture', launchConfig: () => ({ workspace: '/fixture', sessionId, trusted: true }) });
  const client = Object.assign(new EventEmitter(), { sendCommand: () => undefined });
  Object.assign(runtime, { client, activeWorkspace: '/fixture', activeWorkspaceTrusted: true, generation: 1 });
  (runtime as any).wireClient(client, 1);
  const pending: Promise<unknown>[] = [];
  try {
    for (let index = 0; index < 1000; index++) {
      pending.push(runtime.dispatchModUiControl({
        subtype: 'ui_render', surface: 'desktop', component: 'ToolUse', instance_id: `tool-${index}`,
        props: { content: `${index}:` + 'x'.repeat(23_000) },
      }).catch(error => error));
    }
    const retained = (runtime as any).queuedModUiRequests.length;
    assert.ok(retained <= 64, `stalled host retained ${retained} complete 23 KB payloads`);
  } finally {
    (runtime as any).clearPendingConnectionOperations();
    await Promise.all(pending);
  }
});

test('preload coalesces obsolete render snapshots before IPC while preserving actions and reply ownership', async () => {
  const queue = new ModUiIpcQueue(1);
  const sent: string[] = [];
  let release!: () => void;
  const first = queue.request('site', {}, () => new Promise<string>(resolve => { sent.push('first'); release = () => resolve('revision-1'); }));
  await new Promise(resolve => setImmediate(resolve));
  const renders = Array.from({ length: 1000 }, (_, index) => queue.request('site', { text: 'x'.repeat(23_000) }, async () => { sent.push(`render-${index}`); return 'revision-2'; }).catch(error => error));
  const action = queue.request(undefined, {}, async () => { sent.push('action'); return 'handled'; });
  assert.deepEqual(sent, ['first']);
  release();
  assert.equal(await first, 'revision-1');
  const results = await Promise.all(renders);
  assert.equal(results.filter(result => result instanceof Error).length, 999);
  assert.equal(results.at(-1), 'revision-2');
  assert.equal(await action, 'handled');
  assert.deepEqual(sent, ['first', 'render-999', 'action']);
});

test('preload bounds total retained bytes and rejects sparse/cyclic payloads before structured cloning', async () => {
  assert.throws(() => modUiPayloadBytes(new Array(2 ** 32 - 1)), /array exceeds/);
  assert.throws(() => modUiPayloadBytes(new Array(2)), /sparse/);
  const circular: any = {}; circular.self = circular;
  assert.throws(() => modUiPayloadBytes(circular), /acyclic/);
  const queue = new ModUiIpcQueue(1, 1500);
  let release!: () => void;
  const first = queue.request(undefined, { text: 'x'.repeat(400) }, () => new Promise<void>(resolve => { release = resolve; }));
  await new Promise(resolve => setImmediate(resolve));
  await assert.rejects(queue.request(undefined, { text: 'x'.repeat(400) }, async () => undefined), /backlog is full/);
  release(); await first;
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(await queue.request(undefined, {}, async () => 'recovered'), 'recovered');
});

test('a rejected IPC call releases capacity and does not drop a queued user action', async () => {
  const queue = new ModUiIpcQueue(1);
  let reject!: (error: Error) => void;
  const failed = queue.request('render', {}, () => new Promise<void>((_resolve, fail) => { reject = fail; }));
  const failure = assert.rejects(failed, /worker stopped/);
  const action = queue.request(undefined, {}, async () => 'action accepted');
  await new Promise(resolve => setImmediate(resolve));
  reject(new Error('worker stopped'));
  await failure;
  assert.equal(await action, 'action accepted');
});
