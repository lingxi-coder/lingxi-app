import { app, BrowserWindow } from 'electron';

const url = process.argv.find((argument) => argument.startsWith('http://') || argument.startsWith('https://'));
if (!url) throw new Error('fixture URL is required');

const testUserData = process.env.LINGXI_TEST_USER_DATA;
if (testUserData) app.setPath('userData', testUserData);
app.commandLine.appendSwitch('disable-gpu');
app.commandLine.appendSwitch('disable-software-rasterizer');

const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));
async function waitFor(webContents, expression, timeout = 8000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await webContents.executeJavaScript(expression)) return;
    await delay(25);
  }
  throw new Error(`timed out waiting for: ${expression}`);
}

async function runLayerSwitcherScenario(webContents) {
  await webContents.executeJavaScript('window.__settingsScreenTest.selectPage("permissions")');
  const permissions = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  await webContents.executeJavaScript('window.__settingsScreenTest.selectPage("diagnostics")');
  const diagnostics = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  // The pair above (编码/layered vs 高级/not-layered) cannot fail a
  // regression that reads `group === '编码'` instead of `page.layered` —
  // both pages agree on group AND layered. These three are the ones that
  // actually separate the two rules (see `nav.ts`'s own exception comments):
  // `mcp` is IN 编码 but not layered; `custom-providers` and `raw-json` are
  // OUTSIDE 编码 but layered.
  await webContents.executeJavaScript('window.__settingsScreenTest.selectPage("mcp")');
  const mcp = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  await webContents.executeJavaScript('window.__settingsScreenTest.selectPage("custom-providers")');
  const customProviders = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  await webContents.executeJavaScript('window.__settingsScreenTest.selectPage("raw-json")');
  const rawJson = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  return { permissions, diagnostics, mcp, customProviders, rawJson };
}

async function runProjectTabsScenario(webContents) {
  await webContents.executeJavaScript('window.__settingsScreenTest.selectPage("permissions")');
  const withoutProject = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  await webContents.executeJavaScript('window.__settingsScreenTest.setHasProject(true)');
  const withProject = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  return { withoutProject, withProject };
}

async function runPendingBannerScenario(webContents) {
  const noSnapshot = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  await webContents.executeJavaScript(
    'window.__settingsScreenTest.setSnapshot({ model: "opus", theme: "dark" }, { model: "sonnet", theme: "dark" })',
  );
  const pending = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  await webContents.executeJavaScript('window.__settingsScreenTest.setRunning(true)');
  const midTurn = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  await webContents.executeJavaScript('window.__settingsScreenTest.setRunning(false)');
  await webContents.executeJavaScript('window.__settingsScreenTest.clickRestart()');
  await waitFor(webContents, 'window.__settingsScreenTest.state().restartCalls === 1');
  const afterRestart = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  await webContents.executeJavaScript(
    'window.__settingsScreenTest.setSnapshot({ model: "opus" }, { model: "opus" })',
  );
  const resolved = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  return { noSnapshot, pending, midTurn, afterRestart, resolved };
}

async function runMalformedSnapshotScenario(webContents) {
  await webContents.executeJavaScript('window.__settingsScreenTest.setMalformedSnapshot()');
  const state = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  return { state };
}

async function runSessionLoadingGuardScenario(webContents) {
  // The initial mount (session already "ready") fires the fetch-on-ready
  // effect at least once — exactly how many is a React StrictMode artifact
  // (its dev-mode mount→cleanup→mount probe re-runs effects with no cleanup
  // function twice), not something this scenario cares about. Every
  // assertion below is relative to THIS baseline, not a hardcoded count.
  const initial = await webContents.executeJavaScript('window.__settingsScreenTest.state()');

  // Close and reopen while the session is loading: a naive `[activeSessionId]`
  // dependency would fire once here, find `command()` silently no-opping
  // (the host guard for a loading session), and never retry once loading
  // finishes — leaving the snapshot, and the pending banner, permanently null.
  await webContents.executeJavaScript('window.__settingsScreenTest.closeSettings()');
  await webContents.executeJavaScript('window.__settingsScreenTest.setSessionLoading(true)');
  await webContents.executeJavaScript('window.__settingsScreenTest.openSettings()');
  await waitFor(webContents, 'window.__settingsScreenTest.state().dialogPresent');
  const whileLoading = await webContents.executeJavaScript('window.__settingsScreenTest.state()');

  // Now the session finishes loading — the effect must retry on its own,
  // with no user action and no separate "refresh after restart" call needed.
  await webContents.executeJavaScript('window.__settingsScreenTest.setSessionLoading(false)');
  await waitFor(webContents, `window.__settingsScreenTest.state().refreshCalls > ${whileLoading.refreshCalls}`);
  const afterReady = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  return { initial, whileLoading, afterReady };
}

