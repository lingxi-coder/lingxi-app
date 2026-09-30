import { test } from 'node:test';
import { spawn } from 'node:child_process';
import assert from 'node:assert/strict';
import { existsSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { TerminalManager, terminalEnvironment } from '../src/main/terminal';
const serverBin = process.env.LINGXI_TERMINAL_TEST_BIN;
async function until(check: () => boolean, description: string, timeout = 12_000): Promise<void> {
  const deadline = Date.now() + timeout;
  while (!check()) { if (Date.now() >= deadline) throw new Error(`Timed out: ${description}`); await new Promise(resolve => setTimeout(resolve, 20)); }
}
function alive(pid: number): boolean {
  try { process.kill(pid, 0); return true; } catch (error) { if ((error as NodeJS.ErrnoException).code === 'ESRCH') return false; throw error; }
}
test('real Rust broker: cwd, UTF-8, control, resize, isolation, migration and cleanup', {
  skip: !serverBin || process.platform === 'win32' ? 'Set LINGXI_TERMINAL_TEST_BIN to freshly built bridge-server (Unix checks)' : false,
  timeout: 60_000,
}, async () => {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), 'lingxi-terminal-integration-')));
  const home = process.env.HOME; const sentinel = process.env.LINGXI_TERMINAL_TEST_SECRET;
  process.env.HOME = directory; process.env.LINGXI_TERMINAL_TEST_SECRET = 'safe-test-sentinel';
  const manager = new TerminalManager({ serverBin });
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
    manager.migrateScope(scope, { ...scope, sessionId: 'persisted-session' });
    assert.equal(manager.list(scope).length, 0); assert.equal(manager.get(a.id)?.scope.sessionId, 'persisted-session');
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
  child.stderr.resume();
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
    await until(() => /__LX_PID__(\d+)/.test(output), 'signal test background PID');
    jobPid = Number(output.match(/__LX_PID__(\d+)/)![1]);
    child.stdout.pause(); send('input', { data: 'yes x\n' });
    await new Promise(resolve => setTimeout(resolve, 250));
    assert.equal(child.stdin.writableEnded, false); child.kill('SIGTERM');
    await until(() => exited, 'SIGTERM while stdin open and output blocked', 6000);
    assert.equal(exitCode, 0); await until(() => !alive(jobPid!), 'signal cleanup of background process'); jobPid = undefined;
  } finally {
    if (!exited) child.kill('SIGKILL'); child.stdin.destroy(); child.stdout.destroy(); child.stderr.destroy();
    if (jobPid && alive(jobPid)) process.kill(jobPid, 'SIGKILL');
    rmSync(directory, { recursive: true, force: true });
  }
});
