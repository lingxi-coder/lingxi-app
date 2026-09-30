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
const electronDriver = join(fixtureRoot, 'scheduled-cron-electron.mjs');

test('scheduled tasks create, edit, retain failed edits, delete, and refresh through the backend', async () => {
  const viteCacheDir = mkdtempSync(join(tmpdir(), 'lingxi-scheduled-cron-vite-'));
  const temporaryUserData = mkdtempSync(join(tmpdir(), 'lingxi-scheduled-cron-electron-'));
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
    const fixtureUrl = `http://127.0.0.1:${address.port}/scheduled-cron-fixture.html`;
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
        rejectResult(new Error(`Electron scheduled task fixture timed out\n${errors.join('')}`));
      }, 45_000);
      child.once('error', (error) => { clearTimeout(timeout); rejectResult(error); });
      child.once('exit', (code, signal) => {
        clearTimeout(timeout);
        const line = output.join('').trim().split('\n').at(-1);
        if (code !== 0 || !line) {
          rejectResult(new Error(`Electron scheduled task fixture exited ${code ?? signal}\n${errors.join('')}`));
          return;
        }
        try { resolveResult(JSON.parse(line)); }
        catch (error) { rejectResult(new Error(`Invalid Electron scheduled task fixture output: ${line}`, { cause: error })); }
      });
    });

    assert.equal(result.dirtyPollRequests, 0);
    assert.equal(result.pollUpdated, true);
    assert.deepEqual(result.copyDraft, {mode: 'new_session', projectEditable: true});
    assert.equal(result.copied.jobs.length, 2);
    assert.equal(result.copied.jobs[1].automation.ownedSessionId, undefined);
    assert.equal(result.copied.jobs[1].automation.runs, undefined);
    assert.deepEqual(result.narrow, {listHidden: true, overflow: false});
    assert.deepEqual(result.duringMutation, ['update'], 'navigation must not race an in-flight mutation with stale list data');
    assert.equal(result.created.jobs.length, 1);
    assert.equal(result.created.jobs[0].cron, '25 18 * * 5');
    assert.equal(result.created.jobs[0].expires_at, result.picker.expectedExpiry);
    assert.equal(result.picker.invalidHourBlocked, true);
    assert.equal(result.picker.doneFocus, 'At');
    assert.equal(result.picker.timeDescription, '18:25');
    assert.equal(result.picker.calendarTabStops, 1);
    assert.equal(result.picker.escapeFocus, 'At');
    assert.equal(result.picker.monthRollover, true);
    assert.equal(result.picker.previousMonth, true);
    assert.equal(result.picker.calendarFocus, true);
    assert.equal(result.picker.narrowFits, true);
    assert.equal(result.picker.enterCloses, true);
    assert.equal(result.created.jobs[0].durable, true);
    assert.match(result.created.jobs[0].prompt, /Weekly review/);
    assert.equal(result.configured.jobs[0].expires_at, result.picker.expectedExpiry, 'editing other fields must preserve the selected expiry instant');
    assert.equal(result.configured.jobs[0].automation.targetSessionId, 'fixture-session');
    assert.deepEqual(result.configured.jobs[0].automation.reasoning, {type:'level',id:'high'});
    assert.equal(result.configured.jobs[0].automation.notificationPolicy, 'failed');
    assert.equal(result.activeCount, 0);
    assert.equal(result.pausedCount, 1);
    assert.equal(result.reopenedTitle, 'Edited weekly review');
    assert.match(result.rejected.error, /Fixture backend rejected save/);
    assert.equal(result.rejected.title, 'Unsaved title');
    assert.match(result.rejected.state.jobs[0].prompt, /Edited weekly review/);
    assert.doesNotMatch(result.rejected.state.jobs[0].prompt, /Unsaved title/);
    assert.equal(result.deleted.jobs.length, 0);
    assert.equal(result.refreshed.jobs.length, 1);
    assert.equal(result.refreshed.jobs[0].id, 'external');
    assert.deepEqual(result.refreshed.requests.map((request) => request.action), ['list', 'create', 'update', 'update', 'update', 'delete', 'list']);
  } finally {
    if (child && child.exitCode === null && child.signalCode === null) {
      await new Promise((resolveExit) => {
        const forceKill = setTimeout(() => child.kill('SIGKILL'), 3_000);
        child.once('exit', () => { clearTimeout(forceKill); resolveExit(); });
        child.kill('SIGTERM');
      });
    }
    await vite.close();
    rmSync(viteCacheDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
    rmSync(temporaryUserData, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  }
});
