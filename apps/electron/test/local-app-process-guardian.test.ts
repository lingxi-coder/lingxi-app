import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn, type ChildProcess } from 'node:child_process';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
import { guardedProcessArguments } from '../src/main/local-apps/processGuardian.ts';
import { LocalAppRuntime } from '../src/main/local-apps/runtime.ts';
import { LocalAppStore } from './fixtures/local-app-store.ts';

test('guardian preserves command status and closes with the command', { skip: process.platform === 'win32', timeout: 10000 }, async () => {
  const child = spawn('/bin/bash', guardedProcessArguments('printf "completed"; exit 17'), { detached: true, stdio: ['ignore', 'pipe', 'pipe', 'pipe'] });
  const guardian = child.stdio[3] as NodeJS.ReadWriteStream;
  guardian.on('error', () => undefined);
  child.once('exit', () => guardian.end());
  let text = ''; child.stdout!.on('data', chunk => { text += chunk; });
  assert.equal(await new Promise(resolve => child.once('close', resolve)), 17);
  assert.equal(text, 'completed');
});

test('guardian reaps only its app process group after abrupt host death', { skip: process.platform === 'win32', timeout: 15000 }, async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-parent-death-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const parentSource = join(root, 'parent.mjs'), receipt = join(root, 'child.json');
  const module = pathToFileURL(resolve('src/main/local-apps/processGuardian.ts')).href;
  await fs.writeFile(parentSource, `import {spawn} from 'node:child_process'; import {writeFileSync} from 'node:fs'; import {guardedProcessArguments} from ${JSON.stringify(module)};
const app = spawn('/bin/bash', guardedProcessArguments(${JSON.stringify('/bin/bash -c \'trap "" TERM; exec /bin/sleep 120\' & wait')}), {detached:true,stdio:['ignore','pipe','pipe','pipe']});
app.once('spawn',()=>writeFileSync(${JSON.stringify(receipt)},JSON.stringify({pid:app.pid}))); setInterval(()=>{},1000);`);
  const host = spawn(process.execPath, ['--import', resolve('../../packages/bridge-client/node_modules/tsx/dist/loader.mjs'), parentSource], { stdio: 'ignore' });
  let appPid: number | undefined;
  let appGroupExists = true;
  t.after(() => { host.kill('SIGKILL'); if (appPid && appGroupExists) { try { process.kill(-appPid, 'SIGKILL'); } catch { /* already reaped */ } } });
  for (let i = 0; i < 100; i++) {
    try { appPid = JSON.parse(await fs.readFile(receipt, 'utf8')).pid; break; } catch { await delay(30); }
  }
  assert.ok(appPid, 'isolated host starts an owned app');
  process.kill(appPid!, 0);
  host.kill('SIGKILL');
  for (let i = 0; i < 150; i++) {
    try { process.kill(-appPid!, 0); } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ESRCH') { appGroupExists = false; return; }
      throw error;
    }
    await delay(40);
  }
  assert.fail('app process group survived its parent-owned pipe closing');
});

function startGuarded(command: string, guardianMarker?: string) {
  const args = guardedProcessArguments(command);
  if (guardianMarker) {
    const trap = "trap '' USR1";
    assert.ok(args[1]!.includes(trap));
    // Force the wakeup to arrive before the guardian installs its handler if
    // the parent is allowed to start the command before guardian readiness.
    args[1] = args[1]!.replace(trap, `trap '' USR1\n: > '${guardianMarker}'\nsleep 0.75\n${trap}`);
  }
  const child = spawn('/bin/bash', args, { detached: true, stdio: ['ignore', 'pipe', 'pipe', 'pipe'] });
  const guardian = child.stdio[3] as NodeJS.ReadWriteStream;
  guardian.on('error', () => undefined);
  child.once('exit', () => guardian.end());
  const closed = new Promise<{ code: number | null; signal: NodeJS.Signals | null }>((resolve, reject) => {
    child.once('error', reject);
    child.once('close', (code, signal) => resolve({ code, signal }));
  });
  return { child, closed };
}

async function cleanup(child: ChildProcess, closed: Promise<unknown>) {
  if (child.exitCode === null && child.signalCode === null && child.pid) {
    try { process.kill(-child.pid, 'SIGKILL'); } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ESRCH') throw error;
    }
    await closed;
  }
}

