import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import react from '@vitejs/plugin-react';
import { createServer } from 'vite';

const electronRoot = resolve(fileURLToPath(new URL('..', import.meta.url)));
const fixtureRoot = join(electronRoot, 'test', 'fixtures');

test('real App preserves IME Escape, picker focus, originating command and pending resume approvals', async () => {
  const cacheDir = mkdtempSync(join(tmpdir(), 'lingxi-session-picker-vite-'));
  const userData = mkdtempSync(join(tmpdir(), 'lingxi-session-picker-electron-'));
  const vite = await createServer({
    root: fixtureRoot, cacheDir, configFile: false, logLevel: 'error',
    server: { host: '127.0.0.1', port: 0, strictPort: false, hmr: false }, plugins: [react()],
    resolve: { alias: { '@renderer': resolve(electronRoot, 'src/renderer') }, dedupe: ['react', 'react-dom'] },
  });
  let child;
  try {
    await vite.listen();
    const address = vite.httpServer?.address();
    assert.ok(address && typeof address === 'object' && address.port);
    const output = [], errors = [];
    child = spawn(process.execPath, [resolve(electronRoot, 'node_modules/electron/cli.js'),
      join(fixtureRoot, 'session-picker-electron.mjs'), `http://127.0.0.1:${address.port}/session-picker-fixture.html`], {
      cwd: electronRoot,
      env: { ...process.env, ELECTRON_ENABLE_LOGGING: '0', ELECTRON_IS_DEV: '0', LINGXI_TEST_USER_DATA: userData },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    child.stdout.on('data', chunk => output.push(String(chunk)));
    child.stderr.on('data', chunk => errors.push(String(chunk)));
    const result = await new Promise((resolveResult, rejectResult) => {
      const timeout = setTimeout(() => { child.kill('SIGTERM'); rejectResult(new Error(`Session picker fixture timed out\n${errors.join('')}`)); }, 60000);
      child.once('error', error => { clearTimeout(timeout); rejectResult(error); });
      child.once('exit', (code, signal) => {
        clearTimeout(timeout);
        const line = output.join('').trim().split('\n').at(-1);
        if (code !== 0 || !line) { rejectResult(new Error(`Session picker fixture exited ${code ?? signal}\n${errors.join('')}`)); return; }
        try { resolveResult(JSON.parse(line)); }
        catch (error) { rejectResult(new Error(`Invalid fixture output: ${line}`, { cause: error })); }
      });
    });
    assert.deepEqual(result, { imeEscape: true, ordinaryEscapeFocus: true, tabTrap: true, navigationFence: true, resumeApproval: true });
  } finally {
    if (child && child.exitCode === null) {
      child.kill('SIGTERM');
      await new Promise(resolveExit => {
        const timeout = setTimeout(resolveExit, 2000);
        child.once('exit', () => { clearTimeout(timeout); resolveExit(); });
      });
    }
    await vite.close();
    rmSync(cacheDir, { recursive: true, force: true });
    rmSync(userData, { recursive: true, force: true });
  }
});
