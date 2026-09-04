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
const electronDriver = join(fixtureRoot, 'audio-request-electron.mjs');

/**
 * Proves BY EXECUTION that `useBridge` answers the engine's audio requests.
 *
 * The subscription lives in a `useEffect`, and the `.test.ts` suite's only
 * renderer is `react-dom/server`'s `renderToString`, which skips effects — so
 * no unit test in this repo can watch the subscription actually happen. The
 * source-text guards in `audio-requests.test.ts` are a weak substitute, and
 * were measured to be weak: an `includes()` check stayed green through an
 * `if (false && …)` mutation. This boots a real Vite server inside a real
 * Electron renderer, where effects run, delivers a real `audio_request`
 * envelope, and reads back the real `audio_response`.
 *
 * `window.lingxi` is a stand-in for the preload bridge (see the fixture's
 * header for exactly what is real and what is not) — the main-process half of
 * the round trip is covered by the gate-seam test in `audio-requests.test.ts`.
 */
test('a real mounted renderer answers a real audio_request with a real audio_response', async () => {
  const viteCacheDir = mkdtempSync(join(tmpdir(), 'lingxi-audio-request-vite-'));
  const vite = await createServer({
    root: fixtureRoot,
    cacheDir: viteCacheDir,
    configFile: false,
    logLevel: 'error',
    server: { host: '127.0.0.1', port: 0, strictPort: false, hmr: false },
    plugins: [react()],
    resolve: {
      alias: {
        '@renderer': resolve(electronRoot, 'src/renderer'),
      },
    },
  });
  const temporaryUserData = mkdtempSync(join(tmpdir(), 'lingxi-audio-request-electron-'));
  const childOutput = [];
  const childError = [];
  let child;

  try {
    await vite.listen();
    const address = vite.httpServer?.address();
    assert.ok(address && typeof address === 'object' && address.port, 'Vite fixture server did not bind a port');
    const fixtureUrl = `http://127.0.0.1:${address.port}/audio-request-fixture.html`;
    child = spawn(process.execPath, [electronBinary, electronDriver, fixtureUrl], {
      cwd: electronRoot,
      env: { ...process.env, ELECTRON_ENABLE_LOGGING: '0', ELECTRON_IS_DEV: '0', LINGXI_TEST_USER_DATA: temporaryUserData },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    child.stdout.on('data', (chunk) => childOutput.push(String(chunk)));
    child.stderr.on('data', (chunk) => childError.push(String(chunk)));

    const result = await new Promise((resolveResult, rejectResult) => {
      const timeout = setTimeout(() => {
        child.kill('SIGTERM');
        rejectResult(new Error(`Electron audio request fixture timed out\n${childError.join('')}`));
      }, 30_000);
      child.once('error', (error) => { clearTimeout(timeout); rejectResult(error); });
      child.once('exit', (code, signal) => {
        clearTimeout(timeout);
        const output = childOutput.join('').trim().split('\n').at(-1);
        if (code !== 0 || !output) {
          rejectResult(new Error(`Electron audio request fixture exited ${code ?? signal}\n${childError.join('')}`));
          return;
        }
        try { resolveResult(JSON.parse(output)); }
        catch (error) { rejectResult(new Error(`Invalid audio request output: ${output}`, { cause: error })); }
      });
    });

    // One live subscription: StrictMode mounts, unmounts and remounts, so a
    // subscription that failed to clean up would answer every request twice.
    assert.equal(result.listenerCount, 1, 'useBridge must hold exactly one engine-event subscription');

    // The whole point: a request went in, an answer came out.
    assert.equal(result.isRecording.length, 1, 'an is_recording request must be answered exactly once');
    assert.deepEqual(result.isRecording[0].command, {
      type: 'audio_response',
      request_id: 41,
      result: { type: 'recording_state', recording: false },
    });

    // This fixture intentionally omits the native-audio preload surface. The
    // renderer must still answer exactly once with an explicit unavailable
    // failure instead of falling back to browser speech APIs.
    assert.equal(result.transcribe.length, 1, 'a transcribe request must be answered exactly once');
    assert.deepEqual(result.transcribe[0].command, {
      type: 'audio_response',
      request_id: 42,
      result: {
        type: 'failed',
        kind: 'unavailable',
        message: 'native audio is unavailable on this host',
      },
    });

    // A response the engine rejects must not take the renderer down.
    assert.equal(result.dropped.length, 1);
    assert.equal(result.crashed, false, 'a rejected audio_response must not raise an uncaught error in the renderer');
    assert.equal(result.stillAlive, true, 'the renderer must survive a response the engine dropped');
  } finally {
    if (child && child.exitCode === null) {
      child.kill('SIGTERM');
      await new Promise((resolveExit) => {
        const timeout = setTimeout(resolveExit, 2000);
        child.once('exit', () => { clearTimeout(timeout); resolveExit(); });
      });
    }
    await vite.close();
    rmSync(viteCacheDir, { recursive: true, force: true });
    rmSync(temporaryUserData, { recursive: true, force: true });
  }
});