test('guardian is ready before an immediate command exit', { skip: process.platform === 'win32', timeout: 5000 }, async () => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-guardian-'));
  const marker = join(root, 'unready'), commandMarker = join(root, 'command-started');
  const { child, closed } = startGuarded(`: > '${commandMarker}'; exit 17`, marker);
  try {
    for (let i = 0; i < 100; i++) { try { await fs.access(marker); break; } catch { await delay(10); } }
    await assert.rejects(fs.access(commandMarker), 'command must wait for guardian readiness');
    const result = await Promise.race([closed, delay(2000).then(() => { throw new Error('guardian prevented command exit'); })]);
    assert.deepEqual(result, { code: 17, signal: null });
  } finally { await cleanup(child, closed); await fs.rm(root, { recursive: true, force: true }); }
});

test('closing the parent pipe terminates the running command and its descendant', { skip: process.platform === 'win32', timeout: 8000 }, async () => {
  const { child, closed } = startGuarded('sleep 30 & worker=$!; printf "%s\\n" "$worker"; wait "$worker"');
  try {
    const pid = await Promise.race([
      new Promise<number>(resolve => child.stdout!.once('data', data => resolve(Number(String(data).trim())))),
      delay(2000).then(() => { throw new Error('command did not start'); }),
    ]);
    assert.ok(Number.isInteger(pid) && pid > 0);
    process.kill(pid, 0);
    (child.stdio[3] as NodeJS.WritableStream).end();
    await Promise.race([closed, delay(4000).then(() => { throw new Error('command did not stop after parent pipe closed'); })]);
    let alive = true;
    for (let i = 0; i < 40 && alive; i++) {
      try { process.kill(pid, 0); await delay(50); }
      catch (error) {
        if ((error as NodeJS.ErrnoException).code !== 'ESRCH') throw error;
        alive = false;
      }
    }
    assert.equal(alive, false, 'the command descendant must be gone');
  } finally { await cleanup(child, closed); }
});

test('runtime cleans up a detached descendant after its install command exits', { skip: process.platform === 'win32', timeout: 10000 }, async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-exited-command-'));
  const store = new LocalAppStore(root), runtime = new LocalAppRuntime(store, () => undefined);
  t.after(async () => { try { await runtime.dispose(); } finally { await fs.rm(root, { recursive: true, force: true }); } });
  const source = join(root, 'imports', 'source'), pidFile = join(root, 'worker.pid');
  await fs.mkdir(source, { recursive: true });
  await fs.writeFile(join(source, 'index.html'), '<!doctype html>');
  await fs.writeFile(join(source, 'lingxi-app.json'), JSON.stringify({ schema: 2, name: 'Cleanup',
    targets: [{ platform: 'macos', environment: 'native' }], entry: { type: 'static', path: 'index.html' },
    install: [{ command: `sleep 30 >/dev/null 2>&1 & echo $! > '${pidFile}'; exit 0` }] }));
  const row = await store.publish({ staged_path: source });
  row.grants = row.manifest.capabilities ?? []; await store.save(row);
  await runtime.start(row.id);
  const pid = Number(await fs.readFile(pidFile, 'utf8'));
  assert.ok(Number.isInteger(pid) && pid > 0);
  let alive = true;
  for (let i = 0; i < 40 && alive; i++) {
    try { process.kill(pid, 0); await delay(50); }
    catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ESRCH') throw error;
      alive = false;
    }
  }
  assert.equal(alive, false, 'the completed command must not leave its process group running');
});

