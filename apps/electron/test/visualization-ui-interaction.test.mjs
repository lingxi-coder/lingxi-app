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
const fixtureRoot = join(electronRoot, 'test', 'fixtures');

test('visualization panels preserve a real guest through expansion, keyboard dismissal and theme changes', async () => {
  const temporary = mkdtempSync(join(tmpdir(), 'lingxi-visualization-ui-'));
  const guestPreload = join(temporary, 'visualization-preload.cjs');
  await build({ entryPoints: [join(electronRoot, 'src/preload/visualization.ts')], outfile: guestPreload, bundle: true, platform: 'node', format: 'cjs', external: ['electron'] });
  const vite = await createServer({
    root: fixtureRoot, cacheDir: join(temporary, 'vite'), configFile: false, logLevel: 'error',
    server: { host: '127.0.0.1', port: 0, strictPort: false, hmr: false },
    plugins: [react()],
    resolve: { alias: { '@renderer': resolve(electronRoot, 'src/renderer') }, dedupe: ['react', 'react-dom'] },
  });
  let child;
  try {
    await vite.listen();
    const address = vite.httpServer?.address();
    assert.ok(address && typeof address === 'object' && address.port);
    const fixtureUrl = `http://127.0.0.1:${address.port}/visualization-ui-fixture.html`;
    const output = [];
    const errors = [];
    child = spawn(process.execPath, [join(electronRoot, 'node_modules/electron/cli.js'), join(fixtureRoot, 'visualization-ui-electron.mjs'), fixtureUrl], {
      cwd: electronRoot,
      env: { ...process.env, ELECTRON_ENABLE_LOGGING: '0', ELECTRON_IS_DEV: '0', LINGXI_TEST_USER_DATA: join(temporary, 'profile'), LINGXI_VISUALIZATION_GUEST_PRELOAD: guestPreload },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    child.stdout.on('data', chunk => output.push(String(chunk)));
    child.stderr.on('data', chunk => errors.push(String(chunk)));
    const result = await new Promise((resolveResult, rejectResult) => {
      const timeout = setTimeout(() => {
        child.kill('SIGTERM');
        rejectResult(new Error(`Electron visualization fixture timed out\n${errors.join('')}`));
      }, 45_000);
      child.once('error', error => { clearTimeout(timeout); rejectResult(error); });
      child.once('exit', (code, signal) => {
        clearTimeout(timeout);
        const line = output.join('').trim().split('\n').at(-1);
        if (code !== 0 || !line) {
          rejectResult(new Error(`Electron visualization fixture exited ${code ?? signal}\n${errors.join('')}`));
          return;
        }
        try { resolveResult(JSON.parse(line)); }
        catch (error) { rejectResult(new Error(`Invalid fixture output: ${line}`, { cause: error })); }
      });
    });
    assert.equal(result.passed, true);
    assert.equal(result.preservedGuest, true);
  } finally {
    if (child && child.exitCode === null) child.kill('SIGTERM');
    await vite.close();
    rmSync(temporary, { recursive: true, force: true });
  }
});
