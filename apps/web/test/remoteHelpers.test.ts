import assert from 'node:assert/strict';
import test from 'node:test';
import { ManualClock } from '@lingxi/bridge-client/remote';
import { ImageRejected, MAX_UPLOAD_EDGE, fitWithin, nextAttempt, shrinkImage } from '../src/remote/imageShrink';
import { permissionFields } from '../src/remote/permissionDetail';
import { safeNext } from '../src/remote/signInNext';
import { DEVICE_NAME_MAX_CHARS, deviceNameFromUserAgent, displayText, formatPairingCode } from '../src/remote/text';
import { TUNNEL_MEMORY_STORAGE_KEY, TunnelMemory } from '../src/remote/tunnelMemory';
import type { StorageLike } from '../src/remote/env';

test('displayText removes direction controls and invisible characters', () => {
  assert.equal(displayText('abc‮def⁦ghi​jkl'), 'abcdefghijkl');
  assert.equal(displayText('line\none\r\ntwo three\u0007'), 'line one two three');
  assert.equal(displayText('  many   spaces\t here  '), 'many spaces here');
});

test('displayText cuts by code point and marks the cut', () => {
  const clipped = displayText('a'.repeat(100), 10);
  assert.equal(Array.from(clipped).length, 10);
  assert.ok(clipped.endsWith('…'));
  const emoji = displayText('\u{1F600}'.repeat(20), 5);
  assert.equal(Array.from(emoji).length, 5); // no lone surrogate
  assert.equal(displayText(42), '');
  assert.equal(displayText(undefined), '');
});

test('device names name the browser and the system, in at most 64 characters', () => {
  const cases: Array<[string, string, string?]> = [
    ['Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36', 'Chrome · macOS'],
    ['Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36 Edg/124.0.0.0', 'Edge · Windows'],
    ['Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Mobile/15E148 Safari/604.1', 'Safari · iOS'],
    ['Mozilla/5.0 (X11; Linux x86_64; rv:125.0) Gecko/20100101 Firefox/125.0', 'Firefox · Linux'],
    ['Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Mobile Safari/537.36', 'Chrome · Android'],
    ['something else entirely', 'Browser · Web'],
    ['something else entirely', 'Browser · Plan9', 'Plan9'],
  ];
  for (const [ua, expected, hint] of cases) assert.equal(deviceNameFromUserAgent(ua, hint), expected);
  assert.ok(Array.from(deviceNameFromUserAgent('x'.repeat(500))).length <= DEVICE_NAME_MAX_CHARS);
});

test('pairing codes are grouped in threes', () => {
  assert.equal(formatPairingCode('123456'), '123 456');
  assert.equal(formatPairingCode('12'), '12');
});

test('permission details show the whole payload, commands first, with secrets masked', () => {
  const fields = permissionFields(JSON.stringify({ description: 'list files', env: { API_KEY: 'x' }, command: 'curl -H "Authorization: Bearer abc.def" https://example.com' }));
  assert.deepEqual(fields.map((field) => field.key), ['command', 'description', 'env']);
  assert.ok(fields[0]!.value.includes('Bearer [REDACTED]'));
  assert.ok(!fields[0]!.value.includes('abc.def'));
});

test('permission details cope with input that is not an object, and announce a clip', () => {
  assert.deepEqual(permissionFields('not json'), [{ key: '', value: 'not json' }]);
  assert.deepEqual(permissionFields('[1,2]').map((field) => field.key), ['']);
  const [long] = permissionFields(JSON.stringify({ command: 'a'.repeat(25_000) }));
  assert.ok(long!.value.includes('5000 more characters not shown'));
});

test('image sizing never enlarges and keeps the aspect ratio', () => {
  assert.deepEqual(fitWithin(800, 600, MAX_UPLOAD_EDGE), { width: 800, height: 600 });
  assert.deepEqual(fitWithin(3136, 1568, MAX_UPLOAD_EDGE), { width: 1568, height: 784 });
  assert.deepEqual(fitWithin(1, 4000, MAX_UPLOAD_EDGE), { width: 1, height: 1568 });
});

test('a picture that is still too big gets lower quality first, then a smaller size', () => {
  let attempt = { quality: 0.9, scale: 1 };
  const qualities: number[] = [];
  for (let i = 0; i < 5; i++) {
    qualities.push(attempt.quality);
    attempt = nextAttempt(attempt.quality, attempt.scale);
  }
  assert.deepEqual(qualities, [0.9, 0.75, 0.6, 0.45, 0.8]);
  assert.equal(attempt.scale, 0.75);
});

test('a file of an unsupported type is rejected before anything is decoded', async () => {
  await assert.rejects(shrinkImage(new Blob(['x'], { type: 'image/svg+xml' })), (error: unknown) => error instanceof ImageRejected && error.reason === 'type');
});

test('after sign-in only a Remote Control page of this site is a valid destination', () => {
  const origin = 'https://lingxi.example';
  assert.equal(safeNext('?next=%2Fremote%2Fhost%2Fsession', origin), '/remote/host/session');
  assert.equal(safeNext('?next=%2Fremote', origin), '/remote');
  for (const bad of ['', '?next=', '?next=%2F%2Fevil.example%2Fremote', '?next=https%3A%2F%2Fevil.example%2Fremote', '?next=%2Fadmin%2Fmodels', '?next=%2Fremotely', '?next=%5C%5Cevil.example', '?next=javascript%3Aalert(1)']) {
    assert.equal(safeNext(bad, origin), '/remote', bad);
  }
});

function memoryStorage(initial: Record<string, string> = {}): StorageLike & { data: Map<string, string> } {
  const data = new Map(Object.entries(initial));
  return {
    data,
    getItem: (key) => data.get(key) ?? null,
    setItem: (key, value) => void data.set(key, value),
    removeItem: (key) => void data.delete(key),
  };
}

test('a computer that needed the tunnel is dialled through it for ten minutes', () => {
  const clock = new ManualClock(1_000);
  const memory = new TunnelMemory(clock);
  assert.equal(memory.prefers('host'), false);
  memory.remember('host');
  assert.equal(memory.prefers('host'), true);
  assert.equal(memory.prefers('other'), false);
  clock.advance(9 * 60_000);
  assert.equal(memory.prefers('host'), true);
  clock.advance(61_000);
  assert.equal(memory.prefers('host'), false);
  memory.remember('host');
  memory.forget('host');
  assert.equal(memory.prefers('host'), false);
});

test('the tunnel preference survives a reload through session storage, but not once expired', () => {
  const clock = new ManualClock(0);
  const storage = memoryStorage();
  new TunnelMemory(clock, storage).remember('host');
  assert.equal(new TunnelMemory(clock, storage).prefers('host'), true);
  clock.advance(11 * 60_000);
  assert.equal(new TunnelMemory(clock, storage).prefers('host'), false);
});

test('unreadable or refusing storage does not break the tunnel memory', () => {
  const clock = new ManualClock(0);
  const garbage = memoryStorage({ [TUNNEL_MEMORY_STORAGE_KEY]: '{"host": "soon", "x": [1]' });
  assert.equal(new TunnelMemory(clock, garbage).prefers('host'), false);
  const refusing: StorageLike = {
    getItem: () => {
      throw new Error('blocked');
    },
    setItem: () => {
      throw new Error('full');
    },
    removeItem: () => undefined,
  };
  const memory = new TunnelMemory(clock, refusing);
  memory.remember('host');
  assert.equal(memory.prefers('host'), true);
});