test('runtime reaps a descendant holding command stdout after its leader exits', { skip: process.platform === 'win32', timeout: 12000 }, async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-inherited-output-'));
  const store = new LocalAppStore(root), runtime = new LocalAppRuntime(store, () => undefined);
  t.after(async () => { try { await runtime.dispose(); } finally { await fs.rm(root, { recursive: true, force: true }); } });
  const source = join(root, 'imports', 'source'), pidFile = join(root, 'worker.pid');
  await fs.mkdir(source, { recursive: true });
  await fs.writeFile(join(source, 'index.html'), '<!doctype html>');
  await fs.writeFile(join(source, 'lingxi-app.json'), JSON.stringify({ schema: 2, name: 'Inherited output',
    targets: [{ platform: 'macos', environment: 'native' }], entry: { type: 'static', path: 'index.html' },
    install: [{ command: `printf 'command-output\\n'; sleep 30 & echo $! > '${pidFile}'; sleep 0.25; exit 0` }] }));
  const row = await store.publish({ staged_path: source });
  row.grants = row.manifest.capabilities ?? []; await store.save(row);
  const originalKill = process.kill;
  const groupSignals: Array<NodeJS.Signals | number | undefined> = [];
  process.kill = ((pid, signal) => {
    if (pid < 0) groupSignals.push(signal);
    return originalKill(pid, signal);
  }) as typeof process.kill;
  try {
    await Promise.race([
      runtime.start(row.id),
      delay(2500).then(() => { throw new Error('install waited for a descendant holding stdout'); }),
    ]);
  } finally { process.kill = originalKill; }
  const pid = Number(await fs.readFile(pidFile, 'utf8'));
  assert.ok(Number.isInteger(pid) && pid > 0);
  assert.throws(() => process.kill(pid, 0), (error: unknown) => (error as NodeJS.ErrnoException).code === 'ESRCH', 'start must wait for the descendant to exit');
  assert.ok(runtime.logs(row.id).some(line => line.text.includes('command-output')), 'output must drain before start completes');
  assert.ok(groupSignals.length > 0 && groupSignals.every(signal => signal === 0), 'the host may probe but must not signal a stale process-group id');
});

test('a failed or stalled run-ended callback still stops the other service after a crash', { skip: process.platform === 'win32', timeout: 20000 }, async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-callback-failure-'));
  const runtimes: LocalAppRuntime[] = [];
  t.after(async () => { try { await Promise.allSettled(runtimes.map(runtime => runtime.dispose())); } finally { await fs.rm(root, { recursive: true, force: true }); } });
  for (const mode of ['reject', 'throw', 'stall'] as const) {
    const directory = join(root, mode), store = new LocalAppStore(directory);
    const runtime = new LocalAppRuntime(store, () => undefined, { onRunEnded: (_id, _run, reason) => {
      if (reason !== 'failed') return;
      if (mode === 'throw') throw new Error('controlled broker failure');
      if (mode === 'stall') return new Promise<void>(() => undefined);
      return Promise.reject(new Error('controlled broker failure'));
    } });
    runtimes.push(runtime);
    const source = join(directory, 'imports', 'source'), pidFile = join(directory, 'worker.pid'), crashFile = join(directory, 'crash');
    await fs.mkdir(source, { recursive: true });
    await fs.writeFile(join(source, 'index.html'), '<!doctype html>');
    await fs.writeFile(join(source, 'lingxi-app.json'), JSON.stringify({ schema: 2, name: 'Crash cleanup',
      targets: [{ platform: 'macos', environment: 'native' }], entry: { type: 'static', path: 'index.html' },
      processes: [
        { name: 'first', command: `echo first-ready; until [ -f '${crashFile}' ]; do sleep 0.05; done; exit 7`, ready: { output: 'first-ready', timeoutMs: 3000 } },
        { name: 'second', command: `sleep 30 & worker=$!; echo "$worker" > '${pidFile}'; echo second-ready; wait "$worker"`, ready: { output: 'second-ready', timeoutMs: 3000 } },
      ] }));
    const row = await store.publish({ staged_path: source });
    row.grants = row.manifest.capabilities ?? []; await store.save(row);
    await runtime.start(row.id);
    const pid = Number(await fs.readFile(pidFile, 'utf8'));
    assert.ok(Number.isInteger(pid) && pid > 0);
    await fs.writeFile(crashFile, 'crash');
    for (let i = 0; i < 100 && runtime.phase(row.id) !== 'failed'; i++) await delay(30);
    assert.equal(runtime.phase(row.id), 'failed');
    let alive = true;
    for (let i = 0; i < 60 && alive; i++) {
      try { process.kill(pid, 0); await delay(50); }
      catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ESRCH') throw error; alive = false; }
    }
    assert.equal(alive, false, `other services must stop when the callback ${mode}s`);
    if (mode !== 'stall') assert.ok(runtime.logs(row.id).some(line => line.text.includes('controlled broker failure')));
  }
});

