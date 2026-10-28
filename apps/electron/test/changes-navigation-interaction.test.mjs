import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import react from '@vitejs/plugin-react';
import electronBinary from 'electron';
import { createServer } from 'vite';

test('changes navigation works without a session and preserves chat state', async () => {
  const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
  const fixtureRoot = join(root, 'test/fixtures');
  const cacheDir = mkdtempSync(join(tmpdir(), 'lingxi-changes-vite-'));
  const userData = mkdtempSync(join(tmpdir(), 'lingxi-changes-electron-'));
  const vite = await createServer({ root: fixtureRoot, cacheDir, configFile: false, logLevel: 'error',
    server: { host: '127.0.0.1', port: 0, hmr: false }, plugins: [react()],
    optimizeDeps: { entries: ['changes-navigation-fixture.html'] },
    resolve: { alias: { '@renderer': resolve(root, 'src/renderer') }, dedupe: ['react', 'react-dom'] } });
  let child;
  try {
    await vite.listen();
    const address = vite.httpServer.address();
    child = spawn(electronBinary, [join(fixtureRoot, 'changes-navigation-electron.mjs'), `http://127.0.0.1:${address.port}/changes-navigation-fixture.html`], {
      cwd: root, env: { ...process.env, ELECTRON_ENABLE_LOGGING: '0', ELECTRON_IS_DEV: '0', LINGXI_TEST_USER_DATA: userData }, stdio: ['ignore', 'pipe', 'pipe'],
    });
    let output = ''; let errors = '';
    child.stdout.on('data', chunk => { output += chunk; });
    child.stderr.on('data', chunk => { errors += chunk; });
    await new Promise((resolveResult, reject) => {
      const timeout = setTimeout(() => { child.kill('SIGTERM'); reject(new Error(`Changes navigation timed out\n${errors}`)); }, 35_000);
      child.once('error', error => { clearTimeout(timeout); reject(error); });
      child.once('exit', code => { clearTimeout(timeout); code === 0 ? resolveResult() : reject(new Error(`Changes navigation exited ${code}\n${errors}`)); });
    });
    const result = JSON.parse(output.trim().split('\n').at(-1));
    for (const name of ['sideChatDoesNotHideReview', 'chatStatePreserved', 'reviewWithoutSession', 'liveCountAndCleanState', 'noProjectFeedback', 'nonRepositoryFeedback']) assert.equal(result[name], true, name);
  } finally {
    if (child && child.exitCode === null) child.kill('SIGTERM');
    await vite.close();
    rmSync(cacheDir, { recursive: true, force: true });
    rmSync(userData, { recursive: true, force: true });
  }
});
