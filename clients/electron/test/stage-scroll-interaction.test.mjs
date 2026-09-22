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
const electronDriver = join(fixtureRoot, 'stage-scroll-electron.mjs');

test('Stage keeps the actual bottom stable during streaming and respects manual scrolling', async () => {
  const viteCacheDir = mkdtempSync(join(tmpdir(), 'lingxi-stage-scroll-vite-'));
  const temporaryUserData = mkdtempSync(join(tmpdir(), 'lingxi-stage-scroll-electron-'));
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
    const fixtureUrl = `http://127.0.0.1:${address.port}/stage-scroll-fixture.html`;
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
        rejectResult(new Error(`Electron Stage scroll fixture timed out\n${errors.join('')}`));
      }, 20_000);
      child.once('error', (error) => { clearTimeout(timeout); rejectResult(error); });
      child.once('exit', (code, signal) => {
        clearTimeout(timeout);
        const line = output.join('').trim().split('\n').at(-1);
        if (code !== 0 || !line) {
          rejectResult(new Error(`Electron Stage scroll fixture exited ${code ?? signal}\n${errors.join('')}`));
          return;
        }
        try { resolveResult(JSON.parse(line)); }
        catch (error) { rejectResult(new Error(`Invalid Electron Stage scroll fixture output: ${line}`, { cause: error })); }
      });
    });

    assert.ok(result.initialGap <= 1, `initial bottom gap: ${result.initialGap}`);
    assert.equal(result.ancestorScroll, 0, 'following the tail must not scroll an ancestor');
    assert.ok(Math.abs(result.dragAfter - result.awayBefore) <= 1, 'updates during a pointer drag must not snap');
    assert.ok(Math.abs(result.awayAfter - result.awayBefore) <= 1, 'updates must preserve manual position 40px above the bottom');
    assert.ok(result.immediateStreamingGaps.every((gap) => gap <= 1), `pre-paint streaming bottom gaps: ${result.immediateStreamingGaps}`);
    assert.ok(result.streamingGaps.every((gap) => gap <= 1), `streaming bottom gaps: ${result.streamingGaps}`);
    assert.ok(result.resizeGap <= 1, `async child resize bottom gap: ${result.resizeGap}`);
    assert.ok(result.shrinkGap <= 1, `async child shrink bottom gap: ${result.shrinkGap}`);
    assert.equal(result.finalAncestorScroll, 0);
    for (const layout of result.thinkingLayouts) {
      assert.deepEqual(layout, result.thinkingLayouts[0], 'thinking visibility must preserve tail height and scroll position');
    }

    for (const offset of [0, 4]) {
      const baseline = result.deliveryLayouts[offset];
      for (const layout of result.deliveryLayouts.slice(offset, offset + 4)) assert.deepEqual(layout, baseline, 'delivery changes must not resize or move transcript messages');
    }
  } finally {
    if (child && child.exitCode === null) child.kill('SIGTERM');
    await vite.close();
    rmSync(viteCacheDir, { recursive: true, force: true });
    rmSync(temporaryUserData, { recursive: true, force: true });
  }
});
