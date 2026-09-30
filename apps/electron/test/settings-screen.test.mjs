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
    const fixtureTheme = process.env.LINGXI_SETTINGS_SCREEN_THEME === 'dark' ? 'dark' : 'light';
    const fixtureUrl = `http://127.0.0.1:${address.port}/settings-screen-fixture.html?theme=${fixtureTheme}`;
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
  const { permissions, diagnostics, mcp, customProviders } = await runScenario('layer-switcher');
  assert.equal(permissions.hasLayerSwitcher, true, 'permissions is layered and must offer user/project/local');
  assert.equal(diagnostics.hasLayerSwitcher, false, 'diagnostics is client-owned and must not show a layer switcher');
  // `permissions` (编码, layered) vs `diagnostics` (高级, not layered) alone
  // cannot fail a regression that keys the switcher off `group === '编码'`
  // instead of `page.layered` — both pages agree on both properties. These
  // three are the ones that actually separate the two rules.
  assert.equal(mcp.hasLayerSwitcher, false, 'mcp is inside 编码 but NOT layered — its own three-scope storage, no layer switcher');
  assert.equal(customProviders.hasLayerSwitcher, true, 'custom-providers is outside 编码 but IS layered');
});

test('settings exposes a titlebar drag strip and flush right scroll track', async () => {
  const result = await runScenario('window-chrome');
  assert.equal(result.hasDragRegion, true);
  assert.equal(result.dragRegionMode, 'drag');
  assert.equal(result.dragRegionHeight, '18px');
  assert.equal(result.scrollPaddingTop, '0px');
  assert.equal(result.scrollPaddingBottom, '0px');
});

test('the four configuration managers render without horizontal dialog overflow', async () => {
  const result = await runScenario('visual-admin');
  assert.deepEqual(result.pages, ['skills', 'mcp', 'plugins', 'hooks']);
  for (const [page, state] of Object.entries(result.layout)) {
    assert.equal(state.placeholderKind, null, `${page} must render its real settings page`);
    assert.equal(state.horizontalOverflow, false, `${page} must fit the settings dialog horizontally`);
  }
  assert.equal(result.layout.hooks.structuredHookEditor, true, 'hooks must expose event/group/handler controls');
  assert.equal(result.layout.hooks.advancedHookJson, true, 'hooks must retain the advanced JSON editor');
  assert.equal(result.layout.mcp.structuredMcpEditor, true, 'MCP must expose transport-aware structured controls');
  assert.equal(result.layout.plugins.manifestPluginEditor, true, 'plugins must render manifest-driven configuration fields');
});

test('custom providers import, save credentials, preserve edits and isolate layers', async () => {
  const result = await runScenario('custom-providers');
  assert.equal(result.locked.projectDisabled, true, 'cannot switch destination during save');
  assert.equal(result.saved.lastEngineSettingsPatch.destination, 'user');
  assert.deepEqual(result.saved.credentialWrites, ['customlab']);
  assert.equal(JSON.stringify(result.saved.lastEngineSettingsPatch).includes('test-only-secret'), false);
  assert.equal(result.fixedId, true);
  const edited = result.edited.lastEngineSettingsPatch.patch.providers;
  assert.deepEqual(Object.keys(edited), ['customlab']);
  assert.deepEqual(edited.customlab.models[0].aliases, ['fast']);
  assert.deepEqual(edited.customlab.models[0].capabilities, { reasoning: true });
  assert.deepEqual(edited.customlab.models[0].metadata, { display_name: 'Model A' });
  assert.equal(edited.customlab.supportsWebsockets, false);
  assert.equal(edited.customlab.models.length, 1);
  assert.equal(edited.customlab.models[0].id, 'model-renamed');
  assert.deepEqual(edited.customlab.pricing, { 'model-renamed': { inputPerMtok: 1, outputPerMtok: 2 } });
  assert.ok(result.invalidText);
  assert.equal(result.invalidText.includes('test-only-secret'), false);
  assert.equal(result.conflictSkipped, true);
  const imported = result.partial.lastEngineSettingsPatch.patch.providers;
  assert.deepEqual(Object.keys(imported).sort(), ['customlab', 'imported']);
  assert.equal(imported.customlab.baseUrl, 'https://changed.example/v1');
  assert.equal(imported.imported.type, 'anthropic');
  assert.equal(JSON.stringify(imported).includes('test-only-import-secret'), false);
  assert.deepEqual(result.retried.credentialWrites, ['customlab', 'imported']);
  assert.equal(result.clearedOnSwitch, true);
  assert.equal(result.overflow, false);
  assert.match(result.retryError, /正整数/);
  assert.equal(result.malformedRowSurvives, true);
  assert.deepEqual(result.dualAuth, { mode: 'key', keyPresent: true });
  assert.equal(result.legacy.apiKeyEnv, 'LEGACY_KEY');
  assert.deepEqual(result.legacy.models, [{ id: 'legacy-model' }]);
  assert.deepEqual(result.toggledPricing, { 'final-model': { inputPerMtok: 1, outputPerMtok: 2 }, other: { inputPerMtok: 3, outputPerMtok: 4 } });
  assert.equal(result.pricingTargetProtected, true);
});

