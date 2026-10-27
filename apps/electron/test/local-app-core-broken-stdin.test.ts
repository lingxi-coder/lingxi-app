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
  await fs.writeFile(script, `import {createInterface} from 'node:readline';
    createInterface({input:process.stdin}).on('line', line => {
      const value = JSON.parse(line);
      process.stdout.write(JSON.stringify({id:value.id,ok:true,value:'ready'})+'\\n');
    });`);
  await fs.writeFile(executable, `#!/bin/sh
    if [ -f ${shellQuote(marker)} ]; then
      exec ${shellQuote(process.execPath)} ${shellQuote(script)} "$@"
    fi
    IFS= read -r request || exit 1
    case "$request" in *'"op":"hold"'*) ;; *) exit 1 ;; esac
    exec 0<&-
    printf closed > ${shellQuote(marker)}
    exec /bin/sleep 120
  `, { mode: 0o700 });
  const client = new LocalAppCoreClient(executable, root);
  const notices: Error[] = [];
  let childAtDisconnect: ChildProcessWithoutNullStreams | undefined, aliveAtDisconnect = false;
  client.onDisconnected(error => {
    notices.push(error);
    childAtDisconnect = Reflect.get(client, 'child') as ChildProcessWithoutNullStreams | undefined;
    if (childAtDisconnect?.pid) {
      try { process.kill(childAtDisconnect.pid, 0); aliveAtDisconnect = true; } catch { /* reported below */ }
    }
  });
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
  const child = Reflect.get(client, 'child') as ChildProcessWithoutNullStreams | undefined;
  assert.ok(child && child.exitCode === null && child.signalCode === null,
    'fixture must stay alive after closing stdin');
  const pid = child.pid;
  assert.ok(pid, 'fixture must have a live PID');
  const first = outcome(client.request({ op: 'one' }));
  const second = outcome(client.request({ op: 'two' }));
  const failures = await Promise.all([held, first, second]);
  for (const failure of failures) assert.match(String(failure), /EPIPE|ERR_STREAM_DESTROYED|stream/i);
  assert.ok(failures.some(failure => (failure as NodeJS.ErrnoException | undefined)?.code === 'EPIPE'),
    `expected a real EPIPE after closing the pipe: ${failures}`);
  assert.equal(notices.length, 1);
  assert.equal((notices[0] as NodeJS.ErrnoException).code, 'EPIPE');
  assert.equal(childAtDisconnect, child, 'EPIPE must come from the original child');
  assert.equal(aliveAtDisconnect, true, 'the child must be alive when EPIPE is observed');
  await delay(100);
  assert.equal(child.pid, pid);
  assert.equal(child.exitCode, null, 'fixture exited before client cleanup');
  assert.equal(child.signalCode, null, 'fixture was signalled before client cleanup');
  assert.doesNotThrow(() => process.kill(pid, 0), 'fixture must remain alive after EPIPE');
  assert.throws(() => client.request({ op: 'again' }), /EPIPE|ERR_STREAM_DESTROYED|stream/i);
  await client.dispose();
  assert.equal(child.signalCode, 'SIGTERM', 'client must terminate the still-running fixture');
  assert.equal(child.exitCode, null);
  const replacement = new LocalAppCoreClient(executable, root);
  try { assert.equal(await replacement.request({ op: 'ping' }), 'ready'); }
  finally { await replacement.dispose(); }
});
