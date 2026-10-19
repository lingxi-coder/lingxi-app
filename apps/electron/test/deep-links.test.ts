import test from 'node:test';
import assert from 'node:assert/strict';
import { DeepLinkQueue } from '../src/main/deepLinkQueue';

test('cold delivery remains available across renderer readiness, reload and window recreation until ack', () => {
  const queue = new DeepLinkQueue();
  const first = queue.enqueue('lingxi://session/CaseSensitive');
  assert.ok(first);
  assert.deepEqual(queue.next(), first);
  assert.deepEqual(queue.next(), first);
  assert.equal(queue.acknowledge('stale-renderer-id'), false);
  assert.deepEqual(queue.next(), first);
  assert.equal(queue.acknowledge(first.id), true);
  assert.equal(queue.next(), null);
  assert.equal(queue.hasPending, false);
});

test('OS duplicate delivery canonicalizes scheme and HTTPS without losing FIFO ordering', () => {
  const queue = new DeepLinkQueue();
  const first = queue.enqueue('lingxi://session/a?workspace=project.a&mode=chat')!;
  assert.equal(queue.enqueue('https://lingxios.net/open/session/a?mode=chat&workspace=project.a'), null);
  const second = queue.enqueue('lingxi://session/b')!;
  assert.equal(queue.acknowledge(second.id), false);
  queue.acknowledge(first.id);
  // A completed delivery must never swallow an explicit subsequent click.
  assert.ok(queue.enqueue('lingxi://session/a?workspace=project.a&mode=chat'));
  assert.deepEqual(queue.next(), second);
  queue.acknowledge(second.id);
  assert.equal(queue.next()?.target.route, 'session/a');
});

test('queue limits preserve existing deliveries and reject invalid inputs without mutation', () => {
  const queue = new DeepLinkQueue(1);
  const first = queue.enqueue('lingxi://new')!;
  assert.equal(queue.enqueue('lingxi://new'), null);
  assert.throws(() => queue.enqueue('lingxi://session/a'), /Too many pending/);
  assert.throws(() => queue.enqueue('lingxi://new?unknown=secret'));
  assert.deepEqual(queue.next(), first);
});
