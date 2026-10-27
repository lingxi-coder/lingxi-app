import test from 'node:test';
import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { LocalAppRuntime } from '../src/main/local-apps/runtime.js';
import { LocalAppStore } from './fixtures/local-app-store.js';

const helper = resolve('test/fixtures/local-app-process-helper.mjs');
async function setup(mode: string, service = false) {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-windows-helper-'));
  const store = new LocalAppStore(root);
  const runtime = new LocalAppRuntime(store, () => undefined, {
    environment: { PATH: 'C:\\safe', HOME: root, PROVIDER_SECRET: 'must-not-inherit' },
    processHelperExecutable: process.execPath,
    processHelperArguments: [helper],
    resolveWindowsExecutable: async (program, _cwd, env) => {
      assert.equal(program, 'bash.exe'); assert.equal(env.PROVIDER_SECRET, undefined);
      return 'C:\\safe\\bash.exe';
    },
  });
  const source = join(root, 'imports', 'source'); await fs.mkdir(source, { recursive: true });
  await fs.writeFile(join(source, 'index.html'), '<!doctype html>');
  const step = { command: mode, env: { APP_ALLOWED: 'yes' } };
  await fs.writeFile(join(source, 'lingxi-app.json'), JSON.stringify({ schema: 2, name: 'Windows helper',
    targets: [{ platform: 'macos', environment: 'native' }], entry: { type: 'static', path: 'index.html' },
    ...(service ? { processes: [{ ...step, name: 'fixture', ready: { output: 'ready', timeoutMs: 3000 } }] } : { install: [step] }) }));
  const row = await store.publish({ staged_path: source }); row.grants = ['process']; await store.save(row);
  return { root, runtime, row };
}

async function asWindows<T>(run: () => Promise<T>): Promise<T> {
  const original = Object.getOwnPropertyDescriptor(process, 'platform')!;
  try { Object.defineProperty(process, 'platform', { ...original, value: 'win32' }); return await run(); }
  finally { Object.defineProperty(process, 'platform', original); }
}

test('Windows runtime forwards exact allowed environment and completes a verified command', { skip: process.platform === 'win32', timeout: 8000 }, async () => {
  const { root, runtime, row } = await setup('inspect-env');
  try {
    await asWindows(async () => {
      await runtime.start(row.id);
      const payload = runtime.logs(row.id).find(line => line.process === 'installing' && line.stream === 'stdout')?.text;
      assert.ok(payload);
      const actual = JSON.parse(payload);
      assert.equal(actual.command, 'C:\\safe\\bash.exe');
      assert.equal(actual.env.APP_ALLOWED, 'yes');
      assert.equal(actual.env.PROVIDER_SECRET, undefined);
      await runtime.stop(row.id);
    });
  } finally { await runtime.dispose().catch(() => undefined); await fs.rm(root, { recursive: true, force: true }); }
});

test('Windows runtime retains an unverified service until a bound stop retry succeeds', { skip: process.platform === 'win32', timeout: 8000 }, async () => {
  const { root, runtime, row } = await setup('retry', true);
  try {
    await asWindows(async () => {
      await runtime.start(row.id);
      assert.equal(runtime.phase(row.id), 'running');
      await assert.rejects(runtime.stop(row.id), /cleanup needs retry/);
      assert.equal(runtime.phase(row.id), 'stopping');
      const current = (Reflect.get(runtime, 'invocations') as Map<string, { children: Set<unknown> }>).get(row.id)!;
      assert.equal(current.children.size, 1);
      await runtime.stop(row.id);
      assert.equal(runtime.phase(row.id), 'stopped');
      assert.equal(current.children.size, 0);
    });
  } finally { await runtime.dispose().catch(() => undefined); await fs.rm(root, { recursive: true, force: true }); }
});

test('Windows runtime keeps a crashed helper unverified instead of losing its owner', { skip: process.platform === 'win32', timeout: 8000 }, async () => {
  const { root, runtime, row } = await setup('crash', true);
  try {
    await asWindows(async () => {
      await runtime.start(row.id);
      for (let n = 0; n < 100 && runtime.phase(row.id) !== 'failed'; n++) await delay(20);
      assert.equal(runtime.phase(row.id), 'failed');
      await assert.rejects(runtime.stop(row.id), /cleanup needs retry/);
      const current = (Reflect.get(runtime, 'invocations') as Map<string, { children: Set<unknown> }>).get(row.id)!;
      assert.equal(current.children.size, 1);
      assert.equal(runtime.phase(row.id), 'stopping');
    });
  } finally { await runtime.dispose().catch(() => undefined); await fs.rm(root, { recursive: true, force: true }); }
});

test('Windows runtime can restart after a failed startup has a verified empty Job', { skip: process.platform === 'win32', timeout: 8000 }, async () => {
  const { root, runtime, row } = await setup('startup-clean');
  try {
    await asWindows(async () => {
      await assert.rejects(runtime.start(row.id), /startup failed/);
      assert.equal(runtime.phase(row.id), 'failed');
      const failed = (Reflect.get(runtime, 'invocations') as Map<string, { children: Set<unknown> }>).get(row.id)!;
      assert.equal(failed.children.size, 0);
      const source = join(root, 'imports', 'repaired'); await fs.mkdir(source, { recursive: true });
      await fs.writeFile(join(source, 'index.html'), '<!doctype html>');
      await fs.writeFile(join(source, 'lingxi-app.json'), JSON.stringify({ schema: 2, name: 'Repaired',
        targets: [{ platform: 'macos', environment: 'native' }], entry: { type: 'static', path: 'index.html' },
        install: [{ command: 'inspect-env' }] }));
      const repaired = await runtime.store.publish({ id: row.id, staged_path: source }); repaired.grants = ['process']; await runtime.store.save(repaired);
      await runtime.start(row.id);
      assert.equal(runtime.phase(row.id), 'running');
      await runtime.stop(row.id);
    });
  } finally { await runtime.dispose().catch(() => undefined); await fs.rm(root, { recursive: true, force: true }); }
});
