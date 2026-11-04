import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { join, resolve } from 'node:path';
import { test } from 'node:test';

import react from '@vitejs/plugin-react';
import { createServer } from 'vite';
import electronBinary from 'electron';

const electronRoot = resolve(fileURLToPath(new URL('..', import.meta.url)));
const fixtureRoot = join(electronRoot, 'test', 'fixtures');
const electronDriver = join(fixtureRoot, 'settings-transaction-electron.mjs');

test('credential modal protects pending writes and recovers from failure before closing Settings', async () => {
  const viteCacheDir = mkdtempSync(join(tmpdir(), 'lingxi-settings-transaction-vite-'));
  const vite = await createServer({
    root: fixtureRoot,
    cacheDir: viteCacheDir,
    configFile: false,
    logLevel: 'error',
    server: { host: '127.0.0.1', port: 0, strictPort: false, hmr: false },
    plugins: [react()],
    resolve: { alias: { '@renderer': resolve(electronRoot, 'src/renderer') } },
  });
  const temporaryUserData = mkdtempSync(join(tmpdir(), 'lingxi-settings-transaction-electron-'));
  const childOutput = [];
  const childError = [];
  let child;

  try {
    await vite.listen();
    const address = vite.httpServer?.address();
    assert.ok(address && typeof address === 'object' && address.port, 'Vite fixture server did not bind a port');
    const fixtureUrl = `http://127.0.0.1:${address.port}/settings-transaction-fixture.html`;
    child = spawn(electronBinary, [electronDriver, fixtureUrl], {
      cwd: electronRoot,
      env: { ...process.env, ELECTRON_ENABLE_LOGGING: '0', ELECTRON_IS_DEV: '0', LINGXI_TEST_USER_DATA: temporaryUserData },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    child.stdout.on('data', (chunk) => childOutput.push(String(chunk)));
    child.stderr.on('data', (chunk) => childError.push(String(chunk)));

    const result = await new Promise((resolveResult, rejectResult) => {
      const timeout = setTimeout(() => {
        child.kill('SIGTERM');
        rejectResult(new Error(`Electron Settings transaction fixture timed out\n${childError.join('')}`));
      }, 20_000);
      child.once('error', (error) => { clearTimeout(timeout); rejectResult(error); });
      child.once('exit', (code, signal) => {
        clearTimeout(timeout);
        const output = childOutput.join('').trim().split('\n').at(-1);
        if (code !== 0 || !output) {
          rejectResult(new Error(`Electron Settings transaction fixture exited ${code ?? signal}\n${childError.join('')}`));
          return;
        }
        try { resolveResult(JSON.parse(output)); }
        catch (error) { rejectResult(new Error(`Invalid Settings transaction output: ${output}`, { cause: error })); }
      });
    });

    assert.equal(result.connectionTestErrorBeforeDelete, true);
    assert.equal(result.connectionTestErrorAfterDelete, false);
    const idle = { persistencePending: false, removalPending: false, restartCalls: 0, closeCalls: 0, activeSessionId: 'session-a', activeWork: false, persistedCredentials: [] };
    assert.deepEqual(result.pendingRemoval, { modalOpen: true, accountCount: 1, state: { ...idle, removalPending: true } });
    assert.deepEqual(result.busyState, {
      modalOpen: true,
      cancelDisabled: true,
      settingsOpen: true,
      state: { ...idle, persistencePending: true },
    });
    assert.deepEqual(result.afterFailure, { accountCount: 0, state: idle });
    assert.deepEqual(result.afterIdleEscape, idle, 'Escape closes only the idle account dialog');
    const saved = [{ providerId: 'anthropic', credential: 'sk-successful-save' }];
    assert.deepEqual(result.afterSuccess, {
      accountCount: 1,
      successVisible: true,
      state: { ...idle, persistedCredentials: saved },
    });
    assert.deepEqual(result.afterClose, { ...idle, closeCalls: 1, persistedCredentials: saved });
    assert.deepEqual(result.afterSessionSwitch, { ...idle, closeCalls: 1, activeSessionId: 'session-b', activeWork: true, persistedCredentials: saved });
  } finally {
    if (child && child.exitCode === null) {
      child.kill('SIGTERM');
      await new Promise((resolveExit) => {
        const timeout = setTimeout(resolveExit, 2000);
        child.once('exit', () => { clearTimeout(timeout); resolveExit(); });
      });
    }
    await vite.close();
    rmSync(viteCacheDir, { recursive: true, force: true });
    rmSync(temporaryUserData, { recursive: true, force: true });
  }
});
