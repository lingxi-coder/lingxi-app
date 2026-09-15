import { test } from 'node:test';
import assert from 'node:assert/strict';
import { setTimeout as delay } from 'node:timers/promises';
import { TerminalDelivery } from '../src/main/terminal-delivery';
import type { TerminalEvent, TerminalSnapshot } from '../src/shared/terminal';

const snapshot = (sequence = 0, output = ''): TerminalSnapshot => ({ id: 'a', scope: { projectPath: '/project', sessionId: 'chat' }, title: 'project', status: 'running', exitCode: null, sequence, output });

test('terminal delivery pauses upstream and preserves ANSI bytes after a stalled renderer', async () => {
  const events: TerminalEvent[] = [];
  let paused = false;
  const delivery = new TerminalDelivery(event => events.push(event), value => { paused = value; });
  try {
    delivery.watch(snapshot());
    delivery.push({ kind: 'output', terminalId: 'a', sequence: 1, data: 'first' });
    await delay(30);
    assert.equal(events.length, 1);
    let sequence = 1;
    let expected = '';
    while (!paused) { sequence++; const data = '\x1b[31m' + 'x'.repeat(10_000); expected += data; delivery.push({ kind: 'output', terminalId: 'a', sequence, data }); }
    assert.ok(expected.length < 80_000);
    await delay(30);
    assert.equal(events.length, 1, 'IPC stays at one outstanding frame');
    delivery.acknowledge('a', 0);
    await delay(30);
    assert.equal(events.length, 1, 'stale acknowledgement cannot release another frame');
    delivery.acknowledge('a', 1);
    await delay(30);
    assert.deepEqual(events[1], { kind: 'output', terminalId: 'a', sequence, data: expected });
    assert.equal(paused, false);
  } finally { delivery.dispose(); }
});

test('listing another terminal does not drop pending data; snapshot acknowledgements cover older frames', async () => {
  const events: TerminalEvent[] = [];
  const delivery = new TerminalDelivery(event => events.push(event));
  try {
    delivery.watch(snapshot());
    delivery.push({ kind: 'output', terminalId: 'a', sequence: 1, data: 'a' });
    await delay(30);
    delivery.push({ kind: 'output', terminalId: 'a', sequence: 2, data: 'b' });
    delivery.watch(snapshot(2, 'ab'));
    delivery.acknowledge('a', 1);
    await delay(30);
    assert.deepEqual(events[1], { kind: 'output', terminalId: 'a', sequence: 2, data: 'b' });
    delivery.push({ kind: 'output', terminalId: 'a', sequence: 3, data: 'c' });
    delivery.push({ kind: 'output', terminalId: 'a', sequence: 4, data: 'd' });
    delivery.acknowledge('a', 3);
    await delay(30);
    assert.deepEqual(events[2], { kind: 'output', terminalId: 'a', sequence: 4, data: 'd' });
  } finally { delivery.dispose(); }
});

test('unsubscribed windows receive no terminal data and closing clears queued timers', async () => {
  const events: TerminalEvent[] = [];
  const delivery = new TerminalDelivery(event => events.push(event));
  delivery.push({ kind: 'output', terminalId: 'a', sequence: 1, data: 'private' });
  assert.equal(delivery.has('a'), false);
  delivery.watch(snapshot());
  delivery.push({ kind: 'output', terminalId: 'a', sequence: 1, data: 'pending' });
  delivery.push({ kind: 'closed', terminalId: 'a' });
  await delay(30);
  assert.deepEqual(events, [{ kind: 'closed', terminalId: 'a' }]);
  assert.equal(delivery.has('a'), false);
  delivery.dispose();
});
