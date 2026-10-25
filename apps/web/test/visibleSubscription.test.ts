import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createServer } from 'node:http';
import { setTimeout as delay } from 'node:timers/promises';
import { RtdbClient } from '@lingxi/bridge-client/remote';
import { subscribeWhileVisible } from '../src/remote/visibleSubscription';

class Visibility {
  constructor(public visibilityState: string) {}
  listeners = new Set<() => void>();
  addEventListener(_type: 'visibilitychange', listener: () => void): void { this.listeners.add(listener); }
  removeEventListener(_type: 'visibilitychange', listener: () => void): void { this.listeners.delete(listener); }
  change(state: string): void { this.visibilityState = state; for (const listener of this.listeners) listener(); }
}

test('hidden host lists release SSE and resume one fresh subscription when visible', () => {
  const visibility = new Visibility('hidden');
  let started = 0, closed = 0;
  const dispose = subscribeWhileVisible(visibility, () => { started++; return { close() { closed++; } }; });
  assert.equal(started, 0);
  visibility.change('visible');
  visibility.change('visible');
  assert.equal(started, 1);
  visibility.change('hidden');
  visibility.change('hidden');
  assert.equal(closed, 1);
  visibility.change('visible');
  assert.equal(started, 2);
  dispose();
  dispose();
  assert.equal(closed, 2);
  assert.equal(visibility.listeners.size, 0);
  visibility.change('visible');
  assert.equal(started, 2);
});

test('unmount while hidden never starts a subscription afterwards', () => {
  const visibility = new Visibility('hidden');
  const dispose = subscribeWhileVisible(visibility, () => { throw new Error('unexpected stream'); });
  const lateEvent = [...visibility.listeners][0]!;
  dispose();
  visibility.change('visible');
  lateEvent();
  assert.equal(visibility.listeners.size, 0);
});

test('hiding multiple presence tabs closes their real HTTP SSE streams and resuming opens fresh streams', { timeout: 10000 }, async () => {
  let active = 0, opened = 0, events = 0;
  const server = createServer((_request, response) => {
    active++;
    opened++;
    response.writeHead(200, { 'content-type': 'text/event-stream' });
    response.write('event: put\ndata: {"path":"/","data":{}}\n\n');
    response.once('close', () => { active--; });
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  assert.ok(address && typeof address !== 'string');
  const client = new RtdbClient({ databaseUrl: `http://127.0.0.1:${address.port}`, getIdToken: async () => 'test-only' });
  const tabs = Array.from({ length: 3 }, () => new Visibility('visible'));
  const dispose: Array<() => void> = [];
  const until = async (condition: () => boolean): Promise<void> => {
    const deadline = Date.now() + 3000;
    while (!condition()) {
      assert.ok(Date.now() < deadline, `streams active=${active}, opened=${opened}, events=${events}`);
      await delay(10);
    }
  };
  try {
    for (const tab of tabs) dispose.push(subscribeWhileVisible(tab, () => client.listen('hosts', () => { events++; })));
    await until(() => active === 3 && events === 3);
    for (const tab of tabs) tab.change('hidden');
    await until(() => active === 0);
    tabs[0]!.change('visible');
    await until(() => active === 1 && events === 4);
    assert.equal(opened, 4);
    for (const close of dispose) close();
    await until(() => active === 0);
  } finally {
    for (const close of dispose) close();
    server.closeAllConnections();
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
});
