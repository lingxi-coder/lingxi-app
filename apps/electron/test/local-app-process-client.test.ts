import assert from 'node:assert/strict';
import test from 'node:test';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { LocalAppProcessClient } from '../src/main/local-apps/processClient.js';

const fixture = resolve('test/fixtures/local-app-process-helper.mjs');
function client(mode: string, output: Array<[string, string]> = []) {
  return new LocalAppProcessClient({ executable: process.execPath, helperArgs: [fixture],
    command: 'fixture', args: [mode], cwd: process.cwd(), env: { PATH: process.env.PATH ?? '' },
    onOutput: (stream, text) => output.push([stream, text]) });
}

test('process helper keeps streams separate and waits for verified natural cleanup', { timeout: 5000 }, async () => {
  const output: Array<[string, string]> = [], process = client('natural', output);
  await process.waitStarted();
  assert.deepEqual(await process.waitExit(), { code: 0, signal: null });
  assert.equal(output.filter(([stream]) => stream === 'stdout').map(([, text]) => text).join(''), '中\n');
  assert.equal(output.filter(([stream]) => stream === 'stderr').map(([, text]) => text).join(''), 'error: {"type":"cleanup-result","verified":true}\n');
  await process.stop();
});

test('process helper retries the same owned tree after an unverified stop', { timeout: 5000 }, async () => {
  const process = client('retry'); await process.waitStarted();
  await assert.rejects(process.stop(), /controlled failure/);
  await process.stop();
  assert.deepEqual(await process.waitExit(), { code: 1, signal: null });
});

test('process helper never trusts a mismatched attempt or a crashed transport', { timeout: 5000 }, async () => {
  const badAttempt = client('wrong-attempt'); await badAttempt.waitStarted();
  await assert.rejects(badAttempt.stop(), /unverified|invalid/i);
  const crashed = client('crash'); await crashed.waitStarted();
  await assert.rejects(crashed.waitExit(), /unverified/i);
  await assert.rejects(crashed.stop(), /unverified/i);
});

test('process helper rejects forged or malformed payload frames', { timeout: 5000 }, async () => {
  const process = client('invalid-base64'); await process.waitStarted();
  await assert.rejects(process.waitExit(), /unverified|invalid/i);
  await assert.rejects(process.stop(), /unverified|invalid/i);
});

test('startup error only releases ownership when no child is proven', { timeout: 5000 }, async () => {
  const absent = client('no-child');
  await assert.rejects(absent.waitStarted(), /could not spawn/);
  await absent.stop();
  const uncertain = client('uncertain-start');
  await assert.rejects(uncertain.waitStarted(), /cannot verify/);
  await uncertain.stop();
});

test('Windows environment and executable lookup follow the intended spawn contract', async () => {
  const { windowsProcessEnvironment, resolveWindowsProgram } = await import('../src/main/local-apps/processClient.js');
  const env = windowsProcessEnvironment({ Path: 'ignored', PATH: '"C:\\Program Files\\Git\\bin";tools', SystemRoot: 'C:\\Windows', SYSTEMROOT: 'C:\\Other', SECRET: undefined });
  assert.equal(env.PATH, '"C:\\Program Files\\Git\\bin";tools');
  assert.equal(env.Path, undefined);
  assert.equal(env.SYSTEMROOT, 'C:\\Other');
  assert.equal(env.SystemRoot, undefined);
  assert.equal(env.SECRET, undefined);
  const seen: string[] = [];
  const exists = async (candidate: string) => { seen.push(candidate); return candidate === 'C:\\Program Files\\Git\\bin\\bash.exe'; };
  assert.equal(await resolveWindowsProgram('bash.exe', 'C:\\app', env, exists, false), 'C:\\Program Files\\Git\\bin\\bash.exe');
  assert.deepEqual(seen, ['C:\\Program Files\\Git\\bin\\bash.exe']);
  const direct = async (candidate: string) => candidate === 'D:\\custom\\shell.exe';
  assert.equal(await resolveWindowsProgram('D:\\custom\\shell.exe', 'C:\\app', env, direct, true), 'D:\\custom\\shell.exe');
  await assert.rejects(resolveWindowsProgram('missing.exe', 'C:\\app', env, async () => false, true), /not found/i);
});

test('invalid identity, premature proof, clean helper exit, and oversized frames remain unverified', { timeout: 5000 }, async () => {
  for (const mode of ['wrong-id', 'premature-proof', 'clean-close', 'oversized']) {
    const process = client(mode);
    await process.waitStarted();
    await assert.rejects(process.waitExit(), /unverified|invalid/i, mode);
    await assert.rejects(process.stop(), /unverified|invalid/i, mode);
  }
});

