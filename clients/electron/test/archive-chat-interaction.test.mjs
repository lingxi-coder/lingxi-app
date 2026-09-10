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
const electronDriver = join(fixtureRoot, 'archive-chat-electron.mjs');

test('archive chat dialog guards pending work, confirms once, and restores keyboard focus', async () => {
  const viteCacheDir = mkdtempSync(join(tmpdir(), 'lingxi-archive-chat-vite-'));
  const temporaryUserData = mkdtempSync(join(tmpdir(), 'lingxi-archive-chat-electron-'));
  const vite = await createServer({
    root: fixtureRoot,
    cacheDir: viteCacheDir,
    configFile: false,
    logLevel: 'error',
    server: { host: '127.0.0.1', port: 0, strictPort: false, hmr: false },
    plugins: [react()],
    resolve: {
      alias: { '@renderer': resolve(electronRoot, 'src/renderer') },
      dedupe: ['react', 'react-dom'],
    },
  });
  let child;

  try {
    await vite.listen();
    const address = vite.httpServer?.address();
    assert.ok(address && typeof address === 'object' && address.port);
    const fixtureUrl = `http://127.0.0.1:${address.port}/archive-chat-fixture.html`;
    const output = [];
    const errors = [];
    child = spawn(process.execPath, [electronBinary, electronDriver, fixtureUrl], {
      cwd: electronRoot,
      env: {
        ...process.env,
        ELECTRON_ENABLE_LOGGING: '0',
        ELECTRON_IS_DEV: '0',
        LINGXI_TEST_USER_DATA: temporaryUserData,
      },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    child.stdout.on('data', (chunk) => output.push(String(chunk)));
    child.stderr.on('data', (chunk) => errors.push(String(chunk)));

    const result = await new Promise((resolveResult, rejectResult) => {
      const timeout = setTimeout(() => {
        child.kill('SIGTERM');
        rejectResult(new Error(`Electron archive chat fixture timed out\n${errors.join('')}`));
      }, 20_000);
      child.once('error', (error) => { clearTimeout(timeout); rejectResult(error); });
      child.once('exit', (code, signal) => {
        clearTimeout(timeout);
        const line = output.join('').trim().split('\n').at(-1);
        if (code !== 0 || !line) {
          rejectResult(new Error(`Electron archive chat fixture exited ${code ?? signal}\n${errors.join('')}`));
          return;
        }
        try { resolveResult(JSON.parse(line)); }
        catch (error) { rejectResult(new Error(`Invalid Electron archive chat fixture output: ${line}`, { cause: error })); }
      });
    });

    assert.match(result.content, /Weekly project summary/);
    assert.match(result.content, /Archive and remove/);
    assert.match(result.content, /chat history will be kept/);
    assert.equal(result.loadingDisabled, true);
    assert.equal(result.afterCancel.confirms, 0);
    assert.equal(result.afterCancel.closes, 1);
    assert.equal(result.busyFocusContained, true, 'busy dialog must retain focus when its controls are disabled');
    assert.equal(result.busyTabPrevented, true);
    assert.equal(result.busyAllDisabled, true);
    assert.equal(result.busyRemainsOpen, true);
    assert.equal(result.forwardTrap, true);
    assert.equal(result.backwardTrap, true);
    assert.equal(result.escapeRestored, true);
    assert.equal(result.afterConfirm.confirms, 1);
    assert.equal(result.afterConfirm.open, false);
    assert.equal(result.errorDisabled, true);
  } finally {
    if (child && child.exitCode === null) child.kill('SIGTERM');
    await vite.close();
    rmSync(viteCacheDir, { recursive: true, force: true });
    rmSync(temporaryUserData, { recursive: true, force: true });
  }
});