async function runRestartErrorScenario(webContents) {
  await webContents.executeJavaScript(
    'window.__settingsScreenTest.setSnapshot({ model: "opus" }, { model: "sonnet" })',
  );
  await webContents.executeJavaScript('window.__settingsScreenTest.setRestartShouldFail("cancel the active turn before changing engine settings")');
  await webContents.executeJavaScript('window.__settingsScreenTest.clickRestart()');
  await waitFor(webContents, 'window.__settingsScreenTest.state().hasRestartError');
  const afterFailure = await webContents.executeJavaScript('window.__settingsScreenTest.state()');

  // A later successful restart must clear the earlier error rather than
  // leaving a stale failure banner next to a click that just worked.
  await webContents.executeJavaScript('window.__settingsScreenTest.setRestartShouldFail(null)');
  await webContents.executeJavaScript('window.__settingsScreenTest.clickRestart()');
  await waitFor(webContents, '!window.__settingsScreenTest.state().hasRestartError');
  const afterSuccess = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  return { afterFailure, afterSuccess };
}

async function runPageContentScenario(webContents) {
  // Task 18 fix round 1, Important: the registration test in
  // `settings-coding-pages.test.ts` only asserted `nav.ts` says
  // `implemented: true` — true before this task's diff too, since `nav.ts`
  // was untouched. Nothing anywhere asserted `PAGE_CONTENT` (SettingsScreen.tsx),
  // which is what actually determines whether selecting one of these six
  // pages renders real content or the "not wired yet" placeholder. This
  // scenario selects each and reports `placeholderKind`, which is `null`
  // only when a real component is mounted.
  //
  // `voice` joined this list in Task 9 of the desktop-audio-capability
  // plan. Before that it was the one remaining `implemented: false` page —
  // a dedicated `placeholder` scenario used to select it and assert
  // `placeholderKind === 'not-implemented'` here. With `Voice.tsx` landed
  // there is no more honest `implemented: false` page left to demonstrate
  // that placeholder kind with, so that scenario was retired; `voice` now
  // gets the same positive "renders real content" check as every other
  // page in this list instead.
  const ids = ['permissions', 'tools-agent', 'skills', 'mcp', 'hooks', 'plugins', 'raw-json', 'voice'];
  const placeholderKinds = {};
  for (const id of ids) {
    await webContents.executeJavaScript(`window.__settingsScreenTest.selectPage(${JSON.stringify(id)})`);
    const state = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
    placeholderKinds[id] = state.placeholderKind;
  }

  // Task 18 fix round 1, Important: `hooksPageModel().escapeHatch` must be
  // what actually drives the "在 JSON 中编辑" button, not a hardcoded
  // `'raw-json'` literal at the call site that happens to agree with it.
  // Proven here by actually clicking the button and checking the shell
  // navigated to `raw-json` — now (Task 19 registered it) observable as
  // REAL content (`placeholderKind === null`), not the "not wired yet"
  // placeholder this assertion used to see — rather than by reading the
  // model's field in isolation.
  await webContents.executeJavaScript('window.__settingsScreenTest.selectPage("hooks")');
  await webContents.executeJavaScript('document.querySelector(\'[data-testid="hooks-open-raw-json"]\')?.click()');
  const afterHooksEscapeHatch = await webContents.executeJavaScript('window.__settingsScreenTest.state()');

  return { placeholderKinds, afterHooksEscapeHatch };
}

