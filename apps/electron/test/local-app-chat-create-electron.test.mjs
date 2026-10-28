import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { build } from 'esbuild';

const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
test('real Electron persists Chat receipts and preserves IPC error codes through reload', { timeout: 60000 }, async () => {
  const temp = await mkdtemp(join(tmpdir(), 'lingxi-chat-electron-'));
  const bundle = (entry, name) => build({ entryPoints: [join(root, entry)], outfile: join(temp, name), bundle: true, platform: 'node', format: 'cjs', external: ['electron'] });
  await Promise.all([bundle('test/fixtures/local-app-chat-create-electron.ts', 'main.cjs'), bundle('src/preload/localApp.ts', 'preload.cjs')]);
  let child;
  try {
    const env = { ...process.env, LINGXI_TEST_USER_DATA: temp }; delete env.ELECTRON_RUN_AS_NODE;
    child = spawn(createRequire(import.meta.url)('electron'), [join(temp, 'main.cjs')], { cwd: root, env, stdio: ['ignore', 'pipe', 'pipe'] });
    let out = '', err = '';
    child.stdout.on('data', chunk => { out += chunk; }); child.stderr.on('data', chunk => { err += chunk; });
    const result = await new Promise((resolveResult, reject) => {
      const timer = setTimeout(() => { child.kill(); reject(new Error(`Timed out: ${err}\n${out}`)); }, 50000);
      child.once('error', error => { clearTimeout(timer); reject(error); });
      child.once('exit', (code, signal) => { clearTimeout(timer); code === 0 ? resolveResult(JSON.parse(out.trim().split('\n').at(-1))) : reject(new Error(`Electron exited ${code} (${signal}): ${err}\n${out}`)); });
    });
    assert.equal(result.passed, true);
  } finally {
    if (child && child.exitCode === null) child.kill('SIGKILL');
    child?.stdout?.destroy(); child?.stderr?.destroy();
    await rm(temp, { recursive: true, force: true });
  }
});
