import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import react from '@vitejs/plugin-react';
import { createServer } from 'vite';

test('real Electron @ menu shares slash geometry and preserves complete reference interactions', { timeout: 40_000 }, async () => {
  const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
  const fixture = join(root, 'test/fixtures');
  const userData = mkdtempSync(join(tmpdir(), 'lingxi-mention-menu-'));
  const server = await createServer({ root: fixture, configFile: false, logLevel: 'error', plugins: [react()], resolve: { alias: { '@renderer': join(root, 'src/renderer') } }, server: { host: '127.0.0.1', port: 0, hmr: false } });
  let child;
  try {
    await server.listen();
    const address = server.httpServer.address();
    assert(address && typeof address !== 'string');
    child = spawn(process.execPath, [join(root, 'node_modules/electron/cli.js'), join(fixture, 'mention-menu-electron.mjs'), `http://127.0.0.1:${address.port}/composer-draft-fixture.html?mentions`], {
      cwd: root, env: { ...process.env, LINGXI_TEST_USER_DATA: userData }, stdio: ['ignore', 'pipe', 'pipe'],
    });
    let output = '';
    let errors = '';
    child.stdout.on('data', (chunk) => { output += chunk; });
    child.stderr.on('data', (chunk) => { errors += chunk; });
    await new Promise((resolveResult, reject) => {
      const timeout = setTimeout(() => { child.kill('SIGTERM'); reject(new Error(`Mention UI test timed out\n${output}\n${errors}`)); }, 30_000);
      child.once('error', (cause) => { clearTimeout(timeout); reject(cause); });
      child.once('exit', (code) => { clearTimeout(timeout); code === 0 ? resolveResult() : reject(new Error(`Mention UI test failed (${code})\n${output}\n${errors}`)); });
    });
    assert.match(output, /mention interactions passed/);
  } finally {
    if (child && child.exitCode === null) { child.kill('SIGTERM'); await new Promise((done) => child.once('exit', done)); }
    await server.close();
    rmSync(userData, { recursive: true, force: true });
  }
});
