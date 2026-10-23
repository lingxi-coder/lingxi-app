/**
 * The in-memory Realtime Database and the event mirror: the two pieces the
 * host dispatcher and every other test of the remote stack stand on.
 */

import assert from 'node:assert/strict';
import { test } from 'node:test';

import { ManualClock } from '../src/remote/clock.js';
import { MemoryRtdb } from '../src/remote/memoryRtdb.js';
import { SERVER_TIMESTAMP, type RtdbEvent } from '../src/remote/rtdb.js';
import { RtdbMirror, type RtdbChange } from '../src/remote/rtdbMirror.js';
import { settle } from './remote-helpers.js';

function watch(db: MemoryRtdb, path: string, onEvent?: (event: RtdbEvent) => void) {
  const events: RtdbEvent[] = [];
  const subscription = db.listen(path, (event) => {
    events.push(event);
    onEvent?.(event);
  });
  return { events, subscription };
}

// ── MemoryRtdb: data ─────────────────────────────────────────────────────────

test('MemoryRtdb stores, merges and deletes like RTDB', async () => {
  const db = new MemoryRtdb();
  assert.equal(await db.get('/a'), null);
  await db.put('/a', { x: 1, y: { z: 2 } });
  assert.deepEqual(await db.get('/a'), { x: 1, y: { z: 2 } });
  assert.equal(await db.get('/a/y/z'), 2);
  assert.equal(await db.get('/a/missing'), null);

  await db.put('/a', { w: 0 });
  assert.deepEqual(await db.get('/a'), { w: 0 }, 'put replaces');

  await db.patch('/a', { x: 1, 'y/z': 2, w: null });
  assert.deepEqual(await db.get('/a'), { x: 1, y: { z: 2 } }, 'patch merges, multi-segment keys descend, null deletes');

  await db.remove('/a/y');
  assert.deepEqual(db.read('/a'), { x: 1 });
  await db.remove('/a');
  assert.equal(db.read(), null);
});

test('MemoryRtdb never keeps null or empty containers, and turns arrays into index-keyed objects', async () => {
  const db = new MemoryRtdb();
  await db.put('/a', {});
  assert.equal(db.read(), null);
  await db.put('/a/b/c', null);
  assert.equal(db.read(), null);
  await db.put('/a', { b: { c: null }, d: null, e: [] });
  assert.equal(db.read(), null);
  await db.put('/x', ['a', null, 'c']);
  assert.deepEqual(db.read('/x'), { 0: 'a', 2: 'c' });
  await db.put('/leaf', 'text');
  await db.put('/leaf/child', 1);
  assert.deepEqual(db.read('/leaf'), { child: 1 }, 'writing below a leaf replaces the leaf');
});

test('MemoryRtdb returns copies, so callers cannot edit the database by accident', async () => {
  const db = new MemoryRtdb();
  const value = { a: { b: 1 } };
  await db.put('/v', value);
  value.a.b = 2;
  const read = (await db.get('/v')) as typeof value;
  read.a.b = 3;
  assert.deepEqual(db.read('/v'), { a: { b: 1 } });
});

test('MemoryRtdb refuses undefined and non-object patches', async () => {
  const db = new MemoryRtdb();
  await assert.rejects(db.put('/a', undefined), TypeError);
  await assert.rejects(db.patch('/a', null as never), TypeError);
  await assert.rejects(db.patch('/a', ['x'] as never), TypeError);
  assert.equal(db.read(), null);
});

