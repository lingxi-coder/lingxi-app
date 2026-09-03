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
const electronBinary = resolve(electronRoot, 'node_modules/electron/cli.js');
const electronDriver = join(fixtureRoot, 'settings-transaction-electron.mjs');

test('Settings can close during pending credential persistence and ignores the late result', async () => {
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
    child = spawn(process.execPath, [electronBinary, electronDriver, fixtureUrl], {
      cwd: electronRoot,
      env: { ...process.env, ELECTRON_ENABLE_LOGGING: '0', ELECTRON_IS_DEV: '0', LINGXI_TEST_USER_DATA: temporaryUserData, LINGXI_SETTINGS_SCENARIO: 'close' },
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
    assert.deepEqual(result.busyState, {
      closeDisabled: false,
      state: { persistencePending: true, restartCalls: 0, restartTargets: [], restartAttempts: [], restartErrors: 0, closeCalls: 0, activeSessionId: 'session-a', activeWork: false },
    });
    assert.deepEqual(result.afterClose, { persistencePending: true, restartCalls: 0, restartTargets: [], restartAttempts: [], restartErrors: 0, closeCalls: 1, activeSessionId: 'session-a', activeWork: false });
    assert.deepEqual(result.afterLateResult, { persistencePending: false, restartCalls: 0, restartTargets: [], restartAttempts: ['session-a'], restartErrors: 1, closeCalls: 1, activeSessionId: 'session-b', activeWork: true });
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

test('Settings recovery retry keeps the originally captured session id', async () => {
  const viteCacheDir = mkdtempSync(join(tmpdir(), 'lingxi-settings-recovery-vite-'));
  const vite = await createServer({
    root: fixtureRoot,
    cacheDir: viteCacheDir,
    configFile: false,
    logLevel: 'error',
    server: { host: '127.0.0.1', port: 0, strictPort: false, hmr: false },
    plugins: [react()],
    resolve: { alias: { '@renderer': resolve(electronRoot, 'src/renderer') } },
  });
  const temporaryUserData = mkdtempSync(join(tmpdir(), 'lingxi-settings-recovery-electron-'));
  const childOutput = [];
  const childError = [];
  let child;

  try {
    await vite.listen();
    const address = vite.httpServer?.address();
    assert.ok(address && typeof address === 'object' && address.port, 'Vite fixture server did not bind a port');
    const fixtureUrl = `http://127.0.0.1:${address.port}/settings-transaction-fixture.html`;
    child = spawn(process.execPath, [electronBinary, electronDriver, fixtureUrl], {
      cwd: electronRoot,
      env: { ...process.env, ELECTRON_ENABLE_LOGGING: '0', ELECTRON_IS_DEV: '0', LINGXI_TEST_USER_DATA: temporaryUserData, LINGXI_SETTINGS_SCENARIO: 'recovery' },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    child.stdout.on('data', (chunk) => childOutput.push(String(chunk)));
    child.stderr.on('data', (chunk) => childError.push(String(chunk)));

    const result = await new Promise((resolveResult, rejectResult) => {
      const timeout = setTimeout(() => {
        child.kill('SIGTERM');
        rejectResult(new Error(`Electron Settings recovery fixture timed out\n${childError.join('')}`));
      }, 20_000);
      child.once('error', (error) => { clearTimeout(timeout); rejectResult(error); });
      child.once('exit', (code, signal) => {
        clearTimeout(timeout);
        const output = childOutput.join('').trim().split('\n').at(-1);
        if (code !== 0 || !output) {
          rejectResult(new Error(`Electron Settings recovery fixture exited ${code ?? signal}\n${childError.join('')}`));
          return;
        }
        try { resolveResult(JSON.parse(output)); }
        catch (error) { rejectResult(new Error(`Invalid Settings recovery output: ${output}`, { cause: error })); }
      });
    });

    assert.deepEqual(result.afterFirstFailure, {
      persistencePending: false,
      restartCalls: 0,
      restartTargets: [],
      restartAttempts: ['session-a'],
      restartErrors: 1,
      closeCalls: 0,
      activeSessionId: 'session-b',
      activeWork: true,
    });
    assert.deepEqual(result.afterWrongSessionRetry, {
      persistencePending: false,
      restartCalls: 0,
      restartTargets: [],
      restartAttempts: ['session-a', 'session-a'],
      restartErrors: 2,
      closeCalls: 0,
      activeSessionId: 'session-b',
      activeWork: false,
    });
    assert.deepEqual(result.afterRecoveredRetry, {
      persistencePending: false,
      restartCalls: 1,
      restartTargets: ['session-a'],
      restartAttempts: ['session-a', 'session-a', 'session-a'],
      restartErrors: 2,
      closeCalls: 0,
      activeSessionId: 'session-a',
      activeWork: false,
    });
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
