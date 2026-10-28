import { test } from 'node:test';
import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { LocalAppRuntime, shellQuote } from '../src/main/local-apps/runtime.ts';
import { tracedLocalAppCore } from './fixtures/local-app-core-diagnostics.ts';
import { traceLocalAppReadiness } from './fixtures/local-app-runtime-diagnostics.ts';
import { LocalAppStore } from './fixtures/local-app-store.ts';

test('library diagnostics include the executable, pid, stderr, exit and signal', { skip: process.platform === 'win32', timeout: 10000 }, async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-core-diagnostics-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const script = join(root, 'server.mjs'), executable = join(root, 'bridge');
  await fs.writeFile(script, `import {createInterface} from 'node:readline';
    createInterface({input:process.stdin}).on('line', line => {
      const request = JSON.parse(line);
      process.stderr.write('controlled fixture stderr\\n', () => {
        if (request.request.op === 'signal') process.kill(process.pid, 'SIGTERM');
        else process.exit(7);
      });
    });`);
  await fs.writeFile(executable, `#!/bin/sh\nexec ${shellQuote(process.execPath)} ${shellQuote(script)} "$@"\n`, { mode: 0o700 });
  for (const [op, status] of [['exit', 'code=7 signal=none'], ['signal', 'code=none signal=SIGTERM']]) {
    const traces: string[] = [];
    const core = tracedLocalAppCore(executable, root, message => traces.push(message));
    try { await assert.rejects(core.request({ op }), /library exited/); }
    finally { await core.dispose(); }
    assert.ok(traces.includes(`library executable: ${executable}`));
    assert.ok(traces.some(line => /^library process started: pid=\d+$/.test(line)));
    assert.ok(traces.some(line => line.includes('library stderr: controlled fixture stderr')));
    assert.ok(traces.some(line => line.includes('library process close:') && line.endsWith(status)));
  }
});

test('readiness failure evidence distinguishes HTTP errors and refused connections', { timeout: 12000 }, async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-readiness-diagnostics-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  for (const [name, command, expected] of [
    ['http', `${shellQuote(process.execPath)} -e "require('http').createServer((_,r)=>{r.statusCode=503;r.end()}).listen(process.env.PORT,'127.0.0.1')"`, 'http:503'],
    ['refused', `${shellQuote(process.execPath)} -e "setInterval(()=>{},1000)"`, 'fetch:TypeError:ECONNREFUSED'],
  ]) {
    const store = new LocalAppStore(join(root, name));
    const runtime = new LocalAppRuntime(store as unknown as ConstructorParameters<typeof LocalAppRuntime>[0], () => undefined);
    const source = join(store.root, 'imports', 'source');
    await fs.mkdir(source, { recursive: true });
    await fs.writeFile(join(source, 'index.html'), '<!doctype html>');
    await fs.writeFile(join(source, 'lingxi-app.json'), JSON.stringify({ schema: 2, name: 'Diagnostics',
      targets: [{ platform: 'macos', environment: 'native' }], entry: { type: 'static', path: 'index.html' },
      processes: [{ name: 'api', command, env: { FIXTURE_PRIVATE_VALUE: 'fixture-private-value' }, ready: { timeoutMs: 1000 } }] }));
    const row = await store.publish({ staged_path: source }); row.grants = row.manifest.capabilities ?? []; await store.save(row);
    const diagnostics = traceLocalAppReadiness(runtime, row.id, 'api');
    try {
      let failure: Error | undefined;
      try { await runtime.start(row.id); } catch (error) { failure = diagnostics.failure('Controlled failure', error, runtime.logs(row.id)); }
      assert.ok(failure, 'the deliberately unready service must fail');
      const details = JSON.parse(failure.message.slice(failure.message.indexOf('{')));
      assert.ok(details.readiness.expectedPort > 0);
      assert.ok(details.readiness.attempts[expected] > 0, JSON.stringify(details.readiness));
      assert.ok(details.readiness.processes.some((entry: { name: string; pid: number }) => entry.name === 'api' && entry.pid > 0));
      if (process.platform !== 'win32') {
        const shell = details.readiness.processes.find((entry: { name: string }) => entry.name === 'api');
        assert.ok(shell.guardianPipe);
        assert.ok(details.readiness.processGroup.some((entry: { pid: number }) => entry.pid === shell.pid));
        assert.ok(details.readiness.processGroup.some((entry: { parentPID: number }) => entry.parentPID === shell.pid));
      }
      assert.ok(!failure.message.includes('fixture-private-value'));
    } finally { diagnostics.restore(); await runtime.dispose(); }
  }
});