async function runLayerReseedScenario(webContents) {
  // Task 18 fix round 1, Critical: `ToolsAgent`'s `enabledTools`/`outputStyle`
  // text fields and `Plugins`' per-plugin `pluginConfigs` textarea are both
  // SEEDED from the editing layer's value via `useState`'s one-time
  // initializer, and `SettingsScreen.tsx` mounts a page with no `key` — so
  // switching layers used to leave stale text sitting in the field while the
  // card claimed to be editing a different layer. This reproduces that
  // exact sequence against the real rendered page (not just the pure
  // `*FromLayer` reader functions, which were already correct in isolation)
  // and asserts the field re-seeds.
  await webContents.executeJavaScript('window.__settingsScreenTest.setHasProject(true)');
  await webContents.executeJavaScript(
    "window.__settingsScreenTest.setLayeredSnapshot({ user: { enabledTools: ['Bash'] }, project: {} })",
  );
  await webContents.executeJavaScript('window.__settingsScreenTest.selectPage("tools-agent")');
  const toolsInitial = await webContents.executeJavaScript('window.__settingsScreenTest.getFieldValue(\'[aria-label="enabledTools"]\')');
  // Dirty the field with text that belongs to NEITHER layer, so a stale
  // read-back cannot be confused with either layer's real value.
  await webContents.executeJavaScript('window.__settingsScreenTest.setFieldValue(\'[aria-label="enabledTools"]\', "Dirty,Value")');
  const toolsDirty = await webContents.executeJavaScript('window.__settingsScreenTest.getFieldValue(\'[aria-label="enabledTools"]\')');
  await webContents.executeJavaScript('window.__settingsScreenTest.clickLayerTab("project")');
  const toolsAfterSwitch = await webContents.executeJavaScript('window.__settingsScreenTest.getFieldValue(\'[aria-label="enabledTools"]\')');

  // Same reproduction for `Plugins.tsx`'s `PluginConfigRow`, keyed by
  // PLUGIN id rather than by layer — the identical bug via a different
  // seam (React reuses the row's component instance across the layer
  // switch because the `id` key matches in both layers). Explicitly reset
  // to the `user` tab first — the tools-agent half above left the shell's
  // (page-independent) `editingLayer` state on `project`.
  await webContents.executeJavaScript('window.__settingsScreenTest.clickLayerTab("user")');
  await webContents.executeJavaScript(
    'window.__settingsScreenTest.setLayeredSnapshot({ user: { pluginConfigs: { "a@b": { from: "user" } } }, project: { pluginConfigs: { "a@b": { from: "project" } } } })',
  );
  await webContents.executeJavaScript('window.__settingsScreenTest.selectPage("plugins")');
  const configInitial = await webContents.executeJavaScript('window.__settingsScreenTest.getFieldValue(\'[aria-label="a@b 配置"]\')');
  await webContents.executeJavaScript('window.__settingsScreenTest.setFieldValue(\'[aria-label="a@b 配置"]\', "not even json")');
  const configDirty = await webContents.executeJavaScript('window.__settingsScreenTest.getFieldValue(\'[aria-label="a@b 配置"]\')');
  await webContents.executeJavaScript('window.__settingsScreenTest.clickLayerTab("project")');
  const configAfterSwitch = await webContents.executeJavaScript('window.__settingsScreenTest.getFieldValue(\'[aria-label="a@b 配置"]\')');

  // Task 18 fix round 2, Critical: the THIRD instance of the same defect
  // class — `PluginToggleRow`'s `useRef` remembers the last truthy
  // `enabledPlugins[id]` it saw and (before this round's fix) was never
  // reset on a layer switch. Priming the ref on a TRUTHY (config-object)
  // value in `user`, then switching to `project` where the same id is
  // `false`, then toggling it ON in `project` must write a fresh `true` —
  // NOT `user`'s remembered config object — into `project`. Unlike the
  // `ConcatDedup` case above this doesn't duplicate irrecoverably
  // (`enabledPlugins` is `DeepMerge`), so the only way to see the bug is to
  // inspect the VALUE actually written, via the `updateEngineSettings` mock.
  await webContents.executeJavaScript('window.__settingsScreenTest.clickLayerTab("user")');
  await webContents.executeJavaScript(
    'window.__settingsScreenTest.setLayeredSnapshot({ user: { enabledPlugins: { "a@b": { config: "A" } } }, project: { enabledPlugins: { "a@b": false } } })',
  );
  await webContents.executeJavaScript('window.__settingsScreenTest.selectPage("plugins")');
  await webContents.executeJavaScript('window.__settingsScreenTest.clickLayerTab("project")');
  await webContents.executeJavaScript(
    'document.querySelector(\'[data-testid="plugin-toggle-a@b"] button\').click()',
  );
  const afterToggleOnInProject = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  const enabledPluginsToggle = afterToggleOnInProject.lastEngineSettingsPatch;

  return {
    toolsInitial, toolsDirty, toolsAfterSwitch, configInitial, configDirty, configAfterSwitch,
    enabledPluginsToggle,
  };
}

