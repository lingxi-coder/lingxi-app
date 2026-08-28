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
const electronDriver = join(fixtureRoot, 'settings-screen-electron.mjs');

async function runScenario(scenario) {
  const viteCacheDir = mkdtempSync(join(tmpdir(), `lingxi-settings-screen-vite-${scenario}-`));
  const vite = await createServer({
    root: fixtureRoot,
    cacheDir: viteCacheDir,
    configFile: false,
    logLevel: 'error',
    server: { host: '127.0.0.1', port: 0, strictPort: false, hmr: false },
    plugins: [react()],
    resolve: { alias: { '@renderer': resolve(electronRoot, 'src/renderer') } },
  });
  const temporaryUserData = mkdtempSync(join(tmpdir(), `lingxi-settings-screen-electron-${scenario}-`));
  const childOutput = [];
  const childError = [];
  let child;

  try {
    await vite.listen();
    const address = vite.httpServer?.address();
    assert.ok(address && typeof address === 'object' && address.port, 'Vite fixture server did not bind a port');
    const fixtureUrl = `http://127.0.0.1:${address.port}/settings-screen-fixture.html`;
    child = spawn(process.execPath, [electronBinary, electronDriver, fixtureUrl], {
      cwd: electronRoot,
      env: {
        ...process.env,
        ELECTRON_ENABLE_LOGGING: '0',
        ELECTRON_IS_DEV: '0',
        LINGXI_TEST_USER_DATA: temporaryUserData,
        LINGXI_SETTINGS_SCREEN_SCENARIO: scenario,
      },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    child.stdout.on('data', (chunk) => childOutput.push(String(chunk)));
    child.stderr.on('data', (chunk) => childError.push(String(chunk)));

    return await new Promise((resolveResult, rejectResult) => {
      const timeout = setTimeout(() => {
        child.kill('SIGTERM');
        rejectResult(new Error(`Electron SettingsScreen "${scenario}" fixture timed out\n${childError.join('')}`));
      }, 20_000);
      child.once('error', (error) => { clearTimeout(timeout); rejectResult(error); });
      child.once('exit', (code, signal) => {
        clearTimeout(timeout);
        const output = childOutput.join('').trim().split('\n').at(-1);
        if (code !== 0 || !output) {
          rejectResult(new Error(`Electron SettingsScreen "${scenario}" fixture exited ${code ?? signal}\n${childError.join('')}`));
          return;
        }
        try { resolveResult(JSON.parse(output)); }
        catch (error) { rejectResult(new Error(`Invalid SettingsScreen "${scenario}" output: ${output}`, { cause: error })); }
      });
    });
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
}

test('the layer switcher appears only on layered pages', async () => {
  const { permissions, diagnostics, mcp, customProviders, rawJson } = await runScenario('layer-switcher');
  assert.equal(permissions.hasLayerSwitcher, true, 'permissions is layered and must offer user/project/local');
  assert.equal(diagnostics.hasLayerSwitcher, false, 'diagnostics is client-owned and must not show a layer switcher');
  // `permissions` (编码, layered) vs `diagnostics` (高级, not layered) alone
  // cannot fail a regression that keys the switcher off `group === '编码'`
  // instead of `page.layered` — both pages agree on both properties. These
  // three are the ones that actually separate the two rules.
  assert.equal(mcp.hasLayerSwitcher, false, 'mcp is inside 编码 but NOT layered — its own three-scope storage, no layer switcher');
  assert.equal(customProviders.hasLayerSwitcher, true, 'custom-providers is outside 编码 but IS layered');
  assert.equal(rawJson.hasLayerSwitcher, true, 'raw-json is outside 编码 but IS layered');
});

test('project and local tabs are disabled with no project open, with the reason shown as visible text', async () => {
  const { withoutProject, withProject } = await runScenario('project-tabs');
  assert.equal(withoutProject.userDisabled, false, 'the user layer needs no project and must stay editable');
  assert.equal(withoutProject.projectDisabled, true);
  assert.equal(withoutProject.localDisabled, true);
  // The reason must be visible TEXT, not just a `title=` tooltip: Chromium
  // does not dispatch the pointer events a native tooltip needs on a
  // DISABLED control, and a tooltip is invisible to keyboard/screen-reader
  // users regardless.
  assert.match(withoutProject.layerSwitcherReasonText ?? '', /项目/, 'the disabled reason must be visible text, not a title= tooltip');
  // And the disabling is really keyed on the project, not permanent: once one
  // opens, both re-enable — proving the assertion above isn't just "always disabled".
  assert.equal(withProject.projectDisabled, false);
  assert.equal(withProject.localDisabled, false);
  assert.equal(withProject.userDisabled, false);
  assert.equal(withProject.layerSwitcherReasonText, null, 'once a project is open there is nothing to explain');
});

test('the pending-settings banner comes from pendingKeys alone, and its restart action respects a turn in flight', async () => {
  const { noSnapshot, pending, midTurn, afterRestart, resolved } = await runScenario('pending-banner');
  assert.equal(noSnapshot.hasBanner, false, 'with no snapshot yet there is nothing to diff, so no banner');
  assert.equal(pending.hasBanner, true, 'effective and active disagree on `model`, so the banner must appear');
  assert.match(pending.bannerText ?? '', /1 项/, 'exactly one key (`model`) differs, not `theme`');
  assert.equal(pending.restartButtonDisabled, false);
  assert.equal(midTurn.restartButtonDisabled, true, 'a turn in flight must disable restart, not fail silently on click');
  assert.match(midTurn.restartDisabledReasonText ?? '', /对话|回合/, 'the reason must be visible text, not a title= tooltip');
  assert.equal(afterRestart.restartCalls, 1, 'restart goes through bridge.restartBridge, the existing path');
  assert.equal(resolved.hasBanner, false, 'once active catches up to effective, the banner must go away');
});

test('a rejected restart surfaces its error visibly instead of being swallowed, and a later success clears it', async () => {
  const { afterFailure, afterSuccess } = await runScenario('restart-error');
  assert.equal(afterFailure.hasRestartError, true, 'the shell covers <ErrorBanner>, so a caught restart failure must render its own visible error');
  assert.match(afterFailure.restartErrorText ?? '', /cancel the active turn/, 'the ACTUAL host-provided reason must reach the user, not a generic message');
  assert.equal(afterSuccess.hasRestartError, false, 'a later successful restart must clear the earlier failure banner');
});

test('opening settings while the session is loading sends nothing, and the snapshot fetch retries once the session is ready — with no extra action', async () => {
  const { initial, whileLoading, afterReady } = await runScenario('session-loading-guard');
  // Counts are relative to each other, not a hardcoded number: React
  // StrictMode's dev-mode mount→cleanup→mount probe re-runs an effect with
  // no cleanup function twice, so exactly how many times the initial,
  // already-ready mount fires the fetch is an artifact of the test harness,
  // not something this scenario is testing.
  assert.equal(whileLoading.refreshCalls, initial.refreshCalls, 'a session still loading must not receive a fetch attempt at all');
  assert.ok(afterReady.refreshCalls > whileLoading.refreshCalls, 'once loading finishes the effect must retry on its own, with no separate action');
});

test('the settings-focus trap moves focus in on mount, wraps Tab both directions, and restores focus to the opener on close', async () => {
  const { onMount, afterForwardTab, afterBackwardTab, afterClose } = await runScenario('focus-trap');
  assert.equal(onMount.activeElementAriaLabel, 'Close settings', 'mounting an aria-modal dialog must move focus INTO it, not leave it in the background');
  assert.equal(afterForwardTab.activeElementAriaLabel, '搜索设置', 'forward Tab from the last focusable (close) must wrap to the first (search), not escape the dialog');
  assert.equal(afterBackwardTab.activeElementAriaLabel, 'Close settings', 'shift+Tab from the first focusable (search) must wrap to the last (close)');
  assert.equal(afterClose.activeElementId, 'opener', 'closing must restore focus to whatever had it before the dialog mounted');
});

test('an unimplemented page renders an explicit placeholder, not a blank panel — implemented pages without content yet get a different, honest message', async () => {
  const { voice, notWired } = await runScenario('placeholder');
  assert.equal(voice.placeholderKind, 'not-implemented', 'voice is implemented:false and must say so plainly');
  // `permissions`, not `diagnostics`: Task 16 registers the five
  // client-owned pages (including diagnostics), so `permissions` — engine-owned,
  // registered only in a later task — is the still-honest "not wired yet" example now.
  assert.equal(notWired.placeholderKind, 'not-wired', 'permissions is implemented:true but has no page component registered yet');
});

test('a malformed settings snapshot surfaces as an error banner instead of throwing through the render', async () => {
  const { state } = await runScenario('malformed-snapshot');
  assert.equal(state.hasSnapshotError, true);
});