test('losing the guardian reports unverified cleanup and retains the child', { skip: process.platform === 'win32', timeout: 10000 }, async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-missing-guardian-'));
  const shell = join(root, 'shell');
  await fs.writeFile(shell, '#!/bin/bash\nexec 3<&-\necho fixture-ready\nexec sleep 30\n', { mode: 0o700 });
  const runtime = new LocalAppRuntime(new LocalAppStore(root), () => undefined, { shell });
  const run = { id: 'fixture', controller: new AbortController(), children: new Set<ChildProcess>(), services: new Map(), readiness: new Map() };
  const child = (Reflect.get(runtime, 'spawn') as (...args: unknown[]) => ChildProcess)
    .call(runtime, 'fixture', run, 'fixture', 'ignored', root, process.env);
  const closed = new Promise<void>(resolve => child.once('close', () => resolve()));
  t.after(async () => {
    if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
    await Promise.race([closed, delay(3000)]);
    await fs.rm(root, { recursive: true, force: true });
  });
  await Promise.race([
    new Promise<void>(resolve => child.stdout!.once('data', () => resolve())),
    delay(2000).then(() => { throw new Error('fixture child did not start'); }),
  ]);
  const killChildren = Reflect.get(runtime, 'killChildren') as (run: unknown) => Promise<void>;
  await assert.rejects(killChildren.call(runtime, run), /cleanup is unverified/);
  assert.ok(run.children.has(child), 'unverified child ownership must remain available for retry');
  assert.equal(child.exitCode, null);
  assert.equal(child.signalCode, null);
});

test('stalled stop callback does not prevent process cleanup and retry reuses it', { skip: process.platform === 'win32', timeout: 15000 }, async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-stalled-stop-'));
  const store = new LocalAppStore(root);
  let calls = 0, release!: () => void;
  const callback = new Promise<void>(resolve => { release = resolve; });
  const runtime = new LocalAppRuntime(store, () => undefined, { onRunEnded: (_id, _run, reason) => {
    if (reason !== 'stopped') return;
    calls++;
    return callback;
  } });
  t.after(async () => { release(); try { await runtime.dispose().catch(() => undefined); } finally { await fs.rm(root, { recursive: true, force: true }); } });
  const source = join(root, 'imports', 'source'), pidFile = join(root, 'worker.pid');
  await fs.mkdir(source, { recursive: true });
  await fs.writeFile(join(source, 'index.html'), '<!doctype html>');
  await fs.writeFile(join(source, 'lingxi-app.json'), JSON.stringify({ schema: 2, name: 'Stalled stop',
    targets: [{ platform: 'macos', environment: 'native' }], entry: { type: 'static', path: 'index.html' },
    processes: [{ name: 'worker', command: `sleep 30 & worker=$!; echo "$worker" > '${pidFile}'; echo ready; wait "$worker"`, ready: { output: 'ready', timeoutMs: 3000 } }] }));
  const row = await store.publish({ staged_path: source }); row.grants = row.manifest.capabilities ?? []; await store.save(row);
  await runtime.start(row.id);
  const invocationId = runtime.invocationId(row.id)!;
  const pid = Number(await fs.readFile(pidFile, 'utf8'));
  assert.ok(Number.isInteger(pid) && pid > 0);
  const first = runtime.stop(row.id).then(() => undefined, error => error as Error);
  let alive = true;
  for (let i = 0; i < 40 && alive; i++) {
    try { process.kill(pid, 0); await delay(50); }
    catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ESRCH') throw error; alive = false; }
  }
  assert.equal(alive, false, 'process cleanup must run while the callback is blocked');
  const failure = await Promise.race([first, delay(5000).then(() => { throw new Error('stop did not report the stalled callback'); })]);
  assert.ok(failure instanceof Error && /cleanup needs retry/.test(failure.message));
  assert.equal(runtime.phase(row.id), 'stopping');
  const retry = runtime.stop(row.id);
  await delay(100);
  assert.equal(calls, 1, 'retry must reuse the still-pending callback');
  release();
  await retry;
  assert.equal(runtime.phase(row.id), 'stopped');
  await runtime.resume(row.id, row.revision, invocationId);
  await runtime.stop(row.id);
  assert.equal(calls, 2, 'a resumed lifecycle must notify its next stop separately');
});

