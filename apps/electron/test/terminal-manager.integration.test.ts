import { test } from 'node:test';
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import assert from 'node:assert/strict';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, win32 } from 'node:path';
import { StringDecoder } from 'node:string_decoder';
import { TerminalManager, terminalEnvironment } from '../src/main/terminal';
const serverBin = process.env.LINGXI_TERMINAL_TEST_BIN;
async function until(check: () => boolean, description: string, timeout = 12_000): Promise<void> {
  const deadline = Date.now() + timeout;
  while (!check()) { if (Date.now() >= deadline) throw new Error(`Timed out: ${description}`); await new Promise(resolve => setTimeout(resolve, 20)); }
}
function alive(pid: number): boolean {
  try { process.kill(pid, 0); return true; } catch (error) { if ((error as NodeJS.ErrnoException).code === 'ESRCH') return false; throw error; }
}
function observeBroker(child: ChildProcessWithoutNullStreams) {
  let stderr = ''; let closed = false; let spawnError: Error | undefined;
  child.stderr.on('data', chunk => { stderr = (stderr + String(chunk)).slice(-16_384); });
  child.once('error', error => { spawnError = error; });
  child.once('close', () => { closed = true; });
  return {
    failed: () => Boolean(spawnError) || child.exitCode !== null || child.signalCode !== null,
    async describe(): Promise<string> {
      if (!closed) await new Promise<void>(resolve => {
        const timeout = setTimeout(resolve, 500);
        child.once('close', () => { clearTimeout(timeout); resolve(); });
      });
      return `broker exit code=${child.exitCode ?? 'none'} signal=${child.signalCode ?? 'none'} spawnError=${spawnError?.message ?? 'none'} stderr=${JSON.stringify(stderr)}`;
    },
  };
}
async function verifyRawBrokerEof(mode: 'live' | 'exited', binary: string, spawnProcess: typeof spawn = spawn): Promise<void> {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), 'terminal-eof-integration-')));
  const home = join(directory, 'home'), appdata = join(directory, 'appdata'), localappdata = join(directory, 'localappdata'), temp = join(directory, 'tmp');
  for (const folder of [home, appdata, localappdata, temp]) mkdirSync(folder);
  const env = terminalEnvironment(process.env);
  Object.assign(env, { HOME: home, USERPROFILE: home, APPDATA: appdata, LOCALAPPDATA: localappdata, TMPDIR: temp, TMP: temp, TEMP: temp });
  if (process.platform === 'win32') {
    const homeRoot = win32.parse(home).root;
    assert.ok(homeRoot, 'isolated Windows home must have an absolute root');
    env.HOMEDRIVE = homeRoot.replace(/[\\/]+$/, '');
    env.HOMEPATH = home.slice(env.HOMEDRIVE.length) || '\\';
  } else env.SHELL = '/bin/sh';
  const child = spawnProcess(binary, ['--desktop-terminal'], { env, stdio: ['pipe', 'pipe', 'pipe'] }) as ChildProcessWithoutNullStreams;
  const observation = observeBroker(child);
  const decoder = new StringDecoder('utf8');
  const frames: Array<Record<string, unknown>> = [];
  let buffer = ''; let terminalOutput = ''; let inputError: Error | undefined; let frameError: Error | undefined; let closed = false; let outputEnded = false;
  child.stdin.on('error', error => { inputError = error; });
  child.once('close', () => { closed = true; });
  child.stdout.on('data', chunk => {
    buffer += decoder.write(chunk); let newline: number;
    while ((newline = buffer.indexOf('\n')) >= 0) {
      const line = buffer.slice(0, newline); buffer = buffer.slice(newline + 1);
      try {
        const frame: unknown = JSON.parse(line);
        if (!frame || typeof frame !== 'object' || Array.isArray(frame)) throw new Error('invalid broker frame');
        if ((frame as Record<string, unknown>).kind === 'output' && (frame as Record<string, unknown>).terminalId === 'eof-terminal') {
          terminalOutput = (terminalOutput + String((frame as Record<string, unknown>).data ?? '')).slice(-4096);
        }
        if (['response', 'exit', 'shutdown'].includes(String((frame as Record<string, unknown>).kind))) frames.push(frame as Record<string, unknown>);
      } catch (error) { frameError = error as Error; }
    }
  });
  child.stdout.once('end', () => {
    outputEnded = true; buffer += decoder.end();
    if (buffer.trim()) frameError = new Error('incomplete broker output frame');
  });
  const exited = () => frames.some(frame => frame.kind === 'exit' && frame.terminalId === 'eof-terminal');
  const send = (requestId: string, kind: string, fields: Record<string, unknown>) => {
    child.stdin.write(JSON.stringify({ requestId, terminalId: 'eof-terminal', kind, ...fields }) + '\n');
  };
  const response = async (requestId: string) => {
    await until(() => frames.some(frame => frame.kind === 'response' && frame.requestId === requestId) || Boolean(inputError || frameError) || observation.failed(), `broker ${requestId} response`);
    if (inputError || frameError) throw inputError ?? frameError;
    const result = frames.find(frame => frame.kind === 'response' && frame.requestId === requestId);
    assert.ok(result, `broker must answer ${requestId}`);
    return result;
  };
  const requireAcceptedInput = async () => {
    send('eof-live', 'input', { data: process.platform === 'win32' ? '\r' : '\n' });
    const result = await response('eof-live');
    if (result.error) throw new Error(`broker rejected input before EOF: ${String(result.error)}`);
    if (exited()) throw new Error('broker observed terminal exit before EOF');
  };
  try {
    send('eof-create', 'create', { cwd: directory, cols: 80, rows: 24 });
    const created = await response('eof-create');
    assert.ok(!created.error, `broker must create a terminal before EOF: ${String(created.error)}`);
    if (mode === 'exited') {
      send('eof-exit', 'input', { data: process.platform === 'win32' ? 'exit 0\r' : 'exit 0\n' });
      const exitInput = await response('eof-exit');
      assert.ok(!exitInput.error, `broker must accept shell exit input: ${String(exitInput.error)}`);
      await until(() => exited() || Boolean(inputError || frameError) || observation.failed(), 'shell exit before negative liveness probe');
      if (inputError || frameError) throw inputError ?? frameError;
      assert.ok(exited(), 'shell must report its exit before the negative probe');
      assert.equal(frames.find(frame => frame.kind === 'exit' && frame.terminalId === 'eof-terminal')?.exitCode, 0,
        'shell must actually exit with the requested status');
      send('eof-dead', 'input', { data: process.platform === 'win32' ? '\r' : '\n' });
      const rejected = await response('eof-dead');
      assert.equal(typeof rejected.error, 'string', 'broker must reject input after shell exit');
      assert.match(rejected.error as string, /terminal has exited|unknown terminal/);
    } else {
      // The input response follows Rust's has_exited check and input enqueue;
      // it does not prove the OS process is still alive or consumed the input.
      await requireAcceptedInput();
    }
    child.stdin.end();
    await until(() => closed, 'broker close after EOF');
    if (inputError || frameError) throw inputError ?? frameError;
    assert.equal(outputEnded, true, 'broker output must end before close');
    assert.equal(child.exitCode, 0); assert.equal(child.signalCode, null);
    assert.equal(frames.filter(frame => frame.kind === 'shutdown' && frame.reason === 'eof').length, 1);
  } catch (error) {
    throw new Error(`${error instanceof Error ? error.message : String(error)}\n${await observation.describe()} terminalOutput=${JSON.stringify(terminalOutput)}`, { cause: error });
  } finally {
    if (!closed) await new Promise<void>(resolve => {
      const timeout = setTimeout(resolve, 2_000);
      child.once('close', () => { clearTimeout(timeout); resolve(); });
      try { child.kill('SIGKILL'); } catch { clearTimeout(timeout); resolve(); }
    });
    child.stdin.destroy(); child.stdout.destroy(); child.stderr.destroy();
    rmSync(directory, { recursive: true, force: true });
  }
}
test('raw EOF probe checks accepted input and rejects corrupt trailing frames', { timeout: 20_000 }, async t => {
  const directory = mkdtempSync(join(tmpdir(), 'terminal-eof-double-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const fixture = join(directory, 'broker.cjs');
  writeFileSync(fixture, String.raw`
const readline = require('node:readline');
const mode = process.argv[2];
let exited = false;
const io = readline.createInterface({ input: process.stdin });
const frame = value => JSON.stringify(value) + '\n';
io.on('line', line => {
  const request = JSON.parse(line);
  const response = { kind: 'response', requestId: request.requestId, terminalId: request.terminalId, error: null };
  if (request.kind === 'input' && request.data.trim() === 'exit 0') {
    process.stdout.write(frame(response) + frame({ kind: 'exit', terminalId: request.terminalId, exitCode: 0 })); exited = true;
  } else if (request.kind === 'input' && exited) {
    if (mode !== 'accept-after-exit') response.error = 'terminal has exited'; process.stdout.write(frame(response));
  } else if (mode === 'dead' && request.requestId === 'eof-live') {
    process.stdout.write(frame(response) + frame({ kind: 'exit', terminalId: request.terminalId, exitCode: 0 })); exited = true;
  } else process.stdout.write(frame(response));
});
io.on('close', () => {
  process.stdout.write(frame({ kind: 'shutdown', reason: 'eof' }));
  if (mode === 'bad-json') process.stdout.write('{"bad":}\n');
  if (mode === 'partial') process.stdout.write('{"partial":');
  if (mode === 'partial-utf8') process.stdout.write(Buffer.from([0xe4, 0xb8]));
});
`);
  const spawner = (variant: string) => ((_binary: string, _args: string[], options: Parameters<typeof spawn>[2]) => spawn(process.execPath, [fixture, variant], options)) as typeof spawn;
  await verifyRawBrokerEof('live', '/fake', spawner('good'));
  await verifyRawBrokerEof('exited', '/fake', spawner('good'));
  await assert.rejects(verifyRawBrokerEof('exited', '/fake', spawner('accept-after-exit')), /broker must reject input after shell exit/);
  await assert.rejects(verifyRawBrokerEof('live', '/fake', spawner('dead')), /broker observed terminal exit before EOF/);
  await assert.rejects(verifyRawBrokerEof('live', '/fake', spawner('bad-json')), /Unexpected|JSON/);
  for (const variant of ['partial', 'partial-utf8']) {
    await assert.rejects(verifyRawBrokerEof('live', '/fake', spawner(variant)), /incomplete broker output frame/);
  }
});
if (serverBin) {
  test('raw Rust broker acknowledges real stdin EOF after terminal cleanup', { timeout: 45_000 }, () => verifyRawBrokerEof('live', serverBin));
  test('raw Rust broker rejects input probe after shell exit', { timeout: 45_000 }, () => verifyRawBrokerEof('exited', serverBin));
}
test('real Rust broker: cwd, UTF-8, control, resize, isolation, migration and cleanup', {
  skip: !serverBin || process.platform === 'win32' ? 'Set LINGXI_TERMINAL_TEST_BIN to freshly built bridge-server (Unix checks)' : false,
  timeout: 60_000,
}, async () => {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), 'lingxi-terminal-integration-')));
  const home = process.env.HOME; const sentinel = process.env.LINGXI_TERMINAL_TEST_SECRET;
  process.env.HOME = directory; process.env.LINGXI_TERMINAL_TEST_SECRET = 'safe-test-sentinel';
  let observation: ReturnType<typeof observeBroker> | undefined;
  const manager = new TerminalManager({ serverBin, spawnProcess: ((binary: string, args: string[], options: Parameters<typeof spawn>[2]) => {
    const child = spawn(binary, args, options) as ChildProcessWithoutNullStreams;
    observation = observeBroker(child);
    return child;
  }) as typeof spawn });
  const scope = { projectPath: directory, sessionId: '__draft__' };
  let jobPid: number | undefined;
  try {
    const a = await manager.create(scope); const b = await manager.create({ ...scope, sessionId: 'other-session' });
    const output = (id: string) => manager.get(id)?.output ?? '';
    await manager.input(a.id, "printf '__LX_%s__\\n' READY\n");
    await until(() => output(a.id).includes('__LX_READY__'), 'login shell ready');
    await manager.input(a.id, "printf '__LX_CWD__%s\\n' \"$PWD\"; printf '__LX_ENV__%s\\n' \"${LINGXI_TERMINAL_TEST_SECRET-unset}\"; printf '__LX_%s__\\n' '中文😀'\n");
    await until(() => output(a.id).includes('__LX_CWD__' + directory) && output(a.id).includes('__LX_ENV__unset') && output(a.id).includes('__LX_中文😀__'), 'cwd, excluded secret and Unicode');
    assert.ok(!output(b.id).includes('__LX_CWD__' + directory));
    writeFileSync(join(directory, 'terminal-completion-marker.txt'), '__LX_COMPLETION__\n');
    await manager.input(a.id, 'cat terminal-completion-mar\t\n');
    await until(() => output(a.id).includes('__LX_COMPLETION__'), 'native shell Tab completion');
    const vimFile = join(directory, 'terminal-vim.txt');
    const beforeVim = output(a.id).length;
    await manager.input(a.id, 'vim -Nu NONE -n terminal-vim.txt\n');
    await until(() => output(a.id).slice(beforeVim).includes('\u001b[?1049h') && /\[New(?: File)?\]/.test(output(a.id).slice(beforeVim)), 'vim alternate screen and new file');
    await manager.input(a.id, 'iNative中文\u001b:wq\r');
    await until(() => existsSync(vimFile) && readFileSync(vimFile, 'utf8') === 'Native中文\n', 'vim Unicode file save');
    await until(() => output(a.id).slice(beforeVim).includes('\u001b[?1049l'), 'vim restores normal screen');
    await manager.resize(a.id, 111, 31); await manager.input(a.id, "printf '__LX_SIZE__'; stty size\n");
    await until(() => /__LX_SIZE__31 111/.test(output(a.id)), 'PTY dimensions').catch(error => { throw new Error(error.message + ': ' + JSON.stringify(output(a.id).slice(-1000))); });
    await manager.input(a.id, "awk 'BEGIN { for(i=0;i<40000;i++) print \"0123456789012345678901234567890123456789012345678901234567890123\" }'; printf '__LX_%s__\\n' FLOOD_DONE\n");
    await until(() => output(a.id).includes('__LX_FLOOD_DONE__'), 'bounded high-volume output');
    assert.ok(output(a.id).length <= 1024 * 1024);
    const persisted = { ...scope, sessionId: 'persisted-session' };
    manager.migrateScope(scope, persisted);
    assert.equal(manager.list(scope).length, 0); assert.equal(manager.get(a.id)?.scope.sessionId, 'persisted-session');
    const destination = join(directory, 'moved workspace'); mkdirSync(destination);
    const moved = { ...persisted, projectPath: destination };
    manager.migrateScope(persisted, moved);
    assert.equal(manager.list(persisted).length, 0); assert.equal(manager.list(moved)[0]?.id, a.id);
    assert.deepEqual(manager.get(b.id)?.scope, { ...scope, sessionId: 'other-session' });
    const fresh = await manager.create(moved);
    await manager.input(a.id, "printf '__LX_PRESERVED_CWD__%s\\n' \"$PWD\"\n");
    await manager.input(fresh.id, "printf '__LX_MOVED_CWD__%s\\n' \"$PWD\"\n");
    await until(() => output(a.id).includes('__LX_PRESERVED_CWD__' + directory)
      && output(fresh.id).includes('__LX_MOVED_CWD__' + destination), 'relocated owner keeps its shell while new terminals use the new directory');
    await manager.close(fresh.id);
    await manager.input(a.id, "printf '__LX_%s__\\n' SLEEP; sleep 30\n");
    await until(() => output(a.id).includes('__LX_SLEEP__'), 'foreground process start');
    await manager.input(a.id, '\u0003'); await manager.input(a.id, "printf '__LX_%s__\\n' INTERRUPTED\n");
    await until(() => output(a.id).includes('__LX_INTERRUPTED__'), 'Ctrl+C recovers prompt');
    await manager.input(a.id, "sleep 30 & printf '__LX_PID__%s\\n' $!\n");
    await until(() => /__LX_PID__(\d+)/.test(output(a.id)), 'background job PID').catch(error => { throw new Error(error.message + ': ' + JSON.stringify(output(a.id).slice(-1600))); });
    jobPid = Number(output(a.id).match(/__LX_PID__(\d+)/)![1]); assert.ok(alive(jobPid));
    await manager.close(a.id); await until(() => !alive(jobPid!), 'closed terminal process tree'); jobPid = undefined;
    assert.equal(manager.get(a.id), undefined); assert.equal(manager.get(b.id)?.status, 'running');
    await manager.input(b.id, "(nohup sleep 30 >/dev/null 2>&1 & printf '__LX_ORPHAN__%s\\n' $! )\n");
    await until(() => /__LX_ORPHAN__(\d+)/.test(output(b.id)), 'detached background PID');
    jobPid = Number(output(b.id).match(/__LX_ORPHAN__(\d+)/)![1]);
    await manager.input(b.id, 'exit 7\n'); await until(() => manager.get(b.id)?.status === 'exited', 'shell exit'); assert.equal(manager.get(b.id)?.exitCode, 7);
    await manager.close(b.id); await until(() => !alive(jobPid!), 'closed exited terminal cleans orphaned job'); jobPid = undefined;
    // Exceed the broker's live-terminal cap cumulatively to detect exited-handle leaks.
    for (let i = 0; i < 34; i++) {
      const terminal = await manager.create({ ...scope, sessionId: 'restart-check' });
      await manager.input(terminal.id, 'exit 0\n');
      await until(() => manager.get(terminal.id)?.status === 'exited', 'restart exit');
      await manager.close(terminal.id);
    }
    const c = await manager.create({ ...scope, sessionId: 'shutdown-session' });
    await manager.input(c.id, "sleep 30 & printf '__LX_PID__%s\\n' $!\n");
    await until(() => /__LX_PID__(\d+)/.test(output(c.id)), 'shutdown job PID');
    jobPid = Number(output(c.id).match(/__LX_PID__(\d+)/)![1]);
    await manager.dispose(); await until(() => !alive(jobPid!), 'application shutdown process tree'); jobPid = undefined;
  } catch (error) {
    const diagnostic = observation ? await observation.describe() : 'broker was not spawned';
    throw new Error(`${error instanceof Error ? error.message : String(error)}\n${diagnostic}`, { cause: error });
  } finally {
    await manager.dispose(); if (jobPid && alive(jobPid)) process.kill(jobPid, 'SIGKILL');
    if (home === undefined) delete process.env.HOME; else process.env.HOME = home;
    if (sentinel === undefined) delete process.env.LINGXI_TERMINAL_TEST_SECRET; else process.env.LINGXI_TERMINAL_TEST_SECRET = sentinel;
    rmSync(directory, { recursive: true, force: true });
  }
});