test('stopping before helper admission never sends the start request', { timeout: 5000 }, async () => {
  const process = client('retry');
  await process.stop();
  await assert.rejects(process.waitStarted(), /cancelled/);
  assert.equal(process.started, false);
});

test('bounded output processing still admits a stop while the helper streams', { timeout: 5000 }, async () => {
  const output: Array<[string, string]> = [], process = client('flood', output);
  await process.waitStarted();
  await new Promise(resolve => setTimeout(resolve, 30));
  await process.stop();
  assert.ok(output.length > 0);
  assert.deepEqual(await process.waitExit(), { code: 1, signal: null });
});

test('a burst of output yields before dispatching stop', { timeout: 8000 }, async () => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-output-burst-'));
  const marker = join(root, 'stop-accepted');
  let outputs = 0;
  const process = new LocalAppProcessClient({ executable: globalThis.process.execPath, helperArgs: [fixture],
    command: 'fixture', args: ['burst'], cwd: globalThis.process.cwd(), env: { TEST_STOP_MARKER: marker },
    onOutput: () => { outputs++; const until = Date.now() + 2; while (Date.now() < until) { /* controlled slow sink */ } } });
  let stopping: Promise<void> | undefined;
  try {
    await process.waitStarted();
    const outputsAtStop = outputs;
    stopping = process.stop();
    assert.ok(outputsAtStop <= 31, 'startup must yield after a bounded batch of output frames');
    for (;;) { try { await fs.access(marker); break; } catch { await new Promise(resolve => setTimeout(resolve, 10)); } }
    await stopping;
  } finally { await stopping?.catch(() => undefined); await fs.rm(root, { recursive: true, force: true }); }
});

test('stop during blocked startup retains ownership and can retry after startup fails', { timeout: 5000 }, async () => {
  const process = client('blocked-start');
  const startup = process.waitStarted();
  await new Promise(resolve => setTimeout(resolve, 50));
  await assert.rejects(process.stop(), /startup is unverified/);
  await assert.rejects(startup, /startup cancelled/);
  await process.stop();
  await assert.rejects(process.waitExit(), /startup cancelled/);
});

test('stop reaches the helper before its controlled output stream completes', { timeout: 5000 }, async () => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-stop-barrier-'));
  const marker = join(root, 'stop-accepted');
  let outputs = 0;
  const process = new LocalAppProcessClient({ executable: globalThis.process.execPath, helperArgs: [fixture],
    command: 'fixture', args: ['stop-barrier'], cwd: globalThis.process.cwd(), env: { TEST_STOP_MARKER: marker },
    onOutput: () => { outputs++; } });
  try {
    await process.waitStarted();
    await process.stop();
    assert.equal(await fs.readFile(marker, 'utf8'), 'accepted-before-output-complete');
    assert.equal(outputs, 801, 'all output must drain before the verified receipt completes');
  } finally { await process.stop().catch(() => undefined); await fs.rm(root, { recursive: true, force: true }); }
});

test('more than 128 unverified stops remain retryable', { timeout: 8000 }, async () => {
  const process = client('retry-many');
  await process.waitStarted();
  try {
    for (let n = 0; n < 140; n++) await assert.rejects(process.stop(), /controlled failure/, `attempt ${n + 1}`);
    await process.stop();
    assert.deepEqual(await process.waitExit(), { code: 1, signal: null });
  } finally {
    const helper = Reflect.get(process, 'helper') as import('node:child_process').ChildProcessWithoutNullStreams;
    if (helper.exitCode === null && helper.signalCode === null) { const closed = new Promise<void>(resolve => helper.once('close', () => resolve())); helper.stdin.end(); await closed; }
  }
});

test('a late verified response for an old attempt cannot validate a newer stop', { timeout: 5000 }, async () => {
  const process = client('late-proof');
  await process.waitStarted();
  try {
    await assert.rejects(process.stop(), /controlled failure/);
    await assert.rejects(process.stop(), /still unverified/);
    await process.stop();
    assert.deepEqual(await process.waitExit(), { code: 1, signal: null });
  } finally {
    const helper = Reflect.get(process, 'helper') as import('node:child_process').ChildProcessWithoutNullStreams;
    if (helper.exitCode === null && helper.signalCode === null) { const closed = new Promise<void>(resolve => helper.once('close', () => resolve())); helper.stdin.end(); await closed; }
  }
});

