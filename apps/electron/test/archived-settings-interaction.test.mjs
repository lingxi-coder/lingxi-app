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
const electronDriver = join(fixtureRoot, 'archived-settings-electron.mjs');

test('archived settings lists all projects, searches, and restores with failure recovery', async () => {
  const viteCacheDir = mkdtempSync(join(tmpdir(), 'lingxi-archived-settings-vite-'));
  const temporaryUserData = mkdtempSync(join(tmpdir(), 'lingxi-archived-settings-electron-'));
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
    const fixtureUrl = `http://127.0.0.1:${address.port}/archived-settings-fixture.html`;
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
        rejectResult(new Error(`Electron archived settings fixture timed out\n${errors.join('')}`));
      }, 20_000);
      child.once('error', (error) => { clearTimeout(timeout); rejectResult(error); });
      child.once('exit', (code, signal) => {
        clearTimeout(timeout);
        const line = output.join('').trim().split('\n').at(-1);
        if (code !== 0 || !line) {
          rejectResult(new Error(`Electron archived settings fixture exited ${code ?? signal}\n${errors.join('')}`));
          return;
        }
        try { resolveResult(JSON.parse(line)); }
        catch (error) { rejectResult(new Error(`Invalid Electron archived settings fixture output: ${line}`, { cause: error })); }
      });
    });

    assert.equal(result.count, 27);
    assert.equal(result.navPresent, true);
    assert.deepEqual(result.projects, ['/fixture/project-alpha', '/fixture/project-beta']);
    assert.equal(result.projectSearchCount, 13);
    assert.match(result.emptySearch, /没有匹配/);
    assert.equal(result.failed.open, true);
    assert.equal(result.failed.closes, 0);
    assert.equal(result.failed.retryEnabled, true);
    assert.match(result.failed.error, /Fixture restore failed/);
    assert.deepEqual(result.failed.calls, [['/fixture/project-beta', 'archive-23']]);
    assert.equal(result.succeeded.open, false);
    assert.equal(result.succeeded.closes, 1);
    assert.deepEqual(result.succeeded.calls, [['/fixture/project-beta', 'archive-23'], ['/fixture/project-beta', 'archive-23']]);
  } finally {
    if (child && child.exitCode === null) child.kill('SIGTERM');
    await vite.close();
    rmSync(viteCacheDir, { recursive: true, force: true });
    rmSync(temporaryUserData, { recursive: true, force: true });
  }
});
