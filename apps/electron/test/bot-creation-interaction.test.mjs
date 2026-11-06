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

test('Bot setup works through the whole window: describe, prepare chat, stream proposal, review, create and retry', async () => {
  const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
  const fixtureRoot = join(root, 'test/fixtures');
  const temporary = mkdtempSync(join(tmpdir(), 'lingxi-bot-creation-'));
  const vite = await createServer({ root: fixtureRoot, cacheDir: join(temporary, 'vite'), configFile: false, logLevel: 'error',
    server: { host: '127.0.0.1', port: 0, hmr: false }, plugins: [react()],
    optimizeDeps: { entries: ['bot-creation-fixture.html'] },
    resolve: { alias: { '@renderer': resolve(root, 'src/renderer') }, dedupe: ['react', 'react-dom'] } });
  let child;
  try {
    await vite.listen();
    child = spawn(electronBinary, [join(fixtureRoot, 'bot-creation-electron.mjs'), `http://127.0.0.1:${vite.httpServer.address().port}/bot-creation-fixture.html`], {
      cwd: root, env: { ...process.env, ELECTRON_ENABLE_LOGGING: '0', ELECTRON_IS_DEV: '0', LINGXI_TEST_USER_DATA: join(temporary, 'electron') }, stdio: ['ignore', 'pipe', 'pipe'],
    });
    let output = ''; let errors = '';
    child.stdout.on('data', chunk => { output += chunk; });
    child.stderr.on('data', chunk => { errors += chunk; });
    await new Promise((done, reject) => {
      const timeout = setTimeout(() => { child.kill('SIGTERM'); reject(new Error(`Bot setup timed out\n${errors}`)); }, 60_000);
      child.once('error', error => { clearTimeout(timeout); reject(error); });
      child.once('exit', code => { clearTimeout(timeout); code === 0 ? done() : reject(new Error(`Bot setup exited ${code}\n${errors}`)); });
    });
    assert.deepEqual(JSON.parse(output.trim().split('\n').at(-1)), { draftPreserved: true, chatPrepared: true, streamingPreview: true, reviewed: true, retried: true });
  } finally {
    if (child && child.exitCode === null) child.kill('SIGTERM');
    await vite.close();
    rmSync(temporary, { recursive: true, force: true });
  }
});
