import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn, type ChildProcess } from 'node:child_process';
import { EventEmitter } from 'node:events';
import { LocalAppRuntime } from '../src/main/local-apps/runtime.ts';

const runtime = new LocalAppRuntime({} as ConstructorParameters<typeof LocalAppRuntime>[0], () => undefined);
const waitExit = (child: ChildProcess, signal: AbortSignal) =>
  (Reflect.get(runtime, 'waitExit') as (child: ChildProcess, signal: AbortSignal) => Promise<void>).call(runtime, child, signal);

async function closed(child: ChildProcess): Promise<void> {
  await new Promise<void>((resolve, reject) => { child.once('error', reject); child.once('close', () => resolve()); });
}

test('waitExit rejects a signal even after close was already delivered', { timeout: 5000 }, async () => {
  let child: ChildProcess;
  if (process.platform === 'win32') {
    child = Object.assign(new EventEmitter(), { exitCode: null, signalCode: 'SIGTERM' }) as unknown as ChildProcess;
  } else {
    child = spawn(process.execPath, ['-e', "process.kill(process.pid, 'SIGTERM')"]);
    await closed(child);
    assert.equal(child.signalCode, 'SIGTERM');
  }
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 1000);
  try { await assert.rejects(waitExit(child, controller.signal), /signal SIGTERM/); }
  finally { clearTimeout(timer); controller.abort(); }
});

test('waitExit reports a signal delivered after the listener was attached', { timeout: 5000 }, async () => {
  const child = Object.assign(new EventEmitter(), { exitCode: null, signalCode: null }) as unknown as ChildProcess;
  const controller = new AbortController();
  const pending = assert.rejects(waitExit(child, controller.signal), /signal SIGTERM/);
  child.emit('close', null, 'SIGTERM');
  await pending;
});

test('waitExit retains success and nonzero exit semantics after close', { timeout: 5000 }, async () => {
  for (const code of [0, 7]) {
    const child = spawn(process.execPath, ['-e', `process.exit(${code})`]);
    await closed(child);
    const controller = new AbortController();
    if (code === 0) await waitExit(child, controller.signal);
    else await assert.rejects(waitExit(child, controller.signal), /code 7/);
  }
});
