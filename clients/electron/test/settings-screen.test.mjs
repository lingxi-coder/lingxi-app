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

// This scenario used to select `voice` (the one remaining `implemented:
// false` page after Task 19) and assert `placeholderKind === 'not-
// implemented'`, proving the shell renders an explicit placeholder rather
// than a blank panel for a declared-but-unbuilt page. Task 9 of the
// desktop-audio-capability plan built `Voice.tsx`, so `voice` is now
// `implemented: true` too (pinned by `settings-nav.test.ts`'s "every
// declared settings page is implemented") — there is no more honest
// `implemented: false` page left anywhere in `SETTINGS_NAV` for this
// scenario to demonstrate the `not-implemented` placeholder with, so the
// scenario and this test were retired rather than kept pointed at a page
// that no longer proves anything. `voice` now gets the same positive
// "renders real content, not a placeholder" check as every other page —
// see `all six Task 18 pages...` below (extended to eight).

test('a malformed settings snapshot surfaces as an error banner instead of throwing through the render', async () => {
  const { state } = await runScenario('malformed-snapshot');
  assert.equal(state.hasSnapshotError, true);
});

test('all six Task 18 pages plus Task 19\'s raw-json and Task 9\'s voice are actually registered in PAGE_CONTENT, not just declared in nav.ts', async () => {
  // Task 18 fix round 1, Important: the older registration test only
  // checked `nav.ts`'s `implemented` flag — true before Task 18's diff too,
  // since `nav.ts` was untouched. This checks the thing Task 18 actually
  // added: selecting each page renders real content (`placeholderKind ===
  // null`), not the "not wired yet" placeholder a forgotten `PAGE_CONTENT`
  // entry would silently fall back to. Task 19 extended the same check to
  // `raw-json`; Task 9 of the desktop-audio-capability plan extends it to
  // `voice` — the actual instrument (not just a source-text guard) that
  // `Voice.tsx` is reachable and mounted, per that task's own warning that
  // a wiring check can pass while the code it names stays dead.
  const { placeholderKinds, afterHooksEscapeHatch } = await runScenario('page-content');
  for (const id of ['permissions', 'tools-agent', 'skills', 'mcp', 'hooks', 'plugins', 'raw-json', 'voice']) {
    assert.equal(placeholderKinds[id], null, `${id} must render real content, not a placeholder`);
  }
  // hooksPageModel().escapeHatch actually drives the button's navigation
  // target: clicking it must land on `raw-json`'s REAL content (Task 19
  // registered it), not a no-op and not a placeholder.
  assert.equal(afterHooksEscapeHatch.placeholderKind, null, 'the escape-hatch button must navigate to raw-json, which now renders real content');
});

test('switching layers re-seeds a dirty draft field instead of leaving stale text next to a different layer\'s data', async () => {
  // Task 18 fix round 1, Critical: `ToolsAgent`'s `enabledTools` input and
  // `Plugins`' per-plugin config textarea are both seeded via `useState`'s
  // one-time initializer with no re-seed on `editingLayer` change and no
  // remount `key` from the shell — so a save after switching layers could
  // write one layer's stale text into a different layer entirely
  // (`enabledTools` is `ConcatDedup`, so that duplicates permanently).
  const {
    toolsInitial, toolsDirty, toolsAfterSwitch,
    configInitial, configDirty, configAfterSwitch,
    enabledPluginsToggle,
  } = await runScenario('layer-reseed');

  assert.equal(toolsInitial, 'Bash', 'the user layer set enabledTools to ["Bash"]');
  assert.equal(toolsDirty, 'Dirty,Value', 'the field must reflect what was typed before any layer switch');
  assert.equal(
    toolsAfterSwitch, '',
    'switching to the project layer (which set nothing) must re-seed the field to empty, not leave the dirty text or the old user-layer value sitting there',
  );

  assert.equal(configInitial, JSON.stringify({ from: 'user' }, null, 2));
  assert.equal(configDirty, 'not even json', 'the textarea must reflect what was typed before any layer switch');
  assert.equal(
    configAfterSwitch, JSON.stringify({ from: 'project' }, null, 2),
    'switching from user to project must re-seed the SAME plugin id\'s textarea with project\'s own config, not the dirty text or the stale user-layer value',
  );

  // Task 18 fix round 2, Critical: the THIRD instance of the same defect —
  // `PluginToggleRow`'s `useRef` primed on `user`'s truthy config object,
  // then (without the fix) surviving the switch to `project` where the
  // same id is `false`. Toggling it ON in `project` must write a fresh
  // `true`, not `user`'s remembered `{config:"A"}`.
  assert.ok(enabledPluginsToggle, 'toggling the plugin on in the project layer must dispatch a write');
  assert.equal(enabledPluginsToggle.destination, 'project', 'the write must target the layer actually being edited');
  assert.deepEqual(
    enabledPluginsToggle.patch, { enabledPlugins: { 'a@b': true } },
    'toggling on in the project layer must write a fresh `true`, not the user layer\'s remembered config object',
  );
});

test('switching layers remounts the page component (key={editingLayer}), independent of any one page\'s own re-seed effect', async () => {
  // Task 18 fix round 2: pins the STRUCTURAL fix directly. Detected via DOM
  // node identity (an expando property surviving or not), not focus —
  // clicking the layer tab to cause the switch would itself move focus to
  // the tab button regardless of remounting.
  const { markerBeforeSwitch, markerAfterSwitch, markerAfterUnrelatedRerender } = await runScenario('remount-on-layer-switch');

  assert.equal(markerBeforeSwitch, 'sentinel-before-switch', 'sanity check: the marker must attach before any switch');
  assert.equal(
    markerAfterSwitch, null,
    'switching editingLayer must remount the page — the marker must NOT survive on a fresh DOM node',
  );

  // Negative control: an unrelated re-render (a fresh snapshot, same page,
  // same layer) must NOT be mistaken for a remount by this same detection
  // mechanism — otherwise this test would trivially pass for the wrong
  // reason (a marker that never survives ANY re-render, remount or not).
  assert.equal(
    markerAfterUnrelatedRerender, 'sentinel-no-layer-change',
    'a re-render that does not change editingLayer must NOT remount the page — the marker must survive',
  );
});

test('adding an allow rule dispatches exactly what capturePermissionEdit would produce, not an inline shape that happens to agree with it today', async () => {
  const { lastPermissionRuleCall } = await runScenario('permission-rule-dispatch');
  assert.deepEqual(lastPermissionRuleCall, {
    destination: 'user',
    behavior: 'allow',
    add: ['Bash(ls:*)'],
    remove: [],
  });
});
