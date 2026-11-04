import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import react from '@vitejs/plugin-react';
import { createServer } from 'vite';

test('inline account setup preserves multiple auth accounts, focus, progress and secret clearing', async () => {
  const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
  const temporary = mkdtempSync(join(tmpdir(), 'lingxi-account-design-'));
  const vite = await createServer({ root: join(root, 'test/fixtures'), cacheDir: join(temporary, 'vite'), configFile: false,
    logLevel: 'error', server: { host: '127.0.0.1', port: 0, hmr: false }, plugins: [react()],
    resolve: { alias: { '@renderer': join(root, 'src/renderer') }, dedupe: ['react', 'react-dom'] } });
  let child;
  try {
    await vite.listen();
    const url = `http://127.0.0.1:${vite.httpServer.address().port}/provider-accounts-design.html`;
    child = spawn(process.execPath, [join(root, 'node_modules/electron/cli.js'), join(root, 'test/fixtures/provider-accounts-design-electron.mjs'), url],
      { cwd: root, env: { ...process.env, LINGXI_TEST_USER_DATA: join(temporary, 'electron') }, stdio: ['ignore', 'pipe', 'pipe'] });
    let output = '', errors = '';
    child.stdout.on('data', data => { output += data; }); child.stderr.on('data', data => { errors += data; });
    await new Promise((resolveExit, reject) => {
      const timer = setTimeout(() => { child.kill('SIGTERM'); reject(new Error(`Account design fixture timed out: ${errors}`)); }, process.env.LINGXI_DESIGN_SCREENSHOTS ? 60_000 : 20_000);
      child.once('error', error => { clearTimeout(timer); reject(error); });
      child.once('exit', code => { clearTimeout(timer); code === 0 ? resolveExit() : reject(new Error(`Account design fixture failed: ${errors}`)); });
    });
    const result = JSON.parse(output.trim().split('\n').at(-1));
    for (const key of ['initialSetup', 'multiAccount', 'directoryStatuses', 'inline', 'focus', 'oauthCancel', 'apiKeyClear', 'testingFeedback', 'reorder', 'removalConfirmation']) assert.equal(result[key], true, key);
  } finally {
    if (child && child.exitCode === null) child.kill('SIGTERM');
    await vite.close(); rmSync(temporary, { recursive: true, force: true });
  }
});
