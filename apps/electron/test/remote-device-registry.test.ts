import assert from 'node:assert/strict';
import { mkdtemp, readFile, readdir, rm, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { ManualClock, b64uEncode, randomBytes, randomId } from '@lingxi/bridge-client/remote';

import { DeviceRegistry, type ApprovedDeviceInput } from '../src/main/remoteControl/deviceRegistry.js';
import { MemoryFiles, START, diagnosticsLog } from './remote-helpers.js';

const PATH = '/virtual/remote-control/devices.json';

function device(name = 'Test Phone'): ApprovedDeviceInput {
  return { device_id: randomId(), key: b64uEncode(randomBytes(32)), name, platform: 'ios' };
}

function setup(files = new MemoryFiles(), options: { revokedMemoryMs?: number } = {}) {
  const clock = new ManualClock(START);
  const log = diagnosticsLog();
  const registry = new DeviceRegistry({ path: PATH, files, clock, diagnostics: log.diagnostics, ...options });
  return { clock, log, registry, files };
}

test('a device is approved by its id and key together, per account', () => {
  const { registry, clock } = setup();
  const phone = device();
  assert.equal(registry.isApproved('uid-1', phone.device_id, phone.key), false);

  registry.approve('uid-1', phone);
  assert.equal(registry.isApproved('uid-1', phone.device_id, phone.key), true);
  assert.equal(registry.isApproved('uid-1', phone.device_id, b64uEncode(randomBytes(32))), false, 'another key under the same id');
  assert.equal(registry.isApproved('uid-2', phone.device_id, phone.key), false, 'another account');
  assert.deepEqual(registry.list('uid-2'), []);

  assert.deepEqual(registry.list('uid-1'), [{ ...phone, approved_at: START, last_seen: START }]);

  // Approving again refreshes the name and last_seen, and keeps the first approval time.
  clock.advance(60_000);
  registry.approve('uid-1', { ...phone, name: 'Renamed Phone' });
  assert.deepEqual(registry.list('uid-1'), [{ ...phone, name: 'Renamed Phone', approved_at: START, last_seen: START + 60_000 }]);
});

test('devices are listed most recently seen first, and touch only concerns known devices', () => {
  const { registry, clock, files } = setup();
  const older = device('older');
  const newer = device('newer');
  registry.approve('uid-1', older);
  clock.advance(1000);
  registry.approve('uid-1', newer);
  assert.deepEqual(registry.list('uid-1').map((entry) => entry.name), ['newer', 'older']);

  clock.advance(1000);
  registry.touch('uid-1', older.device_id);
  assert.deepEqual(registry.list('uid-1').map((entry) => entry.name), ['older', 'newer']);
  assert.equal(registry.list('uid-1')[0]!.last_seen, START + 2000);

  const writes = files.log.filter((line) => line.startsWith('write')).length;
  registry.touch('uid-1', randomId());
  registry.touch('uid-2', older.device_id);
  assert.equal(files.log.filter((line) => line.startsWith('write')).length, writes, 'an unknown device costs no write');
});

test('the list is a copy: editing it changes nothing', () => {
  const { registry } = setup();
  registry.approve('uid-1', device());
  registry.list('uid-1')[0]!.name = 'tampered';
  assert.equal(registry.list('uid-1')[0]!.name, 'Test Phone');
});

test('approved devices survive a restart, in a file only the user can read', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'lingxi-devices-'));
  try {
    const path = join(directory, 'remote-control', 'devices.json');
    const first = new DeviceRegistry({ path });
    const phone = device();
    first.approve('uid-1', phone);

    const second = new DeviceRegistry({ path });
    assert.equal(second.isApproved('uid-1', phone.device_id, phone.key), true);
    assert.equal(second.list('uid-1')[0]!.name, 'Test Phone');

    const saved = JSON.parse(await readFile(path, 'utf8')) as { version: number; users: Record<string, unknown[]> };
    assert.equal(saved.version, 1);
    assert.deepEqual(Object.keys(saved.users), ['uid-1']);
    assert.equal(saved.users['uid-1']!.length, 1);
    assert.equal((await stat(path)).mode & 0o777, 0o600);
    assert.equal((await stat(join(directory, 'remote-control'))).mode & 0o777, 0o700);
    assert.deepEqual(await readdir(join(directory, 'remote-control')), ['devices.json'], 'no temporary file is left behind');

    second.revoke('uid-1', phone.device_id);
    assert.equal(new DeviceRegistry({ path }).isApproved('uid-1', phone.device_id, phone.key), false);
    assert.deepEqual(JSON.parse(await readFile(path, 'utf8')), { version: 1, users: {} });
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test('every save writes a temporary file and renames it over the real one', () => {
  const { registry, files } = setup();
  registry.approve('uid-1', device());
  assert.deepEqual(files.log, [
    'mkdir /virtual/remote-control 700',
    `write ${PATH}.tmp`,
    `rename ${PATH}.tmp ${PATH}`,
  ]);
  assert.equal(files.modes.get(`${PATH}.tmp`), 0o600);
  assert.ok(files.store.has(PATH));
  assert.ok(!files.store.has(`${PATH}.tmp`));
});

test('a failed save leaves the old file whole; an approval still holds until restart', () => {
  const { registry, files, log } = setup();
  const first = device('first');
  registry.approve('uid-1', first);
  const before = files.store.get(PATH);

  files.failWrites = true;
  const second = device('second');
  registry.approve('uid-1', second);
  assert.equal(registry.isApproved('uid-1', second.device_id, second.key), true);
  assert.equal(files.store.get(PATH), before, 'the file on disk is unchanged');
  assert.ok(!files.store.has(`${PATH}.tmp`), 'the half-written temporary file is cleaned up');
  assert.equal(log.messages('warn').length, 1);
  assert.match(log.messages('warn')[0]!, /could not be saved.*disk full/);
});

test('a revocation holds in memory even when it cannot be saved, and the failure is reported', () => {
  const { registry, files } = setup();
  const phone = device();
  registry.approve('uid-1', phone);

  files.failWrites = true;
  assert.throws(() => registry.revoke('uid-1', phone.device_id), /disk full/);
  assert.equal(registry.isApproved('uid-1', phone.device_id, phone.key), false);
  assert.equal(registry.isRevoked('uid-1', phone.device_id), true);
  assert.equal(JSON.parse(files.store.get(PATH)!).users['uid-1'].length, 1, 'the file still lists it');
});

test('a revoked device is refused for ten minutes, unless it is approved again', () => {
  const { registry, clock } = setup();
  const phone = device();
  registry.approve('uid-1', phone);
  assert.equal(registry.revoke('uid-1', phone.device_id), true);
  assert.equal(registry.isRevoked('uid-1', phone.device_id), true);
  assert.equal(registry.isRevoked('uid-2', phone.device_id), false, 'per account');

  clock.advance(10 * 60_000 - 1);
  assert.equal(registry.isRevoked('uid-1', phone.device_id), true);
  clock.advance(1);
  assert.equal(registry.isRevoked('uid-1', phone.device_id), false);

  registry.revoke('uid-1', phone.device_id);
  registry.approve('uid-1', phone);
  assert.equal(registry.isRevoked('uid-1', phone.device_id), false, 'the user approved it again');
  assert.equal(registry.isApproved('uid-1', phone.device_id, phone.key), true);

  // Revoking a device nobody approved is remembered too (the viewer may have a stale entry), but reports it was unknown.
  const stranger = randomId();
  assert.equal(registry.revoke('uid-1', stranger), false);
  assert.equal(registry.isRevoked('uid-1', stranger), true);
});

test('the memory of a revocation can be shortened', () => {
  const { registry, clock } = setup(new MemoryFiles(), { revokedMemoryMs: 1000 });
  const phone = device();
  registry.revoke('uid-1', phone.device_id);
  clock.advance(999);
  assert.equal(registry.isRevoked('uid-1', phone.device_id), true);
  clock.advance(1);
  assert.equal(registry.isRevoked('uid-1', phone.device_id), false);
});

test('listeners hear about every change to an account, and one that throws harms nobody', () => {
  const { registry } = setup();
  const heard: string[] = [];
  const stop = registry.onChange((uid) => heard.push(uid));
  registry.onChange(() => { throw new Error('listener bug'); });
  const phone = device();
  registry.approve('uid-1', phone);
  registry.touch('uid-1', phone.device_id);
  registry.revoke('uid-1', phone.device_id);
  assert.deepEqual(heard, ['uid-1', 'uid-1', 'uid-1']);
  stop();
  registry.approve('uid-2', device());
  assert.equal(heard.length, 3);
});

test('a file that cannot be understood is set aside, not overwritten, and the registry starts empty', () => {
  for (const [label, text] of [
    ['invalid JSON', '{ not json'],
    ['another version', JSON.stringify({ version: 2, users: {} })],
    ['no users', JSON.stringify({ version: 1 })],
    ['an array', '[]'],
    ['too large', ' '.repeat(4 * 1024 * 1024 + 1)],
  ] as const) {
    const files = new MemoryFiles({ [PATH]: text });
    const { registry, log } = setup(files);
    assert.deepEqual(registry.list('uid-1'), [], label);
    assert.ok(!files.store.has(PATH), `${label}: the bad file moved away`);
    assert.equal(files.store.get(`${PATH}.unreadable`), text, `${label}: ...and was kept for inspection`);
    assert.equal(log.messages('warn').length, 1, label);
    assert.match(log.messages('warn')[0]!, /unreadable/);
  }
});

test('a missing file is not a problem, and any other read error is reported', () => {
  const missing = setup();
  assert.deepEqual(missing.log.entries, []);

  const files = new MemoryFiles();
  files.readFileSync = () => { throw Object.assign(new Error('permission denied'), { code: 'EACCES' }); };
  const denied = setup(files);
  assert.equal(denied.log.messages('warn').length, 1);
});

test('entries that do not look like devices are dropped one by one', () => {
  const good = { ...device('good'), approved_at: 1, last_seen: 2 };
  const entries: unknown[] = [
    good,
    { ...good, device_id: 'short' },
    { ...good, device_id: randomId(), key: 'AAAA' },
    { ...good, device_id: randomId(), name: '' },
    { ...good, device_id: randomId(), name: 'x'.repeat(129) },
    { ...good, device_id: randomId(), name: 'bad\nname' },
    { ...good, device_id: randomId(), name: 'bad name' },
    { ...good, device_id: randomId(), platform: 'windows' },
    { ...good, device_id: randomId(), approved_at: -1 },
    { ...good, device_id: randomId(), last_seen: 'yesterday' },
    'not an object',
    null,
  ];
  const files = new MemoryFiles({ [PATH]: JSON.stringify({ version: 1, users: { 'uid-1': entries, 'uid-2': 'nope', 'uid-3': [] } }) });
  const { registry } = setup(files);
  assert.deepEqual(registry.list('uid-1'), [good]);
  assert.deepEqual(registry.list('uid-2'), []);
  assert.deepEqual(registry.list('uid-3'), []);
});

test('a hostile file cannot make the registry hold more than its bounds, and __proto__ is just a name', () => {
  const users: Record<string, unknown[]> = {};
  for (let i = 0; i < 70; i++) users[`uid-${i}`] = [{ ...device(`d${i}`), approved_at: 1, last_seen: 1 }];
  users['uid-0'] = Array.from({ length: 300 }, (_, i) => ({ ...device(`c${i}`), approved_at: 1, last_seen: 1 }));
  // JSON.stringify cannot write an own `__proto__` key, JSON.parse reads one.
  const proto = JSON.stringify([{ ...device('proto'), approved_at: 1, last_seen: 1 }]);
  const text = `{"version":1,"users":{"__proto__":${proto},${JSON.stringify(users).slice(1)}}`;
  const { registry } = setup(new MemoryFiles({ [PATH]: text }));
  assert.equal(registry.list('uid-0').length, 256, 'devices per account are capped');
  const kept = Array.from({ length: 70 }, (_, i) => registry.list(`uid-${i}`).length).filter((count) => count > 0).length;
  assert.equal(kept, 63, 'accounts are capped at 64 in all');
  assert.equal(registry.list('__proto__')[0]?.name, 'proto', 'a plain account like any other');
  assert.equal(({} as Record<string, unknown>)['approved_at'], undefined, 'Object.prototype was not touched');
});
