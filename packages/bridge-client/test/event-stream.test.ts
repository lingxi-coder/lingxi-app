import assert from 'node:assert/strict';
import { test } from 'node:test';
import { BridgeClient } from '../src/client.js';

function internals(client: BridgeClient) {
  return client as unknown as {
    eventQueue: unknown[];
    onMessage(data: Buffer): void;
    onClose(code: number, reason: string): void;
  };
}

function deliver(client: BridgeClient, text: string) {
  internals(client).onMessage(Buffer.from(JSON.stringify({
    type: 'event', payload: { type: 'text_delta', text },
  })));
}

test('emitter-only consumers do not retain a second copy of long event feeds', () => {
  const client = new BridgeClient();
  let received = 0;
  client.on('event', () => { received += 1; });
  for (let index = 0; index < 10_000; index += 1) deliver(client, String(index));
  assert.equal(received, 10_000);
  assert.equal(internals(client).eventQueue.length, 0);
});

test('events subscribes immediately and buffers ordered events before next', async () => {
  const client = new BridgeClient();
  const events = client.events();
  deliver(client, 'first');
  deliver(client, 'second');
  assert.deepEqual(await events.next(), { value: { type: 'text_delta', text: 'first' }, done: false });
  assert.deepEqual(await events.next(), { value: { type: 'text_delta', text: 'second' }, done: false });
  await events.return!();
});

test('return releases backlog, settles parked reads and stops future buffering', async () => {
  const client = new BridgeClient();
  const events = client.events();
  const parked = events.next();
  await events.return!();
  assert.deepEqual(await parked, { value: undefined, done: true });
  assert.deepEqual(await events.next(), { value: undefined, done: true });
  deliver(client, 'after return');
  assert.equal(internals(client).eventQueue.length, 0);

  const another = client.events();
  deliver(client, 'unused backlog');
  await another.return!();
  assert.equal(internals(client).eventQueue.length, 0);
});

test('returning one iterator does not terminate another subscriber', async () => {
  const client = new BridgeClient();
  const first = client.events();
  const second = client.events();
  const firstRead = first.next();
  const secondRead = second.next();
  await first.return!();
  deliver(client, 'remaining subscriber');
  assert.deepEqual(await firstRead, { value: undefined, done: true });
  assert.deepEqual(await secondRead, { value: { type: 'text_delta', text: 'remaining subscriber' }, done: false });
  await second.return!();
});

test('connection close permits draining subscribed events then finishes', async () => {
  const client = new BridgeClient();
  const events = client.events();
  deliver(client, 'before close');
  internals(client).onClose(1000, 'closed');
  assert.deepEqual(await events.next(), { value: { type: 'text_delta', text: 'before close' }, done: false });
  assert.deepEqual(await events.next(), { value: undefined, done: true });
});

test('returning after connection close preserves another subscriber backlog', async () => {
  const client = new BridgeClient();
  const first = client.events();
  const second = client.events();
  deliver(client, 'before close');
  internals(client).onClose(1000, 'closed');
  await first.return!();
  assert.deepEqual(await second.next(), { value: { type: 'text_delta', text: 'before close' }, done: false });
  assert.deepEqual(await second.next(), { value: undefined, done: true });
});

test('late subscriptions receive live events without replaying prior listener events', async () => {
  const client = new BridgeClient();
  deliver(client, 'before subscription');
  const events = client.events();
  const read = events.next();
  deliver(client, 'live');
  assert.deepEqual(await read, { value: { type: 'text_delta', text: 'live' }, done: false });
  await events.return!();
});

test('failed connection settles an iterator subscribed before connecting', { timeout: 1_000 }, async () => {
  const client = new BridgeClient({ lockfilePath: '/private/tmp/lingxi-missing-event-stream-lockfile/never-created.lock' });
  const events = client.events();
  const parked = events.next();
  await assert.rejects(client.connect());
  assert.deepEqual(await parked, { value: undefined, done: true });
  assert.deepEqual(await events.next(), { value: undefined, done: true });
});

test('close without a socket settles a preconnect iterator', async () => {
  const client = new BridgeClient();
  const events = client.events();
  const parked = events.next();
  client.close();
  assert.deepEqual(await parked, { value: undefined, done: true });
});