test('SERVER_TIMESTAMP is replaced by the clock\'s time on put and patch, at any depth', async () => {
  const clock = new ManualClock(1_700_000_000_000);
  const db = new MemoryRtdb({ clock });
  await db.put('/p', { created_at: SERVER_TIMESTAMP, nested: { t: SERVER_TIMESTAMP }, list: [SERVER_TIMESTAMP] });
  assert.deepEqual(db.read('/p'), { created_at: 1_700_000_000_000, nested: { t: 1_700_000_000_000 }, list: { 0: 1_700_000_000_000 } });
  clock.advance(5000);
  await db.patch('/p', { last_seen: SERVER_TIMESTAMP, 'nested/t': SERVER_TIMESTAMP });
  assert.deepEqual(db.read('/p/last_seen'), 1_700_000_005_000);
  assert.deepEqual(db.read('/p/nested/t'), 1_700_000_005_000);
  assert.equal(db.read('/p/created_at'), 1_700_000_000_000);
  assert.deepEqual(SERVER_TIMESTAMP, { '.sv': 'timestamp' }, 'the placeholder itself is untouched');
  await assert.rejects(db.put('/q', { '.sv': 'increment' }), TypeError);
  await assert.rejects(db.put('/q', { '.sv': 'timestamp', extra: 1 }), TypeError);
});

test('coerceArrays hands out integer-keyed objects as arrays when the largest key is below twice the number of keys', async () => {
  const db = new MemoryRtdb({ coerceArrays: true });
  await db.put('/dense', { 0: 'a', 1: 'b', 2: 'c' });
  await db.put('/holes', { 0: 'a', 2: 'c' });
  await db.put('/sparse', { 0: 'a', 5: 'b' });
  await db.put('/half', { 0: 'a', 3: 'b' });
  await db.put('/edge', { 0: 'a', 4: 'b' });
  await db.put('/single0', { 0: 'a' });
  await db.put('/single1', { 1: 'a' });
  await db.put('/single2', { 2: 'a' });
  await db.put('/leading-zero', { '00': 'a', 1: 'b' });
  await db.put('/prefixed', { f0: 'a', f1: 'b' });
  await db.put('/mixed', { 0: 'a', x: 'b' });
  await db.put('/nested', { list: { 0: { 0: 'p', 1: 'q' }, 1: 'r' } });
  assert.deepEqual(db.read('/dense'), ['a', 'b', 'c']);
  assert.deepEqual(db.read('/holes'), ['a', null, 'c']);
  assert.deepEqual(db.read('/sparse'), { 0: 'a', 5: 'b' });
  assert.deepEqual(db.read('/half'), ['a', null, null, 'b'], 'the largest key 3 is below 2 * 2 keys (what the real emulator does, too)');
  assert.deepEqual(db.read('/edge'), { 0: 'a', 4: 'b' }, 'the largest key 4 is not below 2 * 2 keys');
  assert.deepEqual(db.read('/single0'), ['a']);
  assert.deepEqual(db.read('/single1'), [null, 'a']);
  assert.deepEqual(db.read('/single2'), { 2: 'a' });
  assert.deepEqual(db.read('/leading-zero'), { '00': 'a', 1: 'b' }, 'a key with a leading zero is not an array index');
  assert.deepEqual(db.read('/prefixed'), { f0: 'a', f1: 'b' }, 'the f prefix is what keeps tunnel frames objects');
  assert.deepEqual(db.read('/mixed'), { 0: 'a', x: 'b' });
  assert.deepEqual(db.read('/nested'), { list: [['p', 'q'], 'r'] });
});

// ── MemoryRtdb: listeners ────────────────────────────────────────────────────

test('listen starts with a put at / carrying the current subtree, delivered asynchronously', async () => {
  const db = new MemoryRtdb();
  const empty = watch(db, '/box');
  assert.equal(empty.events.length, 0, 'not synchronously');
  await settle();
  assert.deepEqual(empty.events, [{ type: 'put', path: '/', data: null }]);

  await db.put('/box', { a: 1 });
  const late = watch(db, '/box');
  await settle();
  assert.deepEqual(late.events, [{ type: 'put', path: '/', data: { a: 1 } }]);
});

