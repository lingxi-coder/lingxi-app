import { test } from 'node:test';
import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import type { ChildProcessWithoutNullStreams } from 'node:child_process';
import { LocalAppCoreClient } from '../src/main/local-apps/coreClient.ts';
import { shellQuote } from '../src/main/local-apps/runtime.ts';

test('a closed library stdin rejects all pending requests and requires recovery', { skip: process.platform === 'win32', timeout: 10000 }, async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-broken-input-'));
  const script = join(root, 'server.mjs'), executable = join(root, 'bridge'), marker = join(root, 'stdin-closed');
  await fs.writeFile(script, `import {closeSync,writeFileSync} from 'node:fs'; import {createInterface} from 'node:readline';
    process.stdin.on('error', () => {});
    createInterface({input:process.stdin}).on('line', line => {
      const value = JSON.parse(line);
      if (value.request.op === 'hold') {
        process.stdin.pause(); closeSync(0); writeFileSync(${JSON.stringify(marker)}, 'closed');
      } else process.stdout.write(JSON.stringify({id:value.id,ok:true,value:'ready'})+'\\n');
    });
    setInterval(() => {}, 1000);`);
  await fs.writeFile(executable, `#!/bin/sh\nexec ${shellQuote(process.execPath)} ${shellQuote(script)} "$@"\n`, { mode: 0o700 });
  const client = new LocalAppCoreClient(executable, root);
  const notices: Error[] = []; client.onDisconnected(error => notices.push(error));
  t.after(async () => {
    const child = Reflect.get(client, 'child') as ChildProcessWithoutNullStreams | undefined;
    if (child && child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
    await client.dispose(); await fs.rm(root, { recursive: true, force: true });
  });
  const outcome = (promise: Promise<unknown>) => promise.then(() => undefined, error => error as Error);
  const held = outcome(client.request({ op: 'hold' }));
  let ready = false;
  for (let i = 0; i < 100 && !ready; i++) {
    try { await fs.access(marker); ready = true; } catch { await delay(20); }
  }
  assert.ok(ready, 'the live child must close its stdin before the next request');
  const first = outcome(client.request({ op: 'one' }));
  const second = outcome(client.request({ op: 'two' }));
  const failures = await Promise.all([held, first, second]);
  for (const failure of failures) assert.match(String(failure), /EPIPE|ERR_STREAM_DESTROYED|stream/i);
  assert.equal(notices.length, 1);
  assert.throws(() => client.request({ op: 'again' }), /EPIPE|ERR_STREAM_DESTROYED|stream/i);
  await client.dispose();
  const replacement = new LocalAppCoreClient(executable, root);
  try { assert.equal(await replacement.request({ op: 'ping' }), 'ready'); }
  finally { await replacement.dispose(); }
});