test('raw Rust broker SIGTERM cleans jobs with stdin open and stdout backpressured', {
  skip: !serverBin || process.platform === 'win32' ? 'Requires opt-in Unix Rust broker' : false,
  timeout: 20_000,
}, async () => {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), 'lingxi-terminal-signal-')));
  const child = spawn(serverBin!, ['--desktop-terminal'], { env: terminalEnvironment({ ...process.env, HOME: directory }), stdio: ['pipe', 'pipe', 'pipe'] });
  const observation = observeBroker(child);
  let inputError: Error | undefined;
  child.stdin.on('error', error => { inputError = error; });
  let buffer = ''; let output = ''; let exited = false; let exitCode: number | null = null; let jobPid: number | undefined;
  child.on('exit', code => { exited = true; exitCode = code; });
  child.stdout.on('data', chunk => {
    buffer += chunk.toString(); let newline: number;
    while ((newline = buffer.indexOf('\n')) >= 0) {
      const message = JSON.parse(buffer.slice(0, newline)); buffer = buffer.slice(newline + 1);
      if (message.kind === 'output') output = (output + message.data).slice(-8192);
    }
  });
  const send = (kind: string, fields: Record<string, unknown>) => child.stdin.write(JSON.stringify({ requestId: kind, terminalId: 'signal-test', kind, ...fields }) + '\n');
  try {
    send('create', { cwd: directory, cols: 80, rows: 24 });
    send('input', { data: "sleep 30 & printf '__LX_PID__%s\\n' $!\n" });
    await until(() => /__LX_PID__(\d+)/.test(output) || Boolean(inputError) || observation.failed(), 'signal test background PID');
    if (!/__LX_PID__(\d+)/.test(output)) throw inputError ?? new Error('broker exited before starting the shell');
    jobPid = Number(output.match(/__LX_PID__(\d+)/)![1]);
    child.stdout.pause(); send('input', { data: 'yes x\n' });
    await new Promise(resolve => setTimeout(resolve, 250));
    assert.equal(child.stdin.writableEnded, false); child.kill('SIGTERM');
    await until(() => exited, 'SIGTERM while stdin open and output blocked', 6000);
    assert.equal(exitCode, 0); await until(() => !alive(jobPid!), 'signal cleanup of background process'); jobPid = undefined;
  } catch (error) {
    throw new Error(`${error instanceof Error ? error.message : String(error)}\n${await observation.describe()}`, { cause: error });
  } finally {
    if (!exited) child.kill('SIGKILL'); child.stdin.destroy(); child.stdout.destroy(); child.stderr.destroy();
    if (jobPid && alive(jobPid)) process.kill(jobPid, 'SIGKILL');
    rmSync(directory, { recursive: true, force: true });
  }
});
