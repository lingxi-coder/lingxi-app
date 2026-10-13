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
const electronDriver = join(fixtureRoot, 'topbar-summary-electron.mjs');

for (const controlsOnly of [false, true]) {
  test(controlsOnly ? 'real Electron inspector header controls respond to mouse input' : 'real Electron workspace keeps summary pinned with independent details and restored context access', async () => {
    const viteCacheDir = mkdtempSync(join(tmpdir(), 'lingxi-topbar-summary-vite-'));
    const temporaryUserData = mkdtempSync(join(tmpdir(), 'lingxi-topbar-summary-electron-'));
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
      const fixtureUrl = `http://127.0.0.1:${address.port}/topbar-summary-fixture.html`;
      const output = [];
      const errors = [];
      child = spawn(process.execPath, [electronBinary, electronDriver, fixtureUrl, ...(controlsOnly ? ['--inspector-controls-only'] : [])], {
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
          rejectResult(new Error(`Electron topbar fixture timed out\n${errors.join('')}`));
        }, 35_000);
        child.once('error', (error) => { clearTimeout(timeout); rejectResult(error); });
        child.once('exit', (code, signal) => {
          clearTimeout(timeout);
          const line = output.join('').trim().split('\n').at(-1);
          if (code !== 0 || !line) {
            rejectResult(new Error(`Electron topbar fixture exited ${code ?? signal}\n${errors.join('')}`));
            return;
          }
          try { resolveResult(JSON.parse(line)); }
          catch (error) { rejectResult(new Error(`Invalid Electron topbar fixture output: ${line}`, { cause: error })); }
        });
      });

      if (controlsOnly) {
        assert.deepEqual(result.controls, ['summary', 'expand', 'restore', 'hide']);
        return;
      }
      assert.equal(result.checks.length, 9);
      assert.deepEqual(result.overview.sections, ['Context', 'Subagents', 'Resources', 'Plan']);
      assert.equal(result.overview.width, 300);
      assert.deepEqual(result.heights, { chat: 44, detail: 44, panel: 390 });

    } finally {
      if (child && child.exitCode === null) child.kill('SIGTERM');
      await vite.close();
      rmSync(viteCacheDir, { recursive: true, force: true });
      rmSync(temporaryUserData, { recursive: true, force: true });
    }
  });
}
