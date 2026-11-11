import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import react from '@vitejs/plugin-react';
import { createServer } from 'vite';

test('cold replay reaches real Stage through retries and skipped Chromium paint', { timeout: 45000 }, async () => {
  const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
  const fixture = join(root, 'test/fixtures');
  const temp = mkdtempSync(join(tmpdir(), 'lingxi-cold-restore-'));
  const vite = await createServer({ root: fixture, cacheDir: join(temp, 'vite'), configFile: false, logLevel: 'error',
    server: { host: '127.0.0.1', port: 0, hmr: false }, plugins: [react()],
    optimizeDeps: { entries: ['cold-restore-fixture.html'] },
    resolve: { alias: { '@renderer': join(root, 'src/renderer') }, dedupe: ['react', 'react-dom'] } });
  let child;
  try {
    await vite.listen();
    const address = vite.httpServer.address();
    child = spawn(process.execPath, [join(root, 'node_modules/electron/cli.js'), join(fixture, 'cold-restore-electron.mjs'),
      `http://127.0.0.1:${address.port}/cold-restore-fixture.html`], {
      cwd: root, env: { ...process.env, LINGXI_TEST_USER_DATA: join(temp, 'profile'), ELECTRON_ENABLE_LOGGING: '0' },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    let output = '', errors = '';
    child.stdout.on('data', chunk => { output += chunk; }); child.stderr.on('data', chunk => { errors += chunk; });
    const code = await new Promise((resolveExit, reject) => {
      const timer = setTimeout(() => { child.kill('SIGTERM'); reject(new Error(`cold restore fixture timed out\n${errors}`)); }, 35000);
      child.once('error', error => { clearTimeout(timer); reject(error); });
      child.once('exit', code => { clearTimeout(timer); resolveExit(code); });
    });
    assert.equal(code, 0, errors);
    const marker = output.split('\n').find(line => line.startsWith('COLD_RESTORE_RESULT:'));
    assert.ok(marker, `fixture must finish every scenario: ${output}`);
    const results = JSON.parse(marker.slice('COLD_RESTORE_RESULT:'.length));
    assert.deepEqual(results.map(row => row.scenario), ['normal', 'reject', 'stall', 'skipped-paint', 'visible-again']);
    for (const row of results.slice(0, 3)) {
      assert.equal(row.itemCount, 2); assert.equal(row.textContent, true);
      assert.equal(row.attempts, row.scenario === 'normal' ? 1 : 2);
      assert(row.samples.some(sample => sample.phase === 'renderer_base' && sample.messageCount === 2));
      assert(row.samples.some(sample => sample.phase === 'stage_commit' && sample.rowCount === 2));
    }
    console.log(`Cold restore evidence: ${JSON.stringify(results)}`);
  } finally {
    if (child && child.exitCode === null) child.kill('SIGTERM');
    await vite.close(); rmSync(temp, { recursive: true, force: true });
  }
});
