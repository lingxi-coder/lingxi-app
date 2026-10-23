import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import react from '@vitejs/plugin-react';
import { createServer } from 'vite';

test('storage recovery notice retries safely in a real Electron renderer', async () => {
  const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
  const temporary = mkdtempSync(join(tmpdir(), 'lingxi-storage-recovery-'));
  const vite = await createServer({ root: join(root, 'test/fixtures'), cacheDir: join(temporary, 'cache'), configFile: false,
    logLevel: 'error', plugins: [react()], server: { host: '127.0.0.1', port: 0, hmr: false },
    optimizeDeps: { entries: [join(root, 'test/fixtures/session-storage-fixture.html')] },
    resolve: { alias: { '@renderer': join(root, 'src/renderer') }, dedupe: ['react', 'react-dom'] } });
  let child;
  try {
    await vite.listen();
    const address = vite.httpServer.address();
    const output = [], errors = [];
    child = spawn(process.execPath, [join(root, 'node_modules/electron/cli.js'), join(root, 'test/fixtures/session-storage-electron.mjs'),
      `http://127.0.0.1:${address.port}/session-storage-fixture.html`], {
      cwd: root, env: { ...process.env, LINGXI_TEST_USER_DATA: join(temporary, 'profile') }, stdio: ['ignore', 'pipe', 'pipe'],
    });
    child.stdout.on('data', (data) => output.push(String(data)));
    child.stderr.on('data', (data) => errors.push(String(data)));
    const result = await new Promise((resolveResult, reject) => {
      const timeout = setTimeout(() => { child.kill('SIGTERM'); reject(new Error(errors.join('') || 'storage fixture timed out')); }, 25_000);
      child.once('error', (error) => { clearTimeout(timeout); reject(error); });
      child.once('exit', (code) => {
        clearTimeout(timeout);
        if (code !== 0) { reject(new Error(errors.join(''))); return; }
        try { resolveResult(JSON.parse(output.join('').trim().split('\n').at(-1))); } catch (error) { reject(error); }
      });
    });
    assert.equal(result.failedRetry, 'pause retained');
    assert.equal(result.successfulRecovery, 'original history retained');
    assert.equal(result.narrowOverflow, false);
  } finally {
    if (child && child.exitCode === null) child.kill('SIGTERM');
    await vite.close();
    rmSync(temporary, { recursive: true, force: true });
  }
});