async function runPermissionRuleDispatchScenario(webContents) {
  // Task 18 fix round 1, Important: `capturePermissionEdit` existed only
  // for the pure-function test to call — `Permissions.tsx`'s handlers built
  // their `update_permission_rules` call inline, so a regression that
  // routed a rule edit through `updateEngineSettings` instead would have
  // left that test green. This drives the REAL "add an allow rule" button
  // and asserts the mock `bridge.updatePermissionRules` received exactly
  // what `capturePermissionEdit` would have produced — proving the pin is
  // load-bearing, not merely self-referential.
  await webContents.executeJavaScript('window.__settingsScreenTest.selectPage("permissions")');
  await webContents.executeJavaScript(
    'window.__settingsScreenTest.setFieldValue(\'[aria-label="新增允许 (allow)规则"]\', "Bash(ls:*)")',
  );
  await webContents.executeJavaScript(
    'document.querySelector(\'[aria-label="新增允许 (allow)规则"]\').nextElementSibling.click()',
  );
  const state = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  return { lastPermissionRuleCall: state.lastPermissionRuleCall };
}

async function runRemountOnLayerSwitchScenario(webContents) {
  // Task 18 fix round 2: pins the STRUCTURAL fix itself
  // (`key={editingLayer}` on the page component in `SettingsScreen.tsx`)
  // rather than any one page's local-state symptom. Three different local-
  // state mechanisms (a `useState` seeded once, a `useRef` that survived a
  // switch, and whatever the next page invents) have now reproduced the
  // same cross-layer-fork defect; the fix that makes the CLASS
  // unrepresentable is remounting the page on a layer change, and this is
  // the test that fails if that `key` is ever removed, independent of
  // whether any individual page also happens to carry its own
  // `useEffect([editingLayer])` belt-and-suspenders fix.
  //
  // Detects a remount via DOM NODE IDENTITY (an expando property stashed
  // directly on the element), not via focus: clicking the layer-switcher
  // tab to CAUSE the switch would itself move `document.activeElement` to
  // the tab button regardless of whether the page remounted, so a
  // focus-based check cannot tell the two apart.
  await webContents.executeJavaScript('window.__settingsScreenTest.setHasProject(true)');
  await webContents.executeJavaScript('window.__settingsScreenTest.selectPage("tools-agent")');
  await webContents.executeJavaScript(
    'window.__settingsScreenTest.markElement(\'[aria-label="enabledTools"]\', "sentinel-before-switch")',
  );
  const markerBeforeSwitch = await webContents.executeJavaScript(
    'window.__settingsScreenTest.readElementMarker(\'[aria-label="enabledTools"]\')',
  );
  await webContents.executeJavaScript('window.__settingsScreenTest.clickLayerTab("project")');
  const markerAfterSwitch = await webContents.executeJavaScript(
    'window.__settingsScreenTest.readElementMarker(\'[aria-label="enabledTools"]\')',
  );

  // Negative control, in the SAME scenario run: an ordinary re-render that
  // changes PROPS but NEITHER the page NOR `editingLayer` (a fresh settings
  // snapshot arriving while the user stays put) must NOT be mistaken for a
  // remount by this same detection mechanism — otherwise this test would
  // trivially pass for the wrong reason (a marker that never survives ANY
  // re-render, remount or not). Deliberately NOT "navigate away and back":
  // `body`'s `<Component>` element has a DIFFERENT `Component` value
  // (`ToolsAgent` vs `Diagnostics`) at the same JSX position when the PAGE
  // changes, which React treats as a type change and unmounts regardless
  // of `key` — that would remount for a reason that has nothing to do with
  // the fix under test here, producing a false "remounted" result for the
  // wrong reason.
  await webContents.executeJavaScript(
    'window.__settingsScreenTest.markElement(\'[aria-label="enabledTools"]\', "sentinel-no-layer-change")',
  );
  await webContents.executeJavaScript(
    'window.__settingsScreenTest.setLayeredSnapshot({ project: { enabledTools: ["Read"] } })',
  );
  const markerAfterUnrelatedRerender = await webContents.executeJavaScript(
    'window.__settingsScreenTest.readElementMarker(\'[aria-label="enabledTools"]\')',
  );

  return { markerBeforeSwitch, markerAfterSwitch, markerAfterUnrelatedRerender };
}