test('stop aggregates callback and process cleanup errors and allows retry', { skip: process.platform === 'win32', timeout: 5000 }, async () => {
  let callbacks = 0;
  const runtime = new LocalAppRuntime({} as ConstructorParameters<typeof LocalAppRuntime>[0], () => undefined, { onRunEnded: () => {
    callbacks++;
    if (callbacks === 1) throw new Error('controlled callback failure');
  } });
  const unavailable = { pid: 12345, stdio: [] } as unknown as ChildProcess;
  const run = { id: 'fixture', controller: new AbortController(), phase: 'running',
    children: new Set([unavailable]), services: new Map(), retired: [] };
  const stopInvocation = Reflect.get(runtime, 'stopInvocation') as (id: string, run: unknown) => Promise<void>;
  let failure: unknown;
  try { await stopInvocation.call(runtime, 'fixture', run); } catch (error) { failure = error; }
  assert.ok(failure instanceof AggregateError);
  assert.ok(failure.errors.some(error => String(error).includes('controlled callback failure')));
  assert.ok(failure.errors.some(error => String(error).includes('guardian is unavailable')));
  assert.equal(run.phase, 'stopping');
  assert.ok(run.children.has(unavailable));
  run.children.delete(unavailable);
  await stopInvocation.call(runtime, 'fixture', run);
  assert.equal(callbacks, 2);
  assert.equal(run.phase, 'stopped');
});

test('Windows cleanup uses the held leader handle but keeps tree cleanup unverified', { skip: process.platform === 'win32', timeout: 5000 }, async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-taskkill-review-'));
  const marker = join(root, 'taskkill-called');
  await fs.writeFile(join(root, 'taskkill'), `#!/bin/sh\necho called > '${marker}'\nexit 0\n`, { mode: 0o700 });
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const child = spawn(process.execPath, ['-e', 'setInterval(()=>{},1000)'], { stdio: 'ignore' });
  const closed = new Promise<void>(resolve => child.once('close', () => resolve()));
  await new Promise<void>((resolve, reject) => { child.once('spawn', resolve); child.once('error', reject); });
  const runtime = new LocalAppRuntime({} as ConstructorParameters<typeof LocalAppRuntime>[0], () => undefined);
  const run = { children: new Set([child]), services: new Map([['fixture', {}]]) };
  const killChildren = Reflect.get(runtime, 'killChildren') as (run: unknown) => Promise<void>;
  const platform = Object.getOwnPropertyDescriptor(process, 'platform')!, previousPath = process.env.PATH;
  try {
    Object.defineProperty(process, 'platform', { ...platform, value: 'win32' }); process.env.PATH = root;
    await assert.rejects(killChildren.call(runtime, run), /process-tree cleanup is unverified/);
    await Promise.race([closed, delay(2000).then(() => { throw new Error('held leader did not stop'); })]);
    assert.ok(run.children.has(child), 'a stopped leader does not establish tree cleanup');
    assert.equal(run.services.size, 1);
    await assert.rejects(killChildren.call(runtime, run), /process-tree cleanup is unverified/);
    assert.ok(run.children.has(child), 'retry cannot discard the unverified tree');
    await assert.rejects(fs.access(marker), 'a bare PID must not be passed to taskkill');
  } finally {
    Object.defineProperty(process, 'platform', platform); process.env.PATH = previousPath;
    if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
    await closed;
  }
});

test('failed run with a pending notification stays owned across restart and stop', { timeout: 12000 }, async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-failed-notification-'));
  const store = new LocalAppStore(root);
  let release!: () => void, calls = 0, finished = false, stopped = 0;
  const blocked = new Promise<void>(resolve => { release = resolve; });
  const runtime = new LocalAppRuntime(store, () => undefined, { onRunEnded: async (_id, _run, reason) => {
    if (reason === 'stopped') { stopped++; return; }
    if (reason === 'failed') { calls++; await blocked; finished = true; }
  } });
  t.after(async () => { release(); await runtime.dispose().catch(() => undefined); await fs.rm(root, { recursive: true, force: true }); });
  const source = join(root, 'imports', 'source');
  await fs.mkdir(source, { recursive: true });
  await fs.writeFile(join(source, 'index.html'), '<!doctype html>');
  await fs.writeFile(join(source, 'lingxi-app.json'), JSON.stringify({ schema: 2, name: 'Failed notification',
    targets: [{ platform: 'macos', environment: 'native' }], entry: { type: 'static', path: 'index.html' } }));
  const row = await store.publish({ staged_path: source }); row.grants = row.manifest.capabilities ?? []; await store.save(row);
  const old = { id: 'old', operationId: 'old', revision: row.revision, controller: new AbortController(), phase: 'failed',
    children: new Set<ChildProcess>(), services: new Map(), readiness: new Map(), manifest: row.manifest, workspace: '' };
  (Reflect.get(runtime, 'invocations') as Map<string, unknown>).set(row.id, old);
  (Reflect.get(runtime, 'cleanupFailedRun') as (id: string, run: unknown) => void).call(runtime, row.id, old);
  await Promise.resolve();
  assert.equal(calls, 1);
  await assert.rejects(runtime.start(row.id), /cleanup needs retry/);
  assert.equal(runtime.invocationId(row.id), old.id, 'restart cannot abandon the pending failed notification');
  assert.equal(finished, false);
  assert.equal(stopped, 0, 'a failed invocation retains its original failure notification');
  const retry = runtime.stop(row.id);
  await delay(100);
  assert.equal(calls, 1, 'retry must reuse the in-flight failed notification');
  release(); await retry;
  assert.equal(finished, true);
  await runtime.start(row.id);
  assert.notEqual(runtime.invocationId(row.id), old.id);
});

