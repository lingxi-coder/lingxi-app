import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import react from '@vitejs/plugin-react';
import { createServer } from 'vite';

const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
test('expanded activities and tool groups window leaves in the real transcript viewport', async t => {
  const cacheDir = mkdtempSync(join(tmpdir(), 'lingxi-transcript-leaf-vite-'));
  const userData = mkdtempSync(join(tmpdir(), 'lingxi-transcript-leaf-electron-'));
  const server = await createServer({ root: join(root, 'test/fixtures'), cacheDir, configFile: false, logLevel: 'error', plugins: [react()],
    resolve: { alias: { '@renderer': join(root, 'src/renderer') }, dedupe: ['react', 'react-dom'] }, server: { host: '127.0.0.1', port: 0, hmr: false } });
  let child;
  try {
    await server.listen();
    const address = server.httpServer.address();
    const result = await new Promise((accept, reject) => {
      let output = '', errors = '';
      child = spawn(process.execPath, [join(root, 'node_modules/electron/cli.js'), join(root, 'test/fixtures/transcript-leaf-window-electron.mjs'), `http://127.0.0.1:${address.port}/transcript-leaf-window-fixture.html`], {
        cwd: root, env: { ...process.env, LINGXI_TEST_USER_DATA: userData, ELECTRON_ENABLE_LOGGING: '0', ELECTRON_IS_DEV: '0' }, stdio: ['ignore', 'pipe', 'pipe'],
      });
      const timeout = setTimeout(() => { child.kill('SIGTERM'); reject(new Error(`Transcript leaf fixture timeout\n${errors}`)); }, 60_000);
      child.stdout.on('data', chunk => { output += chunk; }); child.stderr.on('data', chunk => { errors += chunk; });
      child.once('error', error => { clearTimeout(timeout); reject(error); });
      child.once('exit', code => {
        clearTimeout(timeout);
        if (code !== 0) { reject(new Error(`Transcript leaf fixture exited ${code}\n${errors}\n${output}`)); return; }
        try { accept(JSON.parse(output.trim().split('\n').at(-1))); } catch (error) { reject(error); }
      });
    });
    assert.ok(result.maxMounted < 100); assert.ok(result.maxSites < 200); assert.ok(result.prependDrift <= 2); assert.ok(result.tailGap <= 1);
    t.diagnostic(JSON.stringify(result));
  } finally { child?.kill('SIGTERM'); await server.close(); rmSync(cacheDir, { recursive: true, force: true }); rmSync(userData, { recursive: true, force: true }); }
});