test('writes below the listened path arrive as put or patch events with relative paths', async () => {
  const db = new MemoryRtdb();
  const { events } = watch(db, '/box');
  await settle();
  events.length = 0;

  await db.put('/box/c1/offer', { n: 1 });
  await db.patch('/box/c1', { 'up/f0': 'x', 'up/f1': 'y' });
  await db.remove('/box/c1/offer');
  await db.put('/box/c1/up/f0', null);
  await settle();
  assert.deepEqual(events, [
    { type: 'put', path: '/c1/offer', data: { n: 1 } },
    { type: 'patch', path: '/c1', data: { 'up/f0': 'x', 'up/f1': 'y' } },
    { type: 'put', path: '/c1/offer', data: null },
    { type: 'put', path: '/c1/up/f0', data: null },
  ]);
});

test('a patch event carries only the children that changed', async () => {
  const db = new MemoryRtdb();
  await db.put('/box/c1', { up: { f0: 'x' }, keep: 1 });
  const { events } = watch(db, '/box');
  await settle();
  events.length = 0;
  await db.patch('/box/c1', { 'up/f0': 'x', keep: 1, fresh: 2, gone: null });
  await settle();
  assert.deepEqual(events, [{ type: 'patch', path: '/c1', data: { fresh: 2 } }]);
});

test('writes that change nothing produce no event', async () => {
  const db = new MemoryRtdb();
  await db.put('/box/a', 1);
  const { events } = watch(db, '/box');
  await settle();
  events.length = 0;
  await db.put('/box/a', 1);
  await db.remove('/box/missing');
  await db.patch('/box', { a: 1 });
  await db.put('/elsewhere', 5);
  await settle();
  assert.deepEqual(events, []);
});

test('a write above the listened path arrives as a put at / with the new subtree, when it changed', async () => {
  const db = new MemoryRtdb();
  const { events } = watch(db, '/a/b');
  await settle();
  events.length = 0;
  await db.put('/a', { b: { c: 1 } });
  await db.put('/a', { b: { c: 1 }, other: 2 });
  await db.patch('/', { 'a/b/c': 2 });
  await db.put('/a', { other: 2 });
  await db.remove('/');
  await settle();
  assert.deepEqual(events, [
    { type: 'put', path: '/', data: { c: 1 } },
    { type: 'put', path: '/', data: { c: 2 } },
    { type: 'put', path: '/', data: null },
  ]);
});

test('every listener gets its own view, in write order, and a closed one gets nothing more', async () => {
  const db = new MemoryRtdb();
  const wide = watch(db, '/box');
  const narrow = watch(db, '/box/c1');
  assert.equal(db.listenerCount, 2);
  await settle();
  wide.events.length = 0;
  narrow.events.length = 0;

  await db.put('/box/c1/a', 1);
  await db.put('/box/c2/a', 2);
  await db.put('/box/c1/b', 3);
  narrow.subscription.close();
  await db.put('/box/c1/c', 4);
  await settle();
  assert.equal(db.listenerCount, 1);
  assert.deepEqual(wide.events.map((e) => e.path), ['/c1/a', '/c2/a', '/c1/b', '/c1/c']);
  assert.deepEqual(narrow.events.map((e) => e.path), ['/a', '/b'], 'nothing arrives after close()');
});

test('close() is idempotent and drops events that were queued but not yet delivered', async () => {
  const db = new MemoryRtdb();
  const w = watch(db, '/box');
  await settle();
  w.events.length = 0;
  const written = db.put('/box/a', 1); // the event is queued here, delivery waits for the next microtask
  w.subscription.close();
  w.subscription.close();
  await written;
  await settle();
  assert.deepEqual(w.events, []);
  assert.equal(db.listenerCount, 0);
});

test('a handler that throws does not stop later events', async () => {
  const db = new MemoryRtdb();
  let calls = 0;
  const { events } = watch(db, '/box', () => {
    if (++calls === 1) throw new Error('handler bug');
  });
  await settle();
  await db.put('/box/a', 1);
  await settle();
  assert.equal(events.length, 2);
});

