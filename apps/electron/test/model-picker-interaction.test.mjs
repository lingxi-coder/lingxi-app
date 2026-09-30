/**
 * Where the composer's model list lands in a real window.
 *
 * The list flies out to the LEFT of the picker menu, and the menu is pinned to
 * the right end of the composer toolbar. `.desktop-workspace-upper` clips at
 * `overflow: hidden` from the sidebar's edge, so in a narrow workspace the
 * flyout was not merely cramped — its left portion, which is where every model
 * name is drawn, was cut off, leaving a blank sliver. Nothing in the unit tests
 * could see that: the widths were all "correct", the window just wasn't as wide
 * as they assumed. So this measures the rendered rectangles, and hit-tests the
 * first row's label, in an Electron window sized like the one that showed the
 * bug.
 */

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
const modelPickerDriver = join(fixtureRoot, 'model-picker-electron.mjs');

test('the model list stays inside the workspace that clips it', async () => {
  const vite = await createServer({
    root: fixtureRoot,
    configFile: false,
    logLevel: 'error',
    server: { host: '127.0.0.1', port: 0, strictPort: false, hmr: false },
    plugins: [react()],
    resolve: { alias: { '@renderer': resolve(electronRoot, 'src/renderer') } },
  });
  const temporaryUserData = mkdtempSync(join(tmpdir(), 'lingxi-model-picker-electron-'));
  const childOutput = [];
  const childError = [];
  let child;

  try {
    await vite.listen();
    const address = vite.httpServer?.address();
    assert.ok(address && typeof address === 'object' && address.port, 'Vite fixture server did not bind a port');
    const fixtureUrl = `http://127.0.0.1:${address.port}/composer-draft-fixture.html`;
    child = spawn(process.execPath, [electronBinary, modelPickerDriver, fixtureUrl], {
      cwd: electronRoot,
      env: {
        ...process.env,
        ELECTRON_ENABLE_LOGGING: '0',
        ELECTRON_IS_DEV: '0',
        LINGXI_TEST_USER_DATA: temporaryUserData,
      },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    child.stdout.on('data', (chunk) => childOutput.push(String(chunk)));
    child.stderr.on('data', (chunk) => childError.push(String(chunk)));

    const result = await new Promise((resolveResult, rejectResult) => {
      const timeout = setTimeout(() => {
        child.kill('SIGTERM');
        rejectResult(new Error(`Electron model picker fixture timed out\n${childError.join('')}`));
      }, 20_000);
      child.once('error', (error) => { clearTimeout(timeout); rejectResult(error); });
      child.once('exit', (code, signal) => {
        clearTimeout(timeout);
        const output = childOutput.join('').trim().split('\n').at(-1);
        if (code !== 0 || !output) {
          rejectResult(new Error(`Electron model picker fixture exited ${code ?? signal}\n${childError.join('')}`));
          return;
        }
        try { resolveResult(JSON.parse(output)); }
        catch (error) { rejectResult(new Error(`Invalid Electron model picker output: ${output}`, { cause: error })); }
      });
    });

    const { narrow, afterBack, wide } = result;

    // A 900px window with the default sidebar: the flyout does not fit beside
    // the menu, so the list drills down over it instead of off the edge.
    assert.equal(narrow.innerWidth, 900);
    assert.ok(
      narrow.submenu.left >= narrow.dock.left,
      `model list starts at ${narrow.submenu.left}, left of the workspace edge at ${narrow.dock.left}`,
    );
    assert.ok(
      narrow.submenu.right <= narrow.dock.right,
      `model list ends at ${narrow.submenu.right}, past the workspace edge at ${narrow.dock.right}`,
    );
    // The names are the reason the panel exists; a clipped panel keeps its
    // width and loses them, so the rectangle alone is not enough.
    assert.equal(narrow.firstRowLabel, 'OpenRouter Auto');
    assert.ok(narrow.firstRowReachable, 'the first model row was not reachable at its own label');
    assert.equal(narrow.submenu.width, 390);

    // Covering the menu would be a dead end without a way back to it.
    assert.deepEqual(afterBack, { rowLabel: 'ModelAuto', rowReachable: true });

    // Given the room, it still flies out beside the menu rather than covering
    // it — the fix is a fallback, not a new permanent layout.
    assert.equal(wide.innerWidth, 1400);
    assert.ok(
      wide.submenu.right <= wide.menu.left,
      `model list ends at ${wide.submenu.right}, overlapping the menu that starts at ${wide.menu.left}`,
    );
    assert.ok(
      wide.submenu.left >= wide.dock.left,
      `model list starts at ${wide.submenu.left}, left of the workspace edge at ${wide.dock.left}`,
    );
    assert.equal(wide.submenu.width, 390);
    assert.ok(wide.firstRowReachable, 'the first model row was not reachable at its own label');
  } finally {
    if (child && child.exitCode === null) {
      child.kill('SIGTERM');
      await new Promise((resolveExit) => {
        const timeout = setTimeout(resolveExit, 2000);
        child.once('exit', () => { clearTimeout(timeout); resolveExit(); });
      });
    }
    await vite.close();
    rmSync(temporaryUserData, { recursive: true, force: true });
  }
});