async function runFocusTrapScenario(webContents) {
  // Start from a controlled mount: close the default-open dialog, focus the
  // element that will stand in for "whatever opened Settings", then reopen.
  await webContents.executeJavaScript('window.__settingsScreenTest.closeSettings()');
  await webContents.executeJavaScript('window.__settingsScreenTest.focusOpener()');
  await webContents.executeJavaScript('window.__settingsScreenTest.openSettings()');
  await waitFor(webContents, 'window.__settingsScreenTest.state().dialogPresent');
  const onMount = await webContents.executeJavaScript('window.__settingsScreenTest.state()');

  // Forward Tab from the LAST focusable element (the close button — nothing
  // in this shell renders after it) must wrap to the FIRST (the nav search
  // input), not escape the dialog into the (inert, in the real app) background.
  webContents.sendInputEvent({ type: 'keyDown', keyCode: 'Tab' });
  await delay(50);
  const afterForwardTab = await webContents.executeJavaScript('window.__settingsScreenTest.state()');

  // Shift+Tab from the FIRST focusable element must wrap back to the LAST.
  webContents.sendInputEvent({ type: 'keyDown', keyCode: 'Tab', modifiers: ['shift'] });
  await delay(50);
  const afterBackwardTab = await webContents.executeJavaScript('window.__settingsScreenTest.state()');

  // Closing restores focus to whatever had it before the dialog mounted —
  // the opener here, standing in for the gear icon / model picker button in
  // the real app.
  await webContents.executeJavaScript('window.__settingsScreenTest.closeSettings()');
  await delay(50);
  const afterClose = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  return { onMount, afterForwardTab, afterBackwardTab, afterClose };
}

async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 1000, height: 720, webPreferences: { sandbox: true } });
  try {
    await window.loadURL(url);
    const { webContents } = window;
    webContents.focus();
    await waitFor(webContents, 'Boolean(window.__settingsScreenTest && document.querySelector(\'[role="dialog"]\'))');
    const scenario = process.env.LINGXI_SETTINGS_SCREEN_SCENARIO ?? 'layer-switcher';
    const result = scenario === 'project-tabs' ? await runProjectTabsScenario(webContents)
      : scenario === 'pending-banner' ? await runPendingBannerScenario(webContents)
      : scenario === 'malformed-snapshot' ? await runMalformedSnapshotScenario(webContents)
      : scenario === 'session-loading-guard' ? await runSessionLoadingGuardScenario(webContents)
      : scenario === 'restart-error' ? await runRestartErrorScenario(webContents)
      : scenario === 'focus-trap' ? await runFocusTrapScenario(webContents)
      : scenario === 'page-content' ? await runPageContentScenario(webContents)
      : scenario === 'layer-reseed' ? await runLayerReseedScenario(webContents)
      : scenario === 'permission-rule-dispatch' ? await runPermissionRuleDispatchScenario(webContents)
      : scenario === 'remount-on-layer-switch' ? await runRemountOnLayerSwitchScenario(webContents)
      : await runLayerSwitcherScenario(webContents);
    process.stdout.write(`${JSON.stringify(result)}\n`);
  } finally {
    if (!window.isDestroyed()) window.destroy();
    if (app.isReady()) await app.quit();
  }
}

main().catch((error) => { process.stderr.write(`${error instanceof Error ? error.stack : String(error)}\n`); app.exit(1); });
