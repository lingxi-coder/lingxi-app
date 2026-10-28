import { test, type TestContext } from 'node:test';
import assert from 'node:assert/strict';
import childProcess, { type ChildProcessWithoutNullStreams } from 'node:child_process';
import { once } from 'node:events';
import { promises as fs } from 'node:fs';
import { syncBuiltinESMExports } from 'node:module';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { LocalAppCoreClient } from '../src/main/local-apps/coreClient.ts';

async function fakeLibrary(t: TestContext) {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-protocol-'));
  const script = join(root, 'server.mjs');
  await fs.writeFile(script, `
    import { createInterface } from 'node:readline';
    process.on('SIGTERM', () => {});
    setInterval(() => {}, 1000);
    createInterface({ input: process.stdin }).on('line', line => {
      const request = JSON.parse(line);
      if (request.request.op === 'hold') return;
      if (request.request.op === 'bad') process.stdout.write('not-json\\n');
      else if (request.request.op === 'large') process.stdout.write('x'.repeat(64 * 1024 * 1024 + 1));
      else if (request.request.op === 'fragment') {
        const response = JSON.stringify({ id: request.id, ok: true, value: 'fragmented' }) + '\\n';
        process.stdout.write(response.slice(0, 10), () => setTimeout(() => process.stdout.write(response.slice(10)), 20));
      } else if (request.request.op === 'denied') {
        process.stdout.write(JSON.stringify({ id: request.id, ok: false, error: { code: 'DENIED', message: 'denied' } }) + '\\n');
      }
      else process.stdout.write(JSON.stringify({ id: request.id, ok: true, value: 'accepted' }) + '\\n');
    });
  `);
  // Use a real Node child on every platform; only substitute the executable
  // because the production constructor supplies CLI arguments to the bridge.
  const spawn = childProcess.spawn;
  const mocked = t.mock.method(childProcess, 'spawn', (_file, _args, options) =>
    spawn(process.execPath, [script], options));
  syncBuiltinESMExports();
  const reports: string[] = [];
  const client = new LocalAppCoreClient('fixture-library', root, message => reports.push(message));
  let child: ChildProcessWithoutNullStreams | undefined;
  let originalKill: ChildProcessWithoutNullStreams['kill'] | undefined;
  const getChild = () => {
    if (!child) {
      child = Reflect.get(client, 'child') as ChildProcessWithoutNullStreams;
      originalKill = child.kill.bind(child);
    }
    return child;
  };
  t.after(async () => {
    mocked.mock.restore();
    syncBuiltinESMExports();
    child ??= Reflect.get(client, 'child') as ChildProcessWithoutNullStreams | undefined;
    if (child && child.exitCode === null && child.signalCode === null) {
      const closed = once(child, 'close');
      (originalKill ?? child.kill.bind(child))('SIGKILL');
      await closed;
    }
    await client.dispose();
    await fs.rm(root, { recursive: true, force: true });
  });
  return { client, getChild, reports };
}

test('fragmented responses and request errors preserve a healthy connection', { timeout: 5000 }, async t => {
  const { client, getChild } = await fakeLibrary(t);
  const notices: Error[] = [];
  client.onDisconnected(error => notices.push(error));
  assert.equal(await client.request({ op: 'fragment' }), 'fragmented');
  await assert.rejects(client.request({ op: 'denied' }), { message: 'denied', code: 'DENIED' });
  assert.equal(await client.request({ op: 'ready' }), 'accepted');
  assert.equal(notices.length, 0);
  getChild();
  await client.dispose();
});

test('malformed library output permanently disconnects the live child and rejects pending requests', { timeout: 10000 }, async t => {
  const { client, getChild } = await fakeLibrary(t);
  const notices: Error[] = [];
  client.onDisconnected(error => notices.push(error));
  assert.equal(await client.request({ op: 'ready' }), 'accepted');
  const child = getChild();
  const closed = once(child, 'close', { signal: AbortSignal.timeout(5000) });
  const held = assert.rejects(client.request({ op: 'hold' }), /Invalid Local App library response/);
  const bad = assert.rejects(client.request({ op: 'bad' }), /Invalid Local App library response/);
  await Promise.all([held, bad]);
  assert.equal(notices.length, 1, 'the host must be notified immediately');
  assert.throws(() => client.request({ op: 'again' }), /Invalid Local App library response/);
  assert.equal(Reflect.get(client, 'child'), child, 'the child remains owned until close');
  const late: Error[] = [];
  client.onDisconnected(error => late.push(error));
  assert.equal(late[0], notices[0], 'late listeners receive the original failure');
  await closed;
  assert.ok(child.exitCode !== null || child.signalCode !== null, 'automatic cleanup confirms child exit');
  if (process.platform !== 'win32') assert.equal(child.signalCode, 'SIGKILL', 'a child ignoring TERM must be killed');
  assert.equal(notices.length, 1, 'close must not replace the original error');
});

test('oversized output disconnects the child and rejects subsequent requests', { timeout: 30000 }, async t => {
  const { client, getChild } = await fakeLibrary(t);
  const notices: Error[] = [];
  client.onDisconnected(error => notices.push(error));
  assert.equal(await client.request({ op: 'ready' }), 'accepted');
  const child = getChild();
  const closed = once(child, 'close', { signal: AbortSignal.timeout(27000) });
  await assert.rejects(client.request({ op: 'large' }), /response exceeds limit/);
  assert.equal(notices.length, 1);
  assert.throws(() => client.request({ op: 'again' }), /response exceeds limit/);
  assert.equal(Reflect.get(client, 'child'), child);
  await closed;
});

test('failed termination remains owned, reports an unverified exit, and can be retried', { timeout: 10000 }, async t => {
  const { client, getChild, reports } = await fakeLibrary(t);
  const notices: Error[] = [];
  client.onDisconnected(error => notices.push(error));
  assert.equal(await client.request({ op: 'ready' }), 'accepted');
  const child = getChild();
  const originalKill = child.kill.bind(child);
  const signals: string[] = [];
  child.kill = signal => { signals.push(String(signal)); throw new Error('simulated termination failure'); };
  await assert.rejects(client.request({ op: 'bad' }), /Invalid Local App library response/);
  for (let attempts = 0; attempts < 100 && reports.length === 0; attempts++) {
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  assert.match(reports[0] ?? '', /exit could not be confirmed.*simulated termination failure/);
  assert.deepEqual(signals, ['SIGTERM', 'SIGKILL']);
  assert.equal(Reflect.get(client, 'child'), child, 'failed cleanup must retain the child');
  assert.equal(child.exitCode, null);
  assert.equal(child.signalCode, null);
  assert.equal(notices.length, 1);
  assert.throws(() => client.request({ op: 'again' }), /Invalid Local App library response/);
  child.kill = originalKill;
  await client.dispose();
  assert.ok(child.exitCode !== null || child.signalCode !== null, 'retry must confirm exit');
});