test('failed run with an unverified child stays owned when restart fails', { skip: process.platform === 'win32', timeout: 8000 }, async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-failed-child-'));
  const store = new LocalAppStore(root), runtime = new LocalAppRuntime(store, () => undefined);
  const source = join(root, 'imports', 'source');
  await fs.mkdir(source, { recursive: true });
  await fs.writeFile(join(source, 'index.html'), '<!doctype html>');
  await fs.writeFile(join(source, 'lingxi-app.json'), JSON.stringify({ schema: 2, name: 'Failed child',
    targets: [{ platform: 'macos', environment: 'native' }], entry: { type: 'static', path: 'index.html' } }));
  const row = await store.publish({ staged_path: source }); row.grants = row.manifest.capabilities ?? []; await store.save(row);
  const child = spawn(process.execPath, ['-e', 'setInterval(()=>{},1000)'], { stdio: 'ignore' });
  const closed = new Promise<void>(resolve => child.once('close', () => resolve()));
  await new Promise<void>((resolve, reject) => { child.once('spawn', resolve); child.once('error', reject); });
  const old = { id: 'old', operationId: 'old', revision: row.revision, controller: new AbortController(), phase: 'failed',
    children: new Set([child]), services: new Map(), readiness: new Map(), manifest: row.manifest, workspace: '' };
  (Reflect.get(runtime, 'invocations') as Map<string, unknown>).set(row.id, old);
  (Reflect.get(runtime, 'cleanupFailedRun') as (id: string, run: unknown) => void).call(runtime, row.id, old);
  t.after(async () => { if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL'); await closed; await fs.rm(root, { recursive: true, force: true }); });
  await assert.rejects(runtime.start(row.id), /cleanup needs retry/);
  assert.equal(runtime.invocationId(row.id), old.id);
  assert.ok(old.children.has(child), 'failed cleanup must retain the original child');
  assert.equal(child.exitCode, null);
});

test('Windows natural command completion releases the leader without taskkill', { skip: process.platform === 'win32', timeout: 5000 }, async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-windows-complete-'));
  const marker = join(root, 'taskkill-called');
  await fs.writeFile(join(root, 'taskkill'), `#!/bin/sh\necho called > '${marker}'\nexit 0\n`, { mode: 0o700 });
  const runtime = new LocalAppRuntime({} as ConstructorParameters<typeof LocalAppRuntime>[0], () => undefined, { shell: '/bin/bash' });
  const run = { id: 'fixture', controller: new AbortController(), children: new Set<ChildProcess>(), services: new Map(), readiness: new Map() };
  const platform = Object.getOwnPropertyDescriptor(process, 'platform')!, previousPath = process.env.PATH;
  let child: ChildProcess;
  try {
    Object.defineProperty(process, 'platform', { ...platform, value: 'win32' }); process.env.PATH = root;
    child = (Reflect.get(runtime, 'spawn') as (...args: unknown[]) => ChildProcess).call(runtime, 'fixture', run, 'installing', 'exit 0', root, process.env);
    await new Promise<void>((resolve, reject) => { child.once('error', reject); child.once('close', () => resolve()); });
    await delay(50);
    assert.equal(run.children.has(child), false, 'a naturally completed command must release its leader');
    await assert.rejects(fs.access(marker), 'an exited PID must not be passed to taskkill');
  } finally {
    Object.defineProperty(process, 'platform', platform); process.env.PATH = previousPath;
    await fs.rm(root, { recursive: true, force: true });
  }
});

