import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import react from '@vitejs/plugin-react';
import { createServer } from 'vite';

test('modal account setup preserves multiple auth accounts, focus, progress and secret clearing', async () => {
  const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
  const temporary = mkdtempSync(join(tmpdir(), 'lingxi-account-design-'));
  const vite = await createServer({ root: join(root, 'test/fixtures'), cacheDir: join(temporary, 'vite'), configFile: false,
    logLevel: 'error', server: { host: '127.0.0.1', port: 0, hmr: false, fs: { allow: [root, resolve(root, '../../assets/brand/provider-icons')] } }, plugins: [react()],
    resolve: { alias: { '@renderer': join(root, 'src/renderer') }, dedupe: ['react', 'react-dom'] } });
  let child;
  try {
    await vite.listen();
    const url = `http://127.0.0.1:${vite.httpServer.address().port}/provider-accounts-design.html`;
    const electron = createRequire(import.meta.url)('electron');
    child = spawn(electron, [join(root, 'test/fixtures/provider-accounts-design-electron.mjs'), url],
      { cwd: root, env: { ...process.env, LINGXI_TEST_USER_DATA: join(temporary, 'electron') }, stdio: ['ignore', 'pipe', 'pipe'] });
    let output = '', errors = '';
    const result = await new Promise((resolveResult, reject) => {
      const timer = setTimeout(() => reject(new Error(`Account design fixture timed out: ${errors}`)), process.env.LINGXI_DESIGN_SCREENSHOTS ? 60_000 : 20_000);
      child.stdout.on('data', data => {
        output += data;
        if (!output.endsWith('\n')) return;
        try {
          const result = JSON.parse(output.trim().split('\n').at(-1));
          clearTimeout(timer);
          resolveResult(result);
        } catch { /* Ignore incomplete or unrelated output. */ }
      });
      child.stderr.on('data', data => { errors += data; });
      child.once('error', error => { clearTimeout(timer); reject(error); });
      child.once('close', code => { clearTimeout(timer); reject(new Error(`Account design fixture exited with ${code}: ${errors}`)); });
    });
    for (const key of ['initialSetup', 'multiAccount', 'directoryStatuses', 'modal', 'focus', 'oauthCancel', 'apiKeyClear', 'testingFeedback', 'reorder', 'removalConfirmation']) assert.equal(result[key], true, key);
  } finally {
    // The renderer's result is available before offscreen GPU teardown, which can stall.
    if (child && child.exitCode === null) child.kill('SIGKILL');
    await vite.close(); rmSync(temporary, { recursive: true, force: true });
  }
});
