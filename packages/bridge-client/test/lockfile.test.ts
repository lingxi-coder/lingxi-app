/**
 * Lockfile reader + version-compat unit tests — mirror the Rust
 * `bridge::lockfile` and `bridge::wire::version_compatible` behavior.
 */

import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, writeFileSync, utimesSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

import { discoverLatestLockfile, discoverLatestLockfileAsync, readLockfile, readLockfileAsync, type LockfileBody } from '../src/lockfile.js';
import { CLIENT_PROTOCOL_VERSION } from '../src/protocol.js';
import { versionCompatible } from '../src/version.js';

function writeLock(dir: string, port: number, token: string): string {
  const body: LockfileBody = {
    pid: 1234,
    workspaceFolders: ['/w'],
    ideName: 'LingXi-Bridge',
    transport: 'ws',
    runningInWindows: false,
    authToken: token,
  };
  const path = join(dir, `${port}.lock`);
  writeFileSync(path, JSON.stringify(body));
  return path;
}

test('readLockfile recovers port from filename and reads the token', () => {
  const dir = mkdtempSync(join(tmpdir(), 'bridge-lock-'));
  try {
    const path = writeLock(dir, 54321, 'deadbeef');
    const lf = readLockfile(path);
    assert.equal(lf.port, 54321);
    assert.equal(lf.body.authToken, 'deadbeef');
    assert.equal(lf.body.transport, 'ws');
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('readLockfile rejects a filename without .lock', () => {
  const dir = mkdtempSync(join(tmpdir(), 'bridge-lock-'));
  try {
    const path = join(dir, '40729.json');
    writeFileSync(path, '{"authToken":"x"}');
    assert.throws(() => readLockfile(path), /not <port>\.lock/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('discoverLatestLockfile returns null for an empty / missing dir', () => {
  const dir = mkdtempSync(join(tmpdir(), 'bridge-lock-'));
  try {
    assert.equal(discoverLatestLockfile(dir), null);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
  assert.equal(discoverLatestLockfile(join(tmpdir(), 'does-not-exist-xyz')), null);
});

test('discoverLatestLockfile picks the most-recently-modified file', () => {
  const dir = mkdtempSync(join(tmpdir(), 'bridge-lock-'));
  try {
    const older = writeLock(dir, 40001, 'older');
    const newer = writeLock(dir, 40002, 'newer');
    // Force a clearly-older mtime on the first file.
    const past = Date.now() / 1000 - 60;
    utimesSync(older, past, past);
    const found = discoverLatestLockfile(dir);
    assert.notEqual(found, null);
    assert.equal(found!.port, 40002);
    assert.equal(found!.body.authToken, 'newer');
    assert.ok(newer.endsWith('40002.lock'));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('discoverLatestLockfile ignores .tmp shadows', () => {
  const dir = mkdtempSync(join(tmpdir(), 'bridge-lock-'));
  try {
    writeFileSync(join(dir, '.40729.lock.tmp'), '{}');
    assert.equal(discoverLatestLockfile(dir), null);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('asynchronous lockfile capture and bounded discovery preserve synchronous selection and validation', async t => {
  const dir = mkdtempSync(join(tmpdir(), 'bridge-async-lock-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  for (let index = 0; index < 70; index++) {
    const path = writeLock(dir, 40000 + index, `token-${index}`);
    const time = Date.now() / 1000 - 100 + index;
    utimesSync(path, time, time);
  }
  const expected = discoverLatestLockfile(dir)!;
  assert.deepEqual(await discoverLatestLockfileAsync(dir), expected);
  assert.deepEqual(await readLockfileAsync(expected.path), readLockfile(expected.path));
  const oldToken = await readLockfileAsync(expected.path);
  writeLock(dir, expected.port, 'rotated-token');
  assert.equal(oldToken.body.authToken, 'token-69', 'a capture must stay stable while the discovery file changes');
  assert.equal((await readLockfileAsync(expected.path)).body.authToken, 'rotated-token', 'a later attempt must capture fresh credentials');
  const malformed = join(dir, 'invalid.json');
  writeFileSync(malformed, '{}');
  await assert.rejects(readLockfileAsync(malformed), /not <port>\.lock/);
  const broken = join(dir, '50000.lock');
  writeFileSync(broken, '{}');
  const newest = Date.now() / 1000 + 10;
  utimesSync(broken, newest, newest);
  assert.equal(discoverLatestLockfile(dir), null);
  assert.equal(await discoverLatestLockfileAsync(dir), null, 'do not silently adopt a different older endpoint after the newest lockfile fails validation');
  assert.equal(await discoverLatestLockfileAsync(join(dir, 'missing')), null);
});

test('client DTO protocol version tracks the source contract major', () => {
  assert.equal(CLIENT_PROTOCOL_VERSION, '33.0.0');
});

test('versionCompatible matches the Rust major-version rule (fail-closed)', () => {
  assert.equal(versionCompatible('0.2.0', '0.2.0'), true);
  assert.equal(versionCompatible('0.2.0', '0.2.7'), true);
  assert.equal(versionCompatible('1.0.0', '1.4.2'), true);
  assert.equal(versionCompatible('0.2.0', '1.0.0'), false);
  assert.equal(versionCompatible('1.0.0', '2.0.0'), false);
  // Fail-closed on unparseable input.
  assert.equal(versionCompatible('1.0.0', ''), false);
  assert.equal(versionCompatible('', '1.0.0'), false);
  assert.equal(versionCompatible('1.0.0', 'abc'), false);
});
