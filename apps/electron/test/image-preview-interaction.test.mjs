import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import { build } from 'esbuild';
import react from '@vitejs/plugin-react';
import { createServer } from 'vite';

const electronRoot = resolve(fileURLToPath(new URL('..', import.meta.url)));

test('full-screen image preview fits, zooms, pans, downloads, and dismisses accessibly', async () => {
  const temporary = mkdtempSync(join(tmpdir(), 'lingxi-image-preview-'));
  const vite = await createServer({
    root: join(electronRoot, 'test/fixtures'), cacheDir: join(temporary, 'vite'), configFile: false, logLevel: 'error',
    server: { host: '127.0.0.1', port: 0, hmr: false }, plugins: [react()],
    resolve: { alias: { '@renderer': resolve(electronRoot, 'src/renderer') }, dedupe: ['react', 'react-dom'] },
  });
  let child;
  try {
    await build({ entryPoints: [join(electronRoot, 'src/main/imageSave.ts')], bundle: true, platform: 'node', format: 'esm', outfile: join(temporary, 'imageSave.mjs') });
    await vite.listen();
    const address = vite.httpServer.address();
    const output = [], errors = [];
    child = spawn(process.execPath, [join(electronRoot, 'node_modules/electron/cli.js'), join(electronRoot, 'test/fixtures/image-preview-electron.mjs'), `http://127.0.0.1:${address.port}/image-preview-fixture.html`], {
      cwd: electronRoot,
      env: { ...process.env, ELECTRON_ENABLE_LOGGING: '0', ELECTRON_IS_DEV: '0', LINGXI_TEST_USER_DATA: join(temporary, 'user'), LINGXI_IMAGE_SAVE_MODULE: join(temporary, 'imageSave.mjs'), LINGXI_IMAGE_DOWNLOAD: join(temporary, 'download.png') },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    child.stdout.on('data', (chunk) => output.push(String(chunk)));
    child.stderr.on('data', (chunk) => errors.push(String(chunk)));
    const result = await new Promise((resolveResult, reject) => {
      const timeout = setTimeout(() => { child.kill('SIGTERM'); reject(new Error(`Image preview timed out\n${errors.join('')}`)); }, 35_000);
      child.once('error', (error) => { clearTimeout(timeout); reject(error); });
      child.once('exit', (code) => {
        clearTimeout(timeout);
        if (code !== 0) return reject(new Error(`Image preview failed (${code})\n${errors.join('')}`));
        try { resolveResult(JSON.parse(output.join('').trim().split('\n').at(-1))); } catch (error) { reject(error); }
      });
    });
    assert.ok(result.desktop.fits && result.desktop.centered && result.desktop.controlsClear);
    assert.equal(result.desktop.focus, 'Close preview (Esc)');
    assert.equal(result.desktop.modal, true);
    assert.equal(result.zoomed.scale, '100%');
    assert.ok(result.panned, 'dragging must move the zoomed image');
    assert.ok(result.wheelZoomed, 'wheel input must zoom the image');
    assert.ok(result.buttonsZoomed && result.doubleClickZoomed);
    assert.ok(result.focusTrapped && result.focusRestored);
    assert.ok(result.narrow.fits && result.narrow.controlsClear);
    assert.ok(result.landscape.fits && result.landscape.centered);
    assert.ok(result.resized.fits && result.resized.centered);
    assert.ok(result.downloaded > 1000);
    assert.ok(result.cancelled && result.saveError && result.savedOriginal);
    assert.ok(result.backdropClosed && result.buttonClosed && result.errorClosed);
    assert.ok(result.reducedMotion && result.backgroundScrollRestored);
  } finally {
    if (child && child.exitCode === null) child.kill('SIGTERM');
    await vite.close();
    rmSync(temporary, { recursive: true, force: true });
  }
});