test('reconnect() replays the whole subtree as a put at /, like a dropped and reopened stream', async () => {
  const db = new MemoryRtdb();
  await db.put('/box', { c1: { up: { f0: 'x' } } });
  const { events, subscription } = watch(db, '/box');
  await settle();
  subscription.reconnect();
  await settle();
  assert.deepEqual(events, [
    { type: 'put', path: '/', data: { c1: { up: { f0: 'x' } } } },
    { type: 'put', path: '/', data: { c1: { up: { f0: 'x' } } } },
  ]);
  subscription.close();
  subscription.reconnect();
  await settle();
  assert.equal(events.length, 2);
});

// ── RtdbMirror ───────────────────────────────────────────────────────────────

const paths = (changes: RtdbChange[]): string[] => changes.map((change) => `${change.kind} ${change.path.join('/')}`);

test('RtdbMirror reports changes at any depth below the root', () => {
  const mirror = new RtdbMirror();
  const first = mirror.apply({ type: 'put', path: '/', data: { c1: { offer: { a: 1 }, up: { f0: 'x' } } } });
  assert.equal(first.empty, false);
  assert.deepEqual(paths(first.at(1)), ['added c1']);
  assert.deepEqual(paths(first.at(2)), ['added c1/offer', 'added c1/up']);
  assert.deepEqual(paths(first.at(3)), ['added c1/offer/a', 'added c1/up/f0']);
  assert.deepEqual(first.at(3)[1], { path: ['c1', 'up', 'f0'], kind: 'added', value: 'x', previous: null });
  assert.deepEqual(mirror.get(), { c1: { offer: { a: 1 }, up: { f0: 'x' } } });
  assert.deepEqual(mirror.get('/c1/up'), { f0: 'x' });
  assert.equal(mirror.get('/c1/nothing'), null);
});

test('a reconnect\'s repeated snapshot reports only what is genuinely new', () => {
  const mirror = new RtdbMirror();
  mirror.apply({ type: 'put', path: '/', data: { c1: { up: { f0: 'x' } } } });

  const same = mirror.apply({ type: 'put', path: '/', data: { c1: { up: { f0: 'x' } } } });
  assert.equal(same.empty, true);
  for (const depth of [1, 2, 3]) assert.deepEqual(same.at(depth), []);

  const grown = mirror.apply({ type: 'put', path: '/', data: { c1: { up: { f0: 'x', f1: 'y' } }, c2: { offer: { a: 1 } } } });
  assert.equal(grown.empty, false);
  assert.deepEqual(paths(grown.at(1)), ['changed c1', 'added c2']);
  assert.deepEqual(paths(grown.at(3)), ['added c1/up/f1', 'added c2/offer/a']);

  const shrunk = mirror.apply({ type: 'put', path: '/', data: { c2: { offer: { a: 1 } } } });
  assert.deepEqual(paths(shrunk.at(1)), ['removed c1']);
  assert.deepEqual(paths(shrunk.at(3)), ['removed c1/up/f0', 'removed c1/up/f1']);
  assert.deepEqual(shrunk.at(1)[0]!.previous, { up: { f0: 'x', f1: 'y' } });
});

test('RtdbMirror applies puts and patches at relative paths, with null deleting and empty parents vanishing', () => {
  const mirror = new RtdbMirror();
  mirror.apply({ type: 'put', path: '/', data: { c1: { offer: { a: 1 }, up: { f0: 'x' } } } });

  const patched = mirror.apply({ type: 'patch', path: '/c1', data: { 'up/f2': 'z', offer: null } });
  assert.deepEqual(paths(patched.at(3)), ['added c1/up/f2', 'removed c1/offer/a']);
  assert.deepEqual(mirror.get(), { c1: { up: { f0: 'x', f2: 'z' } } });

  const changed = mirror.apply({ type: 'put', path: '/c1/up/f0', data: 'y' });
  assert.deepEqual(changed.at(3), [{ path: ['c1', 'up', 'f0'], kind: 'changed', value: 'y', previous: 'x' }]);

  mirror.apply({ type: 'put', path: '/c1/up/f0', data: null });
  mirror.apply({ type: 'put', path: '/c1/up/f2', data: null });
  assert.equal(mirror.get(), null, 'the emptied branch is gone, not an empty object');

  const removedWhole = new RtdbMirror();
  removedWhole.apply({ type: 'put', path: '/', data: { c1: { up: { f0: 'x' } } } });
  const gone = removedWhole.apply({ type: 'put', path: '/c1', data: null });
  assert.deepEqual(paths(gone.at(1)), ['removed c1']);
  assert.deepEqual(paths(gone.at(3)), ['removed c1/up/f0']);
});