test('Windows cleanup retains an exited leader without signalling its stale PID', { skip: process.platform === 'win32', timeout: 5000 }, async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-windows-stale-pid-'));
  const marker = join(root, 'taskkill-called');
  await fs.writeFile(join(root, 'taskkill'), `#!/bin/sh\necho called > '${marker}'\nexit 0\n`, { mode: 0o700 });
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const child = spawn(process.execPath, ['-e', 'process.exit(0)'], { stdio: 'ignore' });
  await new Promise<void>((resolve, reject) => { child.once('error', reject); child.once('close', () => resolve()); });
  const runtime = new LocalAppRuntime({} as ConstructorParameters<typeof LocalAppRuntime>[0], () => undefined);
  const run = { children: new Set([child]), services: new Map() };
  const killChildren = Reflect.get(runtime, 'killChildren') as (run: unknown) => Promise<void>;
  const platform = Object.getOwnPropertyDescriptor(process, 'platform')!, previousPath = process.env.PATH;
  try {
    Object.defineProperty(process, 'platform', { ...platform, value: 'win32' }); process.env.PATH = root;
    await assert.rejects(killChildren.call(runtime, run), /cleanup is unverified/);
    assert.ok(run.children.has(child));
    await assert.rejects(fs.access(marker), 'an exited PID must not be passed to taskkill');
  } finally { Object.defineProperty(process, 'platform', platform); process.env.PATH = previousPath; }
});

test('replacement cannot discard a previous run that fails during preparation', { timeout: 12000 }, async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-failed-handover-'));
  const store = new LocalAppStore(root);
  let releasePreparation!: () => void, enteredPreparation!: () => void, releaseFailed!: () => void;
  const preparing = new Promise<void>(resolve => { enteredPreparation = resolve; });
  const holdPreparation = new Promise<void>(resolve => { releasePreparation = resolve; });
  const holdFailed = new Promise<void>(resolve => { releaseFailed = resolve; });
  const prepareWorkspace = store.prepareWorkspace.bind(store);
  store.prepareWorkspace = async row => { enteredPreparation(); await holdPreparation; return prepareWorkspace(row); };
  const runtime = new LocalAppRuntime(store, () => undefined, { onRunEnded: (_id, runId, reason) => {
    if (runId === 'old' && reason === 'failed') return holdFailed;
  } });
  t.after(async () => { releasePreparation(); releaseFailed(); await runtime.dispose().catch(() => undefined); await fs.rm(root, { recursive: true, force: true }); });
  const source = join(root, 'imports', 'source');
  await fs.mkdir(source, { recursive: true });
  await fs.writeFile(join(source, 'index.html'), '<!doctype html>');
  await fs.writeFile(join(source, 'lingxi-app.json'), JSON.stringify({ schema: 2, name: 'Failed handover',
    targets: [{ platform: 'macos', environment: 'native' }], entry: { type: 'static', path: 'index.html' } }));
  const row = await store.publish({ staged_path: source }); row.grants = row.manifest.capabilities ?? []; await store.save(row);
  const old = { id: 'old', operationId: 'old', revision: row.revision, controller: new AbortController(), phase: 'running',
    children: new Set<ChildProcess>(), services: new Map(), readiness: new Map(), manifest: row.manifest, workspace: '' };
  (Reflect.get(runtime, 'invocations') as Map<string, unknown>).set(row.id, old);
  const replacement = runtime.start(row.id, false, undefined, true);
  await preparing;
  old.phase = 'failed'; old.controller.abort();
  (Reflect.get(runtime, 'cleanupFailedRun') as (id: string, run: unknown) => void).call(runtime, row.id, old);
  releasePreparation();
  await assert.rejects(replacement, /cleanup needs retry/);
  assert.equal(runtime.invocationId(row.id), old.id, 'failed previous invocation must remain reachable');
  releaseFailed(); await runtime.stop(row.id);
  assert.equal(runtime.phase(row.id), 'stopped');
});

test('failed notification rejection is retried with the original failure reason', { timeout: 5000 }, async () => {
  const reasons: string[] = [];
  const runtime = new LocalAppRuntime({} as ConstructorParameters<typeof LocalAppRuntime>[0], () => undefined, {
    onRunEnded: (_id, _run, reason) => {
      reasons.push(reason);
      if (reasons.length === 1) throw new Error('controlled failed notification error');
    },
  });
  const run = { id: 'old', controller: new AbortController(), phase: 'failed', children: new Set<ChildProcess>(), services: new Map(), readiness: new Map() };
  (Reflect.get(runtime, 'invocations') as Map<string, unknown>).set('fixture', run);
  (Reflect.get(runtime, 'cleanupFailedRun') as (id: string, run: unknown) => void).call(runtime, 'fixture', run);
  await delay(0);
  assert.deepEqual(reasons, ['failed']);
  await (Reflect.get(runtime, 'stopInvocation') as (id: string, run: unknown) => Promise<void>).call(runtime, 'fixture', run);
  assert.deepEqual(reasons, ['failed', 'failed']);
  assert.equal(run.phase, 'stopped');
});

