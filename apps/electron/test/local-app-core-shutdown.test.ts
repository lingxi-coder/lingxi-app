import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter, once } from 'node:events';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { ChildProcessWithoutNullStreams } from 'node:child_process';
import { LocalAppCoreClient } from '../src/main/local-apps/coreClient.ts';

test('library shutdown waits for the old process before a new owner starts', { skip: process.platform === 'win32', timeout: 12000 }, async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-shutdown-'));
  const source = join(root, 'server.mjs'), executable = join(root, 'bridge');
  await fs.writeFile(source, `
    import { openSync, closeSync, unlinkSync } from 'node:fs';
    import { join } from 'node:path';
    import { createInterface } from 'node:readline';
    const lock = join(process.argv.at(-1), 'owner.lock');
    const fd = openSync(lock, 'wx');
    process.on('SIGTERM', () => setTimeout(() => { closeSync(fd); unlinkSync(lock); process.exit(0); }, 250));
    setInterval(() => {}, 1000);
    createInterface({ input: process.stdin }).on('line', line => {
      const request = JSON.parse(line);
      process.stdout.write(JSON.stringify({ id: request.id, ok: true, value: [] }) + '\\n');
    });
  `);
  await fs.writeFile(executable, `#!/bin/sh\nexec '${process.execPath}' '${source}' "$@"\n`, { mode: 0o700 });
  const children: ChildProcessWithoutNullStreams[] = [];
  t.after(async () => {
    for (const child of children) {
      if (child.exitCode === null && child.signalCode === null) {
        const closed = once(child, 'close'); child.kill('SIGKILL'); await closed;
      }
    }
    await fs.rm(root, { recursive: true, force: true });
  });
  const first = new LocalAppCoreClient(executable, root);
  const firstRequest = first.request({ op: 'list' });
  children.push(Reflect.get(first, 'child') as ChildProcessWithoutNullStreams);
  assert.deepEqual(await firstRequest, []);
  await Promise.all([first.dispose(), first.dispose()]);
  assert.ok(children[0]!.exitCode !== null || children[0]!.signalCode !== null);
  await assert.rejects(fs.access(join(root, 'owner.lock')));

  const replacement = new LocalAppCoreClient(executable, root);
  const nextRequest = replacement.request({ op: 'list' });
  children.push(Reflect.get(replacement, 'child') as ChildProcessWithoutNullStreams);
  assert.deepEqual(await nextRequest, []);
  await replacement.dispose();
});

test('library shutdown reports an unverified exit and can retry', { timeout: 8000 }, async () => {
  const child = new EventEmitter() as EventEmitter & { stdin: { end(): void }; kill(signal: string): boolean };
  const signals: string[] = [];
  child.stdin = { end: () => undefined };
  child.kill = signal => { signals.push(signal); return false; };
  const client = new LocalAppCoreClient('unused', 'unused');
  Reflect.set(client, 'child', child);
  const results = await Promise.allSettled([client.dispose(), client.dispose()]);
  assert.ok(results.every(result => result.status === 'rejected' && /exit could not be confirmed/.test(String(result.reason))));
  assert.deepEqual(signals, ['SIGTERM', 'SIGKILL'], 'concurrent callers share the same shutdown');
  child.kill = signal => { signals.push(signal); setImmediate(() => child.emit('close', 0, null)); return true; };
  await client.dispose();
  assert.deepEqual(signals, ['SIGTERM', 'SIGKILL', 'SIGTERM']);
});

test('library shutdown escalates to SIGKILL and waits for close', { timeout: 8000 }, async () => {
  const child = new EventEmitter() as EventEmitter & { stdin: { end(): void }; kill(signal: string): boolean };
  const signals: string[] = [];
  child.stdin = { end: () => undefined };
  child.kill = signal => {
    signals.push(signal);
    if (signal === 'SIGKILL') setImmediate(() => child.emit('close', null, 'SIGKILL'));
    return true;
  };
  const client = new LocalAppCoreClient('unused', 'unused');
  Reflect.set(client, 'child', child);
  await client.dispose();
  assert.deepEqual(signals, ['SIGTERM', 'SIGKILL']);
});