test('provider drafts and pending credential writes do not survive switching sessions', async () => {
  const result = await runScenario('provider-session-isolation');
  assert.equal(result.draftCleared, true);
  assert.deepEqual(result.state.credentialWrites, []);
  assert.equal(result.state.userDisabled, false, 'an interrupted save releases its layer lock');
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

test('the settings shell never renders an engine restart banner', async () => {
  const state = await runScenario('no-engine-banner');
  assert.equal(state.hasBanner, false, 'runtime active/effective differences do not belong in settings chrome');
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

test('settings excludes background window dragging and Back responds to mouse input', async () => {
  const { panelRegion, backgroundRegion, afterClose } = await runScenario('back-click');
  assert.equal(backgroundRegion, 'drag');
  assert.equal(panelRegion, 'no-drag', 'settings must exclude the native drag regions beneath its overlay');
  assert.equal(afterClose.dialogPresent, false);
  assert.equal(afterClose.closeCalls, 1);
});

test('the settings-focus behavior starts at Back, traverses search, and restores the opener on return', async () => {
  const { onMount, afterForwardTab, afterBackwardTab, afterClose } = await runScenario('focus-trap');
  assert.equal(onMount.activeElementAriaLabel, 'Back to app', 'mounting an aria-modal dialog must move focus INTO it, not leave it in the background');
  assert.equal(afterForwardTab.activeElementAriaLabel, '搜索设置', 'Tab from Back moves to search');
  assert.equal(afterBackwardTab.activeElementAriaLabel, 'Back to app', 'Shift+Tab from search returns to Back');
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

test('settings pages and the Hooks configuration-file shortcut render real content', async () => {
  const { placeholderKinds, afterHooksEscapeHatch, hasFiles, fileActions } = await runScenario('page-content');
  for (const id of ['permissions', 'tools-agent', 'skills', 'mcp', 'hooks', 'plugins', 'diagnostics', 'voice']) {
    assert.equal(placeholderKinds[id], null, `${id} must render real content, not a placeholder`);
  }
  assert.equal(hasFiles, true);
  assert.equal(afterHooksEscapeHatch.placeholderKind, null);
  assert.deepEqual(fileActions.opened, ['/test/home/.lingxi/settings.json', '/test/project/.lingxi/settings.local.json']);
  assert.equal(fileActions.missingDisabled, true);
  assert.equal(fileActions.brokenShown, true);
  assert.equal(fileActions.error, 'test open failure');
  assert.equal(fileActions.rawEditor, false);
});

test('switching layers clears dirty settings drafts and reopens plugin config from the new layer', async () => {
  // Layer changes remount the settings page. Verify a dirty Tools Agent field
  // is re-seeded immediately and an extension detail reopened afterward reads
  // the selected plugin's config from the new layer.
  const {
    toolsInitial, toolsDirty, toolsAfterSwitch,
    configInitial, configDirty, configAfterSwitch,
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
    'reopening the same plugin after switching to project must show project config, not the dirty user-layer draft',
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

test('the layer switcher names the project the ENGINE reported, and switching it really re-points the engine', async () => {
  const { beforeSnapshot, onUserLayer, onProjectLayer, pickerOpen, afterSwitch } = await runScenario('layer-project');

  // 每一层都要有一句说明。三个裸标签（用户/项目/本地）不解释任何东西，而它们的
  // 差别有真实后果 —— 「项目」层的文件随仓库提交给整个团队。
  assert.match(onUserLayer.layerDescriptionText ?? '', /本机所有项目/,
    'the user layer must say it applies to every project on this machine');
  assert.match(onProjectLayer.layerDescriptionText ?? '', /提交/,
    'the project layer must say the file is committed and shared with the team');

  // 用户层是本机全局的，给它挂一个项目名就是在暗示一个不存在的作用域。
  assert.equal(onUserLayer.layerProjectName, null, 'the user layer must not claim a project');

  // 种雷：fixture 的 `bootstrap.workspace.path` 是 `/test/project`，而快照里
  // project 层指向 `/test/engine-answer`。读 workspace 的实现会在这里拿到
  // `project`，只有读 `files_json` 的才拿得到 `engine-answer`。
  assert.equal(onProjectLayer.layerProjectName, 'engine-answer',
    'the project name must come from the snapshot files_json, not bootstrap.workspace');
  assert.equal(onProjectLayer.layerProjectPath, '/test/engine-answer',
    'the full path is always shown alongside the name — basenames collide');

  // 快照还没到时不猜：宁可说「还不知道」也不要指着 B 写 A。
  assert.equal(beforeSnapshot.layerProjectName, null);
  assert.match(beforeSnapshot.layerProjectUnknown ?? '', /确认/,
    'with no snapshot yet the UI must say it does not know the project, not invent one');

  assert.equal(pickerOpen.hasProjectPicker, true, 'the switch button must open a project list');
  assert.match(pickerOpen.projectSwitchWarning ?? '', /切换当前会话/,
    'switching the settings project also switches the conversation — say so before it happens');

  // 判据落在**调用序列**上，不是「点了没报错」：`activateProject` 只写元数据，
  // 引擎进程的 `--cwd` 不会因它改变。所以必须还有一次 open/new 把引擎真的
  // 落到新项目上，否则下一次「项目」层写入仍然写进旧项目的文件。
  assert.ok(afterSwitch.navCalls.includes('activate:/test/other'),
    `expected the project to be activated, got ${JSON.stringify(afterSwitch.navCalls)}`);
  assert.ok(
    afterSwitch.navCalls.some((entry) => entry.startsWith('open:/test/other:') || entry.startsWith('new:/test/other')),
    `activateProject alone does not re-point the engine; expected an open/new session call, got ${JSON.stringify(afterSwitch.navCalls)}`,
  );
  // 有历史会话时打开最近的那个，而不是每切一次项目就凭空造一个空会话。
  assert.ok(afterSwitch.navCalls.includes('open:/test/other:newest'),
    `expected the most recently modified session to be opened, got ${JSON.stringify(afterSwitch.navCalls)}`);
});

 test('Skills and MCP use separate list and detail pages with guarded back navigation', async () => {
  const result = await runScenario('configuration-navigation');
  for (const [page, checks] of Object.entries(result)) {
    for (const [name, passed] of Object.entries(checks)) assert.equal(passed, true, `${page}: ${name}`);
  }
});


test('provider region saves a layer and applies only through reconnect', async () => {
  const result = await runScenario('provider-region');
  assert.equal(result.initial, true);
  assert.equal(result.saved.destination, 'user');
  assert.deepEqual(result.saved.patch, { providerRegion: 'china_mainland' });
  assert.equal(result.pending, true);
  assert.equal(result.restarted, true);
});