test('failed replacement remains owned until its failed notification finishes', { skip: process.platform === 'win32', timeout: 12000 }, async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-failed-candidate-'));
  const store = new LocalAppStore(root);
  let release!: () => void, calls = 0;
  const blocked = new Promise<void>(resolve => { release = resolve; });
  const runtime = new LocalAppRuntime(store, () => undefined, { onRunEnded: (_id, _run, reason) => {
    if (reason === 'failed') { calls++; return blocked; }
  } });
  t.after(async () => { release(); await runtime.dispose().catch(() => undefined); await fs.rm(root, { recursive: true, force: true }); });
  const old = await store.publish({ name: 'Stable', html: '<!doctype html>stable' });
  await runtime.start(old.id);
  const oldId = runtime.invocationId(old.id);
  const source = join(root, 'imports', 'bad-update'); await fs.mkdir(source, { recursive: true });
  await fs.writeFile(join(source, 'index.html'), '<!doctype html>bad');
  await fs.writeFile(join(source, 'lingxi-app.json'), JSON.stringify({ schema: 2, name: 'Bad candidate',
    targets: [{ platform: 'macos', environment: 'native' }], entry: { type: 'static', path: 'index.html' }, install: [{ command: 'exit 17' }] }));
  const updated = await store.publish({ id: old.id, staged_path: source }); updated.grants = ['process']; await store.save(updated);
  await assert.rejects(runtime.start(old.id, false, undefined, true), /cleanup needs retry/);
  const preparing = Reflect.get(runtime, 'preparing') as Map<string, { id: string }>;
  assert.ok(preparing.has(old.id), 'the candidate with an unfinished callback remains reachable');
  assert.equal(runtime.invocationId(old.id), oldId);
  const stop = runtime.stop(old.id);
  await delay(100);
  assert.equal(calls, 1, 'stop must reuse the candidate callback in flight');
  release(); await stop;
  assert.equal(preparing.has(old.id), false);
});

test('a resume startup failure retains its failed notification for stop retry', { skip: process.platform === 'win32', timeout: 12000 }, async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-failed-resume-'));
  const store = new LocalAppStore(root);
  let release!: () => void, failedCalls = 0;
  const blocked = new Promise<void>(resolve => { release = resolve; });
  const runtime = new LocalAppRuntime(store, () => undefined, { onRunEnded: (_id, _run, reason) => {
    if (reason === 'failed') { failedCalls++; return blocked; }
  } });
  t.after(async () => { release(); await runtime.dispose().catch(() => undefined); await fs.rm(root, { recursive: true, force: true }); });
  const source = join(root, 'imports', 'source'); await fs.mkdir(source, { recursive: true });
  await fs.writeFile(join(source, 'index.html'), '<!doctype html>');
  await fs.writeFile(join(source, 'lingxi-app.json'), JSON.stringify({ schema: 2, name: 'Resume failure',
    targets: [{ platform: 'macos', environment: 'native' }], entry: { type: 'static', path: 'index.html' },
    processes: [{ name: 'worker', command: 'echo ready; exec sleep 30', ready: { output: 'ready', timeoutMs: 3000 } }] }));
  const row = await store.publish({ staged_path: source }); row.grants = ['process']; await store.save(row);
  await runtime.start(row.id); const invocation = runtime.invocationId(row.id)!; await runtime.stop(row.id);
  (Reflect.get(runtime, 'options') as { shell?: string }).shell = join(root, 'missing-shell');
  await assert.rejects(runtime.resume(row.id, row.revision, invocation), /cleanup needs retry/);
  assert.equal(runtime.invocationId(row.id), invocation);
  assert.equal(runtime.phase(row.id), 'stopping');
  assert.equal(failedCalls, 1);
  const retry = runtime.stop(row.id); await delay(100);
  assert.equal(failedCalls, 1);
  release(); await retry;
  assert.equal(runtime.phase(row.id), 'stopped');
});
