import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { join, resolve } from 'node:path';
import { test } from 'node:test';

import react from '@vitejs/plugin-react';
import { createServer } from 'vite';

const electronRoot = resolve(fileURLToPath(new URL('..', import.meta.url)));
const fixtureRoot = join(electronRoot, 'test', 'fixtures');
const electronBinary = resolve(electronRoot, 'node_modules/electron/cli.js');
const electronDriver = join(fixtureRoot, 'compact-progress-electron.mjs');

test('real Electron compaction follows engine phases, preserves elapsed time, and settles one row', async () => {
  const viteCacheDir = mkdtempSync(join(tmpdir(), 'lingxi-compact-progress-vite-'));
  const temporaryUserData = mkdtempSync(join(tmpdir(), 'lingxi-compact-progress-electron-'));
  const vite = await createServer({
    root: fixtureRoot,
    cacheDir: viteCacheDir,
    configFile: false,
    logLevel: 'error',
    server: { host: '127.0.0.1', port: 0, strictPort: false, hmr: false },
    plugins: [react()],
    resolve: {
      alias: { '@renderer': resolve(electronRoot, 'src/renderer') },
      dedupe: ['react', 'react-dom'],
    },
  });
  let child;

  try {
    await vite.listen();
    const address = vite.httpServer?.address();
    assert.ok(address && typeof address === 'object' && address.port);
    const fixtureUrl = `http://127.0.0.1:${address.port}/compact-progress-fixture.html`;
    const output = [];
    const errors = [];
    child = spawn(process.execPath, [electronBinary, electronDriver, fixtureUrl], {
      cwd: electronRoot,
      env: {
        ...process.env,
        ELECTRON_ENABLE_LOGGING: '0',
        ELECTRON_IS_DEV: '0',
        LINGXI_TEST_USER_DATA: temporaryUserData,
      },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    child.stdout.on('data', (chunk) => output.push(String(chunk)));
    child.stderr.on('data', (chunk) => errors.push(String(chunk)));

    const result = await new Promise((resolveResult, rejectResult) => {
      const timeout = setTimeout(() => {
        child.kill('SIGTERM');
        rejectResult(new Error(`Electron compaction fixture timed out\n${errors.join('')}`));
      }, 20_000);
      child.once('error', (error) => { clearTimeout(timeout); rejectResult(error); });
      child.once('exit', (code, signal) => {
        clearTimeout(timeout);
        const line = output.join('').trim().split('\n').at(-1);
        if (code !== 0 || !line) {
          rejectResult(new Error(`Electron compaction fixture exited ${code ?? signal}\n${errors.join('')}`));
          return;
        }
        try { resolveResult(JSON.parse(line)); }
        catch (error) { rejectResult(new Error(`Invalid Electron compaction fixture output: ${line}`, { cause: error })); }
      });
    });

    const phaseTitles = { preparing: 'Preparing compaction', summarizing: 'Summarizing conversation', restoring: 'Restoring context' };
    for (const phase of ['preparing', 'summarizing', 'restoring']) {
      const progress = result[phase];
      assert.equal(progress.rowCount, 1, `${phase} must reuse one progress row`);
      assert.equal(progress.progressCount, 1);
      assert.equal(progress.valueNow, ({ preparing: '0', summarizing: '10', restoring: '85' })[phase]);
      assert.equal(progress.title, phaseTitles[phase]);
      assert.equal(progress.id, result.preparing.id);
      assert.equal(progress.item.status, 'running');
    }
    assert.match(result.aged.elapsed, /^9[0-9] seconds elapsed$/);
    assert.match(result.remounted.elapsed, /^9[0-9] seconds elapsed$/);
    assert.equal(result.aged.valueNow, '99');
    assert.equal(result.remounted.valueNow, '99');
    assert.equal(result.aged.item.phaseStartedAt, result.remounted.item.phaseStartedAt);
    assert.equal(result.complete.progressCount, 0);
    assert.equal(result.complete.title, 'Compaction finished');
    assert.match(result.complete.text, /100%/);
    assert.equal(result.complete.rowCount, 1);
    assert.equal(result.complete.id, result.preparing.id);
    assert.doesNotMatch(result.complete.text, /0 → 0|0 B saved/);
    assert.equal(result.summary.rowCount, 1);
    assert.equal(result.summary.title, 'Context compacted');
    assert.equal(result.summary.id, result.preparing.id);
    assert.match(result.summary.text, /42 → 7 messages/);
    assert.match(result.summary.text, /38 KB saved/);
    assert.equal(result.summary.summaries.length, 1);
    assert.equal(result.summary.summaries[0].content, 'Preserved the actual conversation context.');
    for (const status of ['cancelled', 'error', 'skipped']) {
      assert.equal(result[status].rowCount, 1);
      assert.equal(result[status].progressCount, 0);
      assert.equal(result[status].id, result[status].startedId);
      assert.equal(result[status].item.status, status);
    }
    assert.equal(result.cancelled.title, 'Compaction cancelled');
    assert.match(result.error.text, /Summarizer disconnected/);
  } finally {
    if (child && child.exitCode === null) child.kill('SIGTERM');
    await vite.close();
    rmSync(viteCacheDir, { recursive: true, force: true });
    rmSync(temporaryUserData, { recursive: true, force: true });
  }
});