test('RtdbMirror normalizes arrays to index-keyed objects', () => {
  const mirror = new RtdbMirror();
  const changes = mirror.apply({ type: 'put', path: '/', data: { c1: { up: ['a', null, 'c'] } } });
  assert.deepEqual(paths(changes.at(3)), ['added c1/up/0', 'added c1/up/2']);
  assert.deepEqual(mirror.get('/c1/up'), { 0: 'a', 2: 'c' });
});

test('RtdbMirror ignores malformed events and keeps its state', () => {
  const mirror = new RtdbMirror();
  mirror.apply({ type: 'put', path: '/', data: { a: 1 } });

  let bomb: unknown = 1;
  for (let i = 0; i < 100; i++) bomb = { n: bomb };
  for (const event of [
    { type: 'put', path: 7, data: 1 },
    { type: 'patch', path: '/', data: 'not an object' },
    { type: 'patch', path: '/', data: ['array'] },
    { type: 'put', path: '/deep', data: bomb },
  ] as unknown as RtdbEvent[]) {
    const changes = mirror.apply(event);
    assert.equal(changes.empty, true);
    assert.deepEqual(changes.at(1), []);
  }
  assert.deepEqual(mirror.get(), { a: 1 });
});

test('RtdbMirror keeps hostile keys inert', () => {
  const mirror = new RtdbMirror();
  const data = JSON.parse('{"__proto__":{"polluted":true},"constructor":{"x":1},"c1":{"__proto__":2}}') as unknown;
  mirror.apply({ type: 'put', path: '/', data });
  const read = mirror.get() as Record<string, unknown>;
  assert.deepEqual(Object.getOwnPropertyNames(read).sort(), ['__proto__', 'c1', 'constructor']);
  assert.equal(({} as Record<string, unknown>)['polluted'], undefined);
  assert.equal(Object.getPrototypeOf(read), Object.prototype);
  assert.deepEqual(paths(mirror.apply({ type: 'put', path: '/', data }).at(1)), []);
});

test('RtdbMirror.at validates its depth, and clear() forgets everything', () => {
  const mirror = new RtdbMirror();
  const changes = mirror.apply({ type: 'put', path: '/', data: { a: 1 } });
  for (const depth of [0, -1, 1.5, Number.NaN]) assert.throws(() => changes.at(depth), RangeError, String(depth));
  mirror.clear();
  assert.equal(mirror.get(), null);
  assert.deepEqual(paths(mirror.apply({ type: 'put', path: '/', data: { a: 1 } }).at(1)), ['added a']);
});

test('a MemoryRtdb listener feeding an RtdbMirror reports each connection\'s new frames exactly once, even across reconnects', async () => {
  const db = new MemoryRtdb();
  const mirror = new RtdbMirror();
  const seen: string[] = [];
  const subscription = db.listen('/to_host/h1', (event) => {
    for (const change of mirror.apply(event).at(3)) {
      if (change.path[1] === 'up' && change.kind !== 'removed') seen.push(`${change.path[0]}:${change.path[2]}`);
    }
  });
  await settle();
  await db.put('/to_host/h1/c1/up/f0', 'a');
  await db.put('/to_host/h1/c1/up/f1', 'b');
  await db.put('/to_host/h1/c2/up/f0', 'c');
  await settle();
  subscription.reconnect();
  await settle();
  await db.put('/to_host/h1/c1/up/f2', 'd');
  await db.patch('/to_host/h1/c1/up', { f0: null, f1: null });
  await settle();
  subscription.reconnect();
  await settle();
  assert.deepEqual(seen, ['c1:f0', 'c1:f1', 'c2:f0', 'c1:f2']);
});
