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
const electronDriver = join(fixtureRoot, 'command-result-electron.mjs');

test('command result dialog preserves full usage details, fits narrow screens, and restores focus', async () => {
  const viteCacheDir = mkdtempSync(join(tmpdir(), 'lingxi-command-result-vite-'));
  const temporaryUserData = mkdtempSync(join(tmpdir(), 'lingxi-command-result-electron-'));
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
    const fixtureUrl = `http://127.0.0.1:${address.port}/command-result-fixture.html`;
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
        rejectResult(new Error(`Electron compaction fixture timed out\n${errors.join('')}`));
      }, 20_000);
      child.once('error', (error) => { clearTimeout(timeout); rejectResult(error); });
      child.once('exit', (code, signal) => {
        clearTimeout(timeout);
        const line = output.join('').trim().split('\n').at(-1);
        if (code !== 0 || !line) {
          rejectResult(new Error(`Electron compaction fixture exited ${code ?? signal}\n${errors.join('')}`));
          return;
        }
        try { resolveResult(JSON.parse(line)); }
        catch (error) { rejectResult(new Error(`Invalid Electron compaction fixture output: ${line}`, { cause: error })); }
      });
    });

    for (const viewport of [result.desktop, result.narrow]) {
      assert.equal(viewport.open, true);
      assert.equal(viewport.overflow, false);
      assert.equal(viewport.truncated, false);
      assert.equal(viewport.focus, 'Close command result');
      assert.match(viewport.text, /947 lines added/);
      assert.match(viewport.text, /284 lines removed/);
      assert.match(viewport.text, /2.7m cache read/);
      assert.match(viewport.text, /A future metric without a colon/);
      assert.match(viewport.text, /very-long-provider-model-name-over-32-characters/);
    }
    assert.deepEqual(result.dismissed, { closed: true, focus: 'open' });
    assert.equal(result.buttonClosed, true);
  } finally {
    if (child && child.exitCode === null) child.kill('SIGTERM');
    await vite.close();
    rmSync(viteCacheDir, { recursive: true, force: true });
    rmSync(temporaryUserData, { recursive: true, force: true });
  }
});
