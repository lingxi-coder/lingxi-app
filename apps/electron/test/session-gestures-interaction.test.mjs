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

test('session hover, focus, click, hold, and drag coexist in Electron', async () => {
  const viteCacheDir = mkdtempSync(join(tmpdir(), 'lingxi-session-gestures-vite-'));
  const temporaryUserData = mkdtempSync(join(tmpdir(), 'lingxi-session-gestures-electron-'));
  const vite = await createServer({
    root: fixtureRoot, cacheDir: viteCacheDir, configFile: false, logLevel: 'error',
    server: { host: '127.0.0.1', port: 0, strictPort: false, hmr: false },
    plugins: [react()],
    resolve: { alias: { '@renderer': resolve(electronRoot, 'src/renderer') }, dedupe: ['react', 'react-dom'] },
  });
  let child;
  try {
    await vite.listen();
    const address = vite.httpServer?.address();
    assert.ok(address && typeof address === 'object' && address.port);
    const output = [], errors = [];
    child = spawn(process.execPath, [resolve(electronRoot, 'node_modules/electron/cli.js'), join(fixtureRoot, 'session-gestures-electron.mjs'), `http://127.0.0.1:${address.port}/session-gestures-fixture.html`], {
      cwd: electronRoot,
      env: { ...process.env, ELECTRON_ENABLE_LOGGING: '0', ELECTRON_IS_DEV: '0', LINGXI_TEST_USER_DATA: temporaryUserData },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    child.stdout.on('data', chunk => output.push(String(chunk)));
    child.stderr.on('data', chunk => errors.push(String(chunk)));
    const result = await new Promise((resolveResult, rejectResult) => {
      const timeout = setTimeout(() => { child.kill('SIGTERM'); rejectResult(new Error(`Session gesture fixture timed out\n${errors.join('')}`)); }, 25_000);
      child.once('error', error => { clearTimeout(timeout); rejectResult(error); });
      child.once('exit', (code, signal) => {
        clearTimeout(timeout);
        const line = output.join('').trim().split('\n').at(-1);
        if (code !== 0 || !line) { rejectResult(new Error(`Session gesture fixture exited ${code ?? signal}\n${errors.join('')}`)); return; }
        try { resolveResult(JSON.parse(line)); }
        catch (error) { rejectResult(new Error(`Invalid fixture output: ${line}`, { cause: error })); }
      });
    });
    assert.equal(result.passed, true);
  } finally {
    if (child && child.exitCode === null) child.kill('SIGTERM');
    await vite.close();
    rmSync(viteCacheDir, { recursive: true, force: true });
    rmSync(temporaryUserData, { recursive: true, force: true });
  }
});