test('a diagnostic stderr read error cannot crash the owning runtime', { timeout: 5000 }, async () => {
  const diagnostics: string[] = [];
  const process = new LocalAppProcessClient({ executable: globalThis.process.execPath, helperArgs: [fixture],
    command: 'fixture', args: ['retry'], cwd: globalThis.process.cwd(), env: {}, onOutput: () => undefined,
    onDiagnostic: message => diagnostics.push(message) });
  await process.waitStarted();
  try {
    const helper = Reflect.get(process, 'helper') as import('node:child_process').ChildProcessWithoutNullStreams;
    assert.doesNotThrow(() => helper.stderr.emit('error', new Error('controlled stderr read failure')));
    assert.ok(diagnostics.some(message => message.includes('controlled stderr read failure')));
    await assert.rejects(process.stop(), /controlled failure/);
    await process.stop();
  } finally {
    const helper = Reflect.get(process, 'helper') as import('node:child_process').ChildProcessWithoutNullStreams;
    if (helper.exitCode === null && helper.signalCode === null) { const closed = new Promise<void>(resolve => helper.once('close', () => resolve())); helper.stdin.end(); await closed; }
  }
});

test('a late receipt after an actual stop deadline cannot validate the next attempt', { timeout: 5000 }, async t => {
  const process = client('timeout-late'); await process.waitStarted();
  try {
    t.mock.timers.enable({ apis: ['setTimeout'] });
    const expired = process.stop();
    t.mock.timers.tick(25_000);
    await assert.rejects(expired, /did not confirm stop/);
    t.mock.timers.reset();
    await assert.rejects(process.stop(), /still unverified/);
    await process.stop();
    assert.deepEqual(await process.waitExit(), { code: 1, signal: null });
  } finally {
    t.mock.timers.reset();
    const helper = Reflect.get(process, 'helper') as import('node:child_process').ChildProcessWithoutNullStreams;
    if (helper.exitCode === null && helper.signalCode === null) { const closed = new Promise<void>(resolve => helper.once('close', () => resolve())); helper.stdin.end(); await closed; }
  }
});

test('invalid current receipt immediately rejects its waiting stop', { timeout: 5000 }, async t => {
  const process = client('premature-attempt'); await process.waitStarted();
  t.mock.timers.enable({ apis: ['setTimeout'] });
  let settled = false, result: unknown;
  const failed = new Promise<Error>(resolve => process.onFailure(resolve));
  const stop = process.stop().then(() => { settled = true; }, error => { settled = true; result = error; });
  try {
    const failure = await failed;
    assert.match(failure.message, /precedes leader wait/);
    await new Promise<void>(resolve => setImmediate(resolve));
    assert.equal(settled, true, 'fatal protocol failure must reject the active stop without waiting for its deadline');
    assert.match(String(result), /precedes leader wait/);
  } finally {
    t.mock.timers.tick(25_000); t.mock.timers.reset();
    await stop;
    const helper = Reflect.get(process, 'helper') as import('node:child_process').ChildProcessWithoutNullStreams;
    if (helper.exitCode === null && helper.signalCode === null) { const closed = new Promise<void>(resolve => helper.once('close', () => resolve())); helper.stdin.end(); await closed; }
  }
});

test('verified startup cleanup releases ownership without fabricating a process result', { timeout: 5000 }, async () => {
  for (const mode of ['startup-clean', 'startup-clean-pending']) {
    const process = client(mode);
    await assert.rejects(process.waitStarted(), /startup failed/);
    await process.stop();
    await assert.rejects(process.waitExit(), /startup failed/);
    assert.equal(process.started, false);
    assert.equal(process.hasExited, false);
  }
});

test('startup cleanup proof rejects reordered, duplicated, forged, or contradictory frames', { timeout: 5000 }, async () => {
  for (const mode of ['startup-clean-early', 'startup-clean-after-start', 'startup-clean-wrong-id', 'startup-clean-duplicate', 'startup-clean-nochild']) {
    const process = client(mode);
    try {
      await process.waitStarted().catch(() => undefined);
      await assert.rejects(process.stop(), /unverified|invalid/i, mode);
    } finally {
      const helper = Reflect.get(process, 'helper') as import('node:child_process').ChildProcessWithoutNullStreams;
      if (helper.exitCode === null && helper.signalCode === null) { const closed = new Promise<void>(resolve => helper.once('close', () => resolve())); helper.stdin.end(); await closed; }
    }
  }
});

