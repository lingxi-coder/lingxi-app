import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import react from '@vitejs/plugin-react';
import { build } from 'vite';

test('Electron skips oversized Mod requests, releases the old draw and recovers with intact native content', { timeout: 30_000 }, async () => {
  const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
  const fixtures = join(root, 'test/fixtures');
  const temporary = mkdtempSync(join(tmpdir(), 'lingxi-mod-bounds-'));
  let child;
  try {
    await build({
      root: fixtures, base: './', cacheDir: join(temporary, 'cache'), configFile: false,
      logLevel: 'error', plugins: [react()],
      resolve: { alias: { '@renderer': join(root, 'src/renderer') }, dedupe: ['react', 'react-dom'] },
      build: { outDir: join(temporary, 'build'), rollupOptions: { input: join(fixtures, 'mod-ui-bounds-fixture.html') } },
    });
    const output = [], errors = [];
    child = spawn(process.execPath, [join(root, 'node_modules/electron/cli.js'), join(fixtures, 'mod-ui-bounds-electron.mjs'), pathToFileURL(join(temporary, 'build/mod-ui-bounds-fixture.html')).href], {
      cwd: root, env: { ...process.env, LINGXI_TEST_USER_DATA: join(temporary, 'user-data') },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    child.stdout.on('data', chunk => output.push(String(chunk)));
    child.stderr.on('data', chunk => errors.push(String(chunk)));
    await new Promise((resolveExit, reject) => {
      const timer = setTimeout(() => { child.kill('SIGTERM'); reject(new Error(`Electron bounds fixture timed out: ${errors.join('')}`)); }, 20_000);
      child.once('error', error => { clearTimeout(timer); reject(error); });
      child.once('exit', code => {
        clearTimeout(timer);
        if (code !== 0) reject(new Error(`Electron bounds fixture failed (${code}): ${errors.join('')}`));
        else resolveExit();
      });
    });
    const result = JSON.parse(output.join('').trim().split('\n').at(-1));
    assert.equal(result.passed, true);
    assert.equal(result.fullNativeOutputLength, 100001);
    assert.equal(result.noOversizedOrRetryIPC, true);
    assert.equal(result.recoveredWithoutTruncation, true);
    assert.equal(result.weatherUnaffected, true);
  } finally {
    if (child && child.exitCode === null) child.kill('SIGTERM');
    rmSync(temporary, { recursive: true, force: true });
  }
});
