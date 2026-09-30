/**
 * When the model picker may be refused, and when it may not.
 *
 * The rule: only while nothing is up to take the change. A composer that cannot
 * SEND — no provider connected, no trusted workspace, a catalog that never
 * arrived — must still let the user change model, because that control is the
 * way out of those states. A disconnected engine may refuse it, because the
 * switch would be lost; and it must come back by itself when the engine does.
 *
 * `desktop.models` comes from one `list_models` reply per connection. Activating
 * Codex authentication restarts the engine mid-connection, and the restarted
 * engine reports `connected` while the activation is still running — the window
 * in which the renderer's whole listing batch used to be refused. The catalog
 * stayed empty, and the composer's pill, `disabled` on an empty catalog, could
 * not be opened again for the life of the app. Both halves are asserted here in
 * a real window: the pill opens, and opening it asks for the catalog again.
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
const driver = join(fixtureRoot, 'model-picker-recovery-electron.mjs');

test('the model control is refused only when nothing is up to take the change', async () => {
  const vite = await createServer({
    root: fixtureRoot,
    configFile: false,
    logLevel: 'error',
    server: { host: '127.0.0.1', port: 0, strictPort: false, hmr: false },
    plugins: [react()],
    resolve: { alias: { '@renderer': resolve(electronRoot, 'src/renderer') } },
  });
  const temporaryUserData = mkdtempSync(join(tmpdir(), 'lingxi-model-picker-recovery-'));
  const childOutput = [];
  const childError = [];
  let child;

  try {
    await vite.listen();
    const address = vite.httpServer?.address();
    assert.ok(address && typeof address === 'object' && address.port, 'Vite fixture server did not bind a port');
    const fixtureUrl = `http://127.0.0.1:${address.port}/composer-draft-fixture.html`;
    child = spawn(process.execPath, [electronBinary, driver, fixtureUrl], {
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
        rejectResult(new Error(`Electron model picker recovery fixture timed out\n${childError.join('')}`));
      }, 45_000);
      child.once('error', (error) => { clearTimeout(timeout); rejectResult(error); });
      child.once('exit', (code, signal) => {
        clearTimeout(timeout);
        const output = childOutput.join('').trim().split('\n').at(-1);
        if (code !== 0 || !output) {
          rejectResult(new Error(`Electron model picker recovery fixture exited ${code ?? signal}\n${childError.join('')}`));
          return;
        }
        try { resolveResult(JSON.parse(output)); }
        catch (error) { rejectResult(new Error(`Invalid Electron model picker recovery output: ${output}`, { cause: error })); }
      });
    });

    const { empty, opened, listed, notReady, openedWhileNotReady, disconnected, reconnected } = result;

    // Empty is "not heard yet", not "nothing to pick": the engine always lists
    // at least the current model, whose name the pill is already showing.
    assert.equal(empty.disabled, false, 'the model pill was disabled with an empty catalog');
    assert.ok(empty.reachable, 'the model pill was not reachable at its own centre');
    assert.match(empty.aria, /^Model: Auto, reasoning /, 'the pill stopped naming the current model');
    // The click below is the only thing that may ask; a picker that refreshed on
    // its own would make the assertion after it meaningless.
    assert.equal(empty.refreshes, 0);

    assert.ok(opened, 'the driver stopped before opening the picker');
    assert.equal(opened.refreshes, 1, 'opening the picker did not re-request the model catalog');

    // And the reply it asked for lands in the list.
    assert.equal(listed.rows, 4);
    assert.equal(listed.firstRowLabel, 'OpenRouter Auto');

    // A composer that cannot SEND — no provider connected, no trusted workspace
    // — must still let the model change; that control is the way out. The
    // attach button proves `ready` really did go false.
    assert.equal(notReady.attach, true, 'the fixture never actually left the ready state');
    assert.equal(notReady.trigger, false, 'the model pill was disabled by a composer that merely cannot send');
    assert.ok(openedWhileNotReady, 'the picker would not open while the composer could not send');

    // The one refusal that is not a lie: nothing is up to take the change.
    assert.equal(disconnected.trigger, true, 'the model pill stayed live with no engine to take the switch');
    assert.equal(disconnected.menu, false, 'an open picker survived the engine it was commanding going away');
    // And it recovers by itself — no app restart, which is what the original
    // defect required.
    assert.equal(reconnected, false, 'the model pill did not come back when the engine did');
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