test('late control errors cannot revoke startup cleanup proof but helper close is still required', { timeout: 8000 }, async () => {
  for (const mode of ['startup-clean-hold', 'startup-clean-pending-hold']) {
    const root = await fs.mkdtemp(join(tmpdir(), 'local-app-proof-control-'));
    const marker = join(root, 'release'); const diagnostics: string[] = [];
    const process = new LocalAppProcessClient({ executable: globalThis.process.execPath, helperArgs: [fixture], command: 'fixture', args: [mode],
      cwd: globalThis.process.cwd(), env: { TEST_CLOSE_MARKER: marker }, onOutput: () => undefined, onDiagnostic: text => diagnostics.push(text) });
    let stopping: Promise<void> | undefined;
    try {
      await assert.rejects(process.waitStarted(), /startup failed/);
      const helper = Reflect.get(process, 'helper') as import('node:child_process').ChildProcessWithoutNullStreams;
      let pendingWrite: ((error?: Error | null) => void) | undefined;
      if (mode === 'startup-clean-pending-hold') {
        const write = helper.stdin.write.bind(helper.stdin);
        Reflect.set(helper.stdin, 'write', (data: string, callback: (error?: Error | null) => void) => {
          pendingWrite = callback; return write(data, () => undefined);
        });
      }
      let settled = false;
      stopping = process.stop().then(() => { settled = true; }, error => { settled = true; throw error; });
      void stopping.catch(() => undefined);
      for (let n = 0; n < 3000 && !Reflect.get(process, 'cleanupVerified'); n++) await new Promise<void>(resolve => setImmediate(resolve));
      assert.equal(Reflect.get(process, 'cleanupVerified'), true, 'helper must first provide the matching proof');
      if (pendingWrite) pendingWrite(new Error('controlled EPIPE callback'));
      else helper.stdin.emit('error', new Error('controlled EPIPE event'));
      await new Promise<void>(resolve => setImmediate(resolve));
      assert.equal(settled, false, 'verified cleanup must still wait for helper close');
      await fs.writeFile(marker, 'release');
      await stopping;
      assert.ok(diagnostics.some(text => text.includes('controlled EPIPE')));
      await assert.rejects(process.waitExit(), /startup failed/);
    } finally {
      await fs.writeFile(marker, 'release');
      await stopping?.catch(() => undefined);
      await fs.rm(root, { recursive: true, force: true });
    }
  }
});

test('control error before any cleanup proof remains unverified', { timeout: 5000 }, async () => {
  const process = client('retry'); await process.waitStarted();
  const helper = Reflect.get(process, 'helper') as import('node:child_process').ChildProcessWithoutNullStreams;
  helper.stdin.emit('error', new Error('controlled pre-proof EPIPE'));
  await assert.rejects(process.stop(), /unverified.*pre-proof EPIPE/i);
});

test('a bound natural cleanup proof completes an in-flight stop after control loss', { timeout: 8000 }, async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-natural-proof-')); const marker = join(root, 'release');
  const process = new LocalAppProcessClient({ executable: globalThis.process.execPath, helperArgs: [fixture], command: 'fixture', args: ['natural-proof-hold'],
    cwd: globalThis.process.cwd(), env: { TEST_CLOSE_MARKER: marker }, onOutput: () => undefined });
  await process.waitStarted();
  t.mock.timers.enable({ apis: ['setTimeout'] });
  let timersActive = true;
  const stopping = process.stop();
  void stopping.catch(() => undefined);
  try {
    for (let n = 0; n < 3000 && !Reflect.get(process, 'cleanupVerified'); n++) await new Promise<void>(resolve => setImmediate(resolve));
    assert.equal(Reflect.get(process, 'cleanupVerified'), true);
    const helper = Reflect.get(process, 'helper') as import('node:child_process').ChildProcessWithoutNullStreams;
    helper.stdin.emit('error', new Error('controlled EPIPE'));
    await new Promise<void>(resolve => setImmediate(resolve));
    assert.equal(helper.stdin.writableEnded, true, 'a valid owner proof must close control even while a stop reply was pending');
    await fs.writeFile(marker, 'release');
    t.mock.timers.reset(); timersActive = false;
    await stopping;
    assert.deepEqual(await process.waitExit(), { code: 0, signal: null });
  } finally {
    if (timersActive) { t.mock.timers.tick(25_000); t.mock.timers.reset(); }
    const helper = Reflect.get(process, 'helper') as import('node:child_process').ChildProcessWithoutNullStreams;
    await fs.writeFile(marker, 'release'); helper.stdin.end();
    await stopping.catch(() => undefined);
    if (helper.exitCode === null && helper.signalCode === null) await new Promise<void>(resolve => helper.once('close', () => resolve()));
    await fs.rm(root, { recursive: true, force: true });
  }
});
