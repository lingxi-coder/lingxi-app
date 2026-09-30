import { app, BrowserWindow } from 'electron';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

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

async function runProviderRegionScenario(window, webContents) {
  const run = (expression) => webContents.executeJavaScript(expression);
  await run('window.__settingsScreenTest.setLayeredSnapshot({user:{},project:{},local:{}},{providerRegion:"international"})');
  await run('window.__settingsScreenTest.selectPage("custom-providers")');
  await waitFor(webContents, `Boolean(document.querySelector('[aria-label="模型使用区域"]'))`);
  const initial = await run('document.body.textContent.includes("当前运行：国际") && !document.body.textContent.includes("应用并重新连接")');
  await run(`(() => { const select=document.querySelector('[aria-label="模型使用区域"]'); select.value='china_mainland'; select.dispatchEvent(new Event('change',{bubbles:true})); })()`);
  await waitFor(webContents, 'window.__settingsScreenTest.state().lastEngineSettingsPatch?.patch.providerRegion === "china_mainland"');
  const saved = await run('window.__settingsScreenTest.state().lastEngineSettingsPatch');
  await run('window.__settingsScreenTest.setLayeredSnapshot({user:{providerRegion:"china_mainland"},project:{},local:{}},{providerRegion:"international"})');
  await waitFor(webContents, 'document.body.textContent.includes("应用并重新连接")');
  const pending = await run('document.body.textContent.includes("当前运行：国际")');
  const folder=process.env.LINGXI_SETTINGS_SCREENSHOT_DIR;
  if(folder){window.showInactive(); await delay(350); mkdirSync(folder,{recursive:true});writeFileSync(join(folder,'provider-region.png'),(await window.capturePage()).toPNG()); window.hide();}
  await run(`[...document.querySelectorAll('button')].find(b=>b.textContent==='应用并重新连接').click()`);
  await waitFor(webContents, 'window.__settingsScreenTest.state().navCalls.includes("restart")');
  return {initial,saved,pending,restarted:true};
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
  // `mcp` is IN 编码 but not layered; `custom-providers` is
  // OUTSIDE 编码 but layered.
  await webContents.executeJavaScript('window.__settingsScreenTest.selectPage("mcp")');
  const mcp = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  await webContents.executeJavaScript('window.__settingsScreenTest.selectPage("custom-providers")');
  const customProviders = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  return { permissions, diagnostics, mcp, customProviders };
}

async function runProjectTabsScenario(webContents) {
  await webContents.executeJavaScript('window.__settingsScreenTest.selectPage("permissions")');
  const withoutProject = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  await webContents.executeJavaScript('window.__settingsScreenTest.setHasProject(true)');
  const withProject = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  return { withoutProject, withProject };
}

/**
 * 层切换器旁的项目身份来自**引擎**回传的 `files_json`，而不是渲染端的
 * `bootstrap.workspace.path`。这个场景刻意让两者不一致：workspace 说
 * `/test/project`，快照里的 project 层却指向 `/test/engine-answer`。
 * 直接读 workspace 的实现会在这里报出 `project` 而不是 `engine-answer`。
 *
 * 第二半证明切换项目不是只喊了一嗓子：`activateProject` 单独用不换引擎
 * （`main/host.ts` 那一分支只写元数据），所以调用序列里必须还有一次
 * `open:`/`new:`。
 */
async function runLayerProjectScenario(webContents) {
  const call = (expression) => webContents.executeJavaScript(expression);
  await call('window.__settingsScreenTest.setHasProject(true)');
  await call('window.__settingsScreenTest.selectPage("permissions")');

  // 先停在项目层、且快照还没到 —— 「不知道是哪个项目」必须说出来，而不是
  // 悄悄回落到界面侧的当前项目。
  await call('window.__settingsScreenTest.clickLayerTab("project")');
  const beforeSnapshot = await call('window.__settingsScreenTest.state()');

  await call(`window.__settingsScreenTest.setSettingsFiles(${JSON.stringify([
    { layer: 'user', path: '/test/home/.lingxi/settings.json', exists: true, parsed: true },
    { layer: 'project', path: '/test/engine-answer/.lingxi/settings.json', exists: true, parsed: true },
    { layer: 'local', path: '/test/engine-answer/.lingxi/settings.local.json', exists: false, parsed: true },
  ])})`);

  const onProjectLayer = await call('window.__settingsScreenTest.state()');
  await call('window.__settingsScreenTest.clickLayerTab("user")');
  const onUserLayer = await call('window.__settingsScreenTest.state()');
  await call('window.__settingsScreenTest.clickLayerTab("project")');

  await call(`window.__settingsScreenTest.setProjectList(${JSON.stringify(['/test/engine-answer', '/test/other'])}, ${JSON.stringify({
    '/test/other': {
      sessions: [
        { uuid: 'older', title: 'older', modified_rfc3339: '2026-01-01T00:00:00Z', message_count: 1 },
        { uuid: 'newest', title: 'newest', modified_rfc3339: '2026-06-01T00:00:00Z', message_count: 2 },
      ],
    },
  })})`);
  await call('window.__settingsScreenTest.clickTestId("layer-project-switch")');
  const pickerOpen = await call('window.__settingsScreenTest.state()');

  await call('window.__settingsScreenTest.clickProjectOption("/test/other")');
  await new Promise((resolve) => setTimeout(resolve, 250));
  const afterSwitch = await call('window.__settingsScreenTest.state()');

  return { beforeSnapshot, onUserLayer, onProjectLayer, pickerOpen, afterSwitch };
}

async function runNoEngineBannerScenario(webContents) {
  await webContents.executeJavaScript(
    'window.__settingsScreenTest.setSnapshot({ model: "opus", theme: "dark" }, { model: "sonnet", theme: "dark" })',
  );
  return webContents.executeJavaScript('window.__settingsScreenTest.state()');
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
  // finishes — leaving the snapshot permanently null.
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
  const ids = ['permissions', 'tools-agent', 'skills', 'mcp', 'hooks', 'plugins', 'diagnostics', 'voice'];
  const placeholderKinds = {};
  for (const id of ids) {
    await webContents.executeJavaScript(`window.__settingsScreenTest.selectPage(${JSON.stringify(id)})`);
    const state = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
    placeholderKinds[id] = state.placeholderKind;
  }

  // The Hooks shortcut must reach the diagnostics configuration files.
  await webContents.executeJavaScript('window.__settingsScreenTest.selectPage("hooks")');
  await webContents.executeJavaScript('document.querySelector(\'[data-testid="hooks-open-settings-files"]\')?.click()');
  const afterHooksEscapeHatch = await webContents.executeJavaScript('window.__settingsScreenTest.state()');

  const hasFiles = await webContents.executeJavaScript('document.body.textContent.includes("配置文件")');
  await webContents.executeJavaScript(`window.__settingsScreenTest.setSettingsFiles([
    { layer: 'user', path: '/test/home/.lingxi/settings.json', exists: true, parsed: true },
    { layer: 'project', path: '/test/project/.lingxi/settings.json', exists: false, parsed: true },
    { layer: 'local', path: '/test/project/.lingxi/settings.local.json', exists: true, parsed: false, parse_error: 'Invalid JSON' }
  ])`);
  await delay(50);
  const fileActions = await webContents.executeJavaScript(`(async () => {
    const opened = [];
    window.lingxi = { openSettingsFile: async (path) => { opened.push(path); } };
    const user = document.querySelector('[data-testid="settings-file-open-user"]');
    const project = document.querySelector('[data-testid="settings-file-open-project"]');
    const local = document.querySelector('[data-testid="settings-file-open-local"]');
    user.click(); local.click();
    await new Promise(resolve => setTimeout(resolve, 20));
    const brokenShown = document.body.textContent.includes('解析失败：Invalid JSON');
    window.lingxi.openSettingsFile = async () => { throw new Error('test open failure'); };
    user.click();
    await new Promise(resolve => setTimeout(resolve, 20));
    return { opened, missingDisabled: project.disabled, brokenShown,
      error: document.querySelector('[data-testid="settings-file-error"]')?.textContent,
      rawEditor: Boolean(document.querySelector('[data-testid="raw-json-textarea"]')) };
  })()`);
  const screenshotDir = process.env.LINGXI_SETTINGS_SCREENSHOT_DIR;
  if (screenshotDir) {
    mkdirSync(screenshotDir, { recursive: true });
    writeFileSync(join(screenshotDir, 'diagnostics-files.png'), (await webContents.capturePage()).toPNG());
  }
  return { placeholderKinds, afterHooksEscapeHatch, hasFiles, fileActions };
}

async function runWindowChromeScenario(webContents) {
  return webContents.executeJavaScript(`(() => {
    const dragRegion = document.querySelector('.settings-window-drag-region');
    const contentScroll = document.querySelector('.settings-content-scroll');
    const dragStyle = dragRegion && getComputedStyle(dragRegion);
    const scrollStyle = contentScroll && getComputedStyle(contentScroll);
    return {
      hasDragRegion: Boolean(dragRegion),
      dragRegionMode: dragStyle?.webkitAppRegion ?? null,
      dragRegionHeight: dragStyle?.height ?? null,
      scrollPaddingTop: scrollStyle?.paddingTop ?? null,
      scrollPaddingBottom: scrollStyle?.paddingBottom ?? null,
    };
  })()`);
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
  await webContents.executeJavaScript(`(() => {
    const row = document.querySelector('.configuration-entry .extension-hub-row-open');
    if (!row) throw new Error('missing installed plugin configuration row');
    row.click();
  })()`);
  await waitFor(webContents, 'Boolean(document.querySelector(".extension-detail-dialog"))');
  const configInitial = await webContents.executeJavaScript('window.__settingsScreenTest.getFieldValue(\'[aria-label="a@b 配置"]\')');
  await webContents.executeJavaScript('window.__settingsScreenTest.setFieldValue(\'[aria-label="a@b 配置"]\', "not even json")');
  const configDirty = await webContents.executeJavaScript('window.__settingsScreenTest.getFieldValue(\'[aria-label="a@b 配置"]\')');
  await webContents.executeJavaScript('window.__settingsScreenTest.clickLayerTab("project")');
  await waitFor(webContents, '!document.querySelector(".extension-detail-dialog") && Boolean(document.querySelector(".configuration-entry .extension-hub-row-open"))');
  await webContents.executeJavaScript(`document.querySelector('.configuration-entry .extension-hub-row-open').click()`);
  await waitFor(webContents, 'Boolean(document.querySelector(\'[aria-label="a@b 配置"]\'))');
  const configAfterSwitch = await webContents.executeJavaScript('window.__settingsScreenTest.getFieldValue(\'[aria-label="a@b 配置"]\')');

  return {
    toolsInitial, toolsDirty, toolsAfterSwitch, configInitial, configDirty, configAfterSwitch,
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

async function runBackClickScenario(webContents) {
  // Model the sidebar brand's drag region, which remains beneath settings.
  const regions = await webContents.executeJavaScript(`(() => {
    const drag = document.createElement('div');
    drag.className = 'drag-region';
    Object.assign(drag.style, { position: 'absolute', top: '38px', left: '0', width: '240px', height: '48px' });
    document.body.prepend(drag);
    const panel = document.querySelector('[role="dialog"]');
    const button = document.querySelector('[aria-label="Back to app"]');
    const rect = button.getBoundingClientRect();
    return {
      panelRegion: getComputedStyle(panel).getPropertyValue('-webkit-app-region'),
      backgroundRegion: getComputedStyle(drag).getPropertyValue('-webkit-app-region'),
      x: Math.round(rect.x + rect.width / 2),
      y: Math.round(rect.y + rect.height / 2),
    };
  })()`);
  // Input injection alone bypasses OS non-client hit testing; the region
  // assertion in the test also guards the native Electron drag exclusion.
  webContents.sendInputEvent({ type: 'mouseDown', x: regions.x, y: regions.y, button: 'left', clickCount: 1 });
  webContents.sendInputEvent({ type: 'mouseUp', x: regions.x, y: regions.y, button: 'left', clickCount: 1 });
  await delay(50);
  return { ...regions, afterClose: await webContents.executeJavaScript('window.__settingsScreenTest.state()') };
}

async function runFocusTrapScenario(webContents) {
  // Start from a controlled mount: close the default-open dialog, focus the
  // element that will stand in for "whatever opened Settings", then reopen.
  await webContents.executeJavaScript('window.__settingsScreenTest.closeSettings()');
  await webContents.executeJavaScript('window.__settingsScreenTest.focusOpener()');
  await webContents.executeJavaScript('window.__settingsScreenTest.openSettings()');
  await waitFor(webContents, 'window.__settingsScreenTest.state().dialogPresent');
  const onMount = await webContents.executeJavaScript('window.__settingsScreenTest.state()');

  // Back is now first in the left panel; Tab moves to search.
  webContents.sendInputEvent({ type: 'keyDown', keyCode: 'Tab' });
  await delay(50);
  const afterForwardTab = await webContents.executeJavaScript('window.__settingsScreenTest.state()');

  // Shift+Tab from search returns to Back.
  webContents.sendInputEvent({ type: 'keyDown', keyCode: 'Tab', modifiers: ['shift'] });
  await delay(50);
  const afterBackwardTab = await webContents.executeJavaScript('window.__settingsScreenTest.state()');

  // Closing restores focus to whatever had it before the dialog mounted —
  // the opener here, standing in for the gear icon / model picker button in
  // the real app.
  await webContents.executeJavaScript(`document.querySelector('[aria-label="Back to app"]').click()`);
  await delay(50);
  const afterClose = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  return { onMount, afterForwardTab, afterBackwardTab, afterClose };
}

async function runVisualAdminScenario(window, webContents) {
  await webContents.executeJavaScript('window.__settingsScreenTest.setHasProject(true)');
  await webContents.executeJavaScript(`window.__settingsScreenTest.setLayeredSnapshot({
    user: {
      enabledPlugins: { "secure@acme": true },
      pluginConfigs: { "secure@acme": { options: { REGION: "us-west" }, mcpServers: {} } },
      extraKnownMarketplaces: { acme: { source: "acme/plugins" } },
      hooks: {
        PreToolUse: [{ matcher: "Bash", hooks: [{ type: "command", command: "./scripts/check.sh", timeout: 30, statusMessage: "Checking command" }] }],
        Stop: [{ hooks: [{ type: "prompt", prompt: "Review the final response", model: "haiku" }] }]
      }
    },
    project: {},
    local: {}
  })`);
  await delay(100);
  const pages = ['skills', 'mcp', 'plugins', 'hooks'];
  const layout = {};
  const screenshots = {};
  const screenshotDir = process.env.LINGXI_SETTINGS_SCREENSHOT_DIR;
  if (screenshotDir) mkdirSync(screenshotDir, { recursive: true });
  const theme = process.env.LINGXI_SETTINGS_SCREEN_THEME === 'dark' ? 'dark' : 'light';
  for (const page of pages) {
    await webContents.executeJavaScript(`window.__settingsScreenTest.selectPage(${JSON.stringify(page)})`);
    await delay(100);
    if (page === 'mcp') {
      await webContents.executeJavaScript(`(() => {
        const row = document.querySelector('.configuration-entry .extension-hub-row-open');
        if (!row) throw new Error('missing MCP configuration row');
        row.click();
      })()`);
      await delay(50);
    }
    if (page === 'plugins') {
      await webContents.executeJavaScript(`(() => {
        const row = document.querySelector('.configuration-entry .extension-hub-row-open');
        if (!row) throw new Error('missing installed plugin configuration row');
        row.click();
      })()`);
      await waitFor(webContents, 'Boolean(document.querySelector("[aria-label=\\\"REGION option\\\"]"))');
    }
    if (page === 'hooks') {
      for (const label of ['PreToolUse', 'Group 1', 'Handler 1']) {
        await webContents.executeJavaScript(`(() => {
          const button = [...document.querySelectorAll('button')].find((candidate) => candidate.textContent?.includes(${JSON.stringify(label)}));
          button?.click();
        })()`);
        await delay(50);
      }
    }
    layout[page] = await webContents.executeJavaScript(`(() => {
      const dialog = document.querySelector('.extension-detail-dialog')
        ?? document.querySelector('.configuration-detail')
        ?? document.querySelector('[role="dialog"]');
      const state = window.__settingsScreenTest.state();
      return {
        ...state,
        horizontalOverflow: dialog ? dialog.scrollWidth > dialog.clientWidth + 1 : true,
        structuredHookEditor: Boolean(document.querySelector('[aria-label="hook type"]')),
        advancedHookJson: Boolean(document.querySelector('[aria-label="hooks-json"]')),
        structuredMcpEditor: Boolean(document.querySelector('[aria-label="MCP transport type"]')),
        manifestPluginEditor: Boolean(document.querySelector('[aria-label="REGION option"]')),
      };
    })()`);
    if (screenshotDir) {
      const path = join(screenshotDir, `${theme}-${page}.png`);
      writeFileSync(path, (await window.capturePage()).toPNG());
      screenshots[page] = path;
    }
  }
  return { pages, layout, screenshots };
}

async function runCustomProvidersScenario(window, webContents) {
  const run = (code) => webContents.executeJavaScript(code);
  const click = async (label) => {
    await run(`(() => { const b = [...document.querySelectorAll('button')].find(e => e.textContent.trim() === ${JSON.stringify(label)}); if (!b) throw new Error('missing button: ' + ${JSON.stringify(label)}); b.click(); })()`);
    await delay(35);
  };
  const fill = async (label, value) => {
    await run(`window.__settingsScreenTest.setFieldValue(${JSON.stringify(`[aria-label="${label}"]`)}, ${JSON.stringify(value)})`);
    await delay(35);
  };
  const capture = async (name) => {
    const folder = process.env.LINGXI_SETTINGS_SCREENSHOT_DIR;
    if (!folder) return;
    mkdirSync(folder, { recursive: true });
    const theme = process.env.LINGXI_SETTINGS_SCREEN_THEME === 'dark' ? 'dark' : 'light';
    writeFileSync(join(folder, `${theme}-custom-${name}.png`), (await window.capturePage()).toPNG());
  };
  await run('window.__settingsScreenTest.setHasProject(true)');
  await run('window.__settingsScreenTest.setLayeredSnapshot({ user: {}, project: {}, local: {} })');
  await run('window.__settingsScreenTest.selectPage("custom-providers")');
  await delay(70);
  await capture('empty');
  await click('＋ 新增 Provider');
  await fill('Profile 名称', 'customlab');
  await fill('baseUrl', 'https://example.com/v1');
  await fill('API Key', 'test-only-secret-1234');
  await fill('模型 ID 1', 'model-a');
  await capture('editor');
  await run('window.__settingsScreenTest.holdSettingsWrite()');
  await click('保存 Provider');
  const locked = await run('window.__settingsScreenTest.state()');
  await run('window.__settingsScreenTest.clickLayerTab("project")');
  await run('window.__settingsScreenTest.releaseSettingsWrite()');
  await waitFor(webContents, 'window.__settingsScreenTest.state().credentialWrites.length === 1');
  const saved = await run('window.__settingsScreenTest.state()');

  // Edit an existing entry with fields the simple editor does not expose.
  await run(`window.__settingsScreenTest.setLayeredSnapshot({ user: { providers: { customlab: {
    type: 'openai', baseUrl: 'https://example.com/v1', supportsWebsockets: false,
    models: [{ id: 'model-a', aliases: ['fast'], capabilities: { reasoning: true }, metadata: { display_name: 'Model A' } }, { id: 'remove-me' }],
    pricing: { 'model-a': { inputPerMtok: 1, outputPerMtok: 2 }, 'remove-me': { inputPerMtok: 3, outputPerMtok: 4 } }
  } } }, project: { providers: { other: {type:'openai',baseUrl:'https://other.example',apiKeyEnv:'OTHER_KEY',models:[{id:'other'}]} } }, local:{} })`);
  await delay(50);
  await click('编辑');
  const fixedId = await run('document.querySelector(\'[aria-label="Profile 名称"]\').readOnly');
  await fill('baseUrl', 'https://changed.example/v1');
  await fill('模型 ID 1', 'model-renamed');
  await run('document.querySelector(\'[aria-label="移除模型 2"]\').click()');
  await delay(35);
  await click('保存 Provider');
  await waitFor(webContents, 'window.__settingsScreenTest.state().lastEngineSettingsPatch.patch.providers.customlab.baseUrl === "https://changed.example/v1"');
  const edited = await run('window.__settingsScreenTest.state()');
  await delay(50);
  await capture('list');

  await click('导入 JSON');
  await fill('JSON 配置', '{"apiKey":"test-only-secret-1234",broken');
  await click('解析并预览');
  const invalidText = await run('document.querySelector(\'[role="alert"]\')?.textContent');
  await capture('invalid-json');
  const importedJson = JSON.stringify({ provider: {
    customlab: { npm: '@ai-sdk/openai-compatible', options: { baseURL: 'https://conflict.example', apiKey: '{env:EXISTING_KEY}' }, models: { replacement: {} } },
    imported: { npm: '@ai-sdk/anthropic', options: { baseURL: 'https://api.example.com', apiKey: 'test-only-import-secret' }, models: { claude: {} } },
  } });
  await run(`(() => { const files = new DataTransfer(); files.items.add(new File([${JSON.stringify(importedJson)}], 'providers.json', {type:'application/json'})); const input = document.querySelector('input[type=file]'); input.files = files.files; input.dispatchEvent(new Event('change', {bubbles:true})); })()`);
  await waitFor(webContents, 'document.querySelector(\'[aria-label="JSON 配置"]\')?.value.includes("test-only-import-secret")');
  await click('解析并预览');
  const conflictSkipped = await run('document.querySelector(\'input[type="checkbox"]\').checked === false');
  await capture('import-preview');
  await run('window.__settingsScreenTest.failNextCredential()');
  await click('确认导入 1 个 Provider');
  await waitFor(webContents, 'document.body.textContent.includes("配置已保存，凭据保存失败")');
  const partial = await run('window.__settingsScreenTest.state()');
  await capture('credential-retry');
  await click('重试保存凭据');
  await waitFor(webContents, 'window.__settingsScreenTest.state().credentialWrites.includes("imported")');
  const retried = await run('window.__settingsScreenTest.state()');

  await click('导入 JSON');
  await fill('JSON 配置', '{"provider":{}}');
  await run('window.__settingsScreenTest.clickLayerTab("project")');
  await delay(50);
  const clearedOnSwitch = await run('!document.querySelector(\'[aria-label="JSON 配置"]\')');
  await click('编辑');
  window.setSize(760, 720);
  await delay(70);
  const overflow = await run('(() => { const d = document.querySelector(\'[role="dialog"]\'); return d.scrollWidth > d.clientWidth + 1; })()');
  await capture('narrow');
  await run('document.querySelector("details > summary").click()');
  await fill('maxAttempts', '1.5');
  await click('保存');
  const retryError = await run('document.querySelector(\'[role="alert"]\')?.textContent');
  await run('window.__settingsScreenTest.clickLayerTab("user")');
  await delay(35);
  await run('window.__settingsScreenTest.setLayeredSnapshot({ user:{ providers:{ broken:null } }, project:{}, local:{} })');
  await delay(35);
  const malformedRowSurvives = await run('document.body.textContent.includes("broken") && Boolean(document.querySelector(\'[role="dialog"]\'))');
  await click('导入 JSON');
  await fill('JSON 配置', JSON.stringify({ providers: { dual: { type: 'openai', baseUrl: 'https://example.com', apiKeyEnv: 'FALLBACK_KEY', apiKey: 'test-only-explicit-key', models: [{ id: 'm' }] } } }));
  await click('解析并预览');
  const dualAuth = await run('({ mode:document.querySelector(\'[aria-label="认证方式"]\').value, keyPresent:Boolean(document.querySelector(\'[aria-label="API Key"]\')) })');
  await click('← 返回 Provider 列表');
  await run('window.__settingsScreenTest.setLayeredSnapshot({user:{providers:{Legacy:{type:"openai",baseUrl:"https://legacy.example",apiKeyEnv:"LEGACY_KEY",models:["legacy-model"]}}},project:{},local:{}})');
  await delay(35);
  await click('编辑');
  await fill('模型 ID 1', ' legacy-model ');
  await fill('baseUrl', ' https://legacy.example/v1 ');
  await click('保存 Provider');
  await waitFor(webContents, 'window.__settingsScreenTest.state().lastEngineSettingsPatch.patch.providers?.Legacy?.baseUrl === "https://legacy.example/v1"');
  const legacy = await run('window.__settingsScreenTest.state().lastEngineSettingsPatch.patch.providers.Legacy');
  await click('导入 JSON');
  await fill('JSON 配置', JSON.stringify({ providers: { priced: {
    type: 'openai', baseUrl: 'https://example.com', apiKeyEnv: 'PRICED_KEY',
    models: [{ id: 'old' }, { id: 'other' }],
    pricing: { old: { inputPerMtok: 1, outputPerMtok: 2 }, other: { inputPerMtok: 3, outputPerMtok: 4 } },
  } } }));
  await click('解析并预览');
  await fill('模型 ID 1', 'other');
  const pricingTargetProtected = await run('document.querySelector(\'[aria-label="移除模型 2"]\').disabled && !document.querySelector(\'[aria-label="移除模型 1"]\').disabled');
  await run('document.querySelector(\'input[type="checkbox"]\').click()');
  await delay(35);
  await run('document.querySelector(\'input[type="checkbox"]\').click()');
  await delay(35);
  await fill('模型 ID 1', 'final-model');
  await click('确认导入 1 个 Provider');
  await waitFor(webContents, 'window.__settingsScreenTest.state().lastEngineSettingsPatch.patch.providers?.priced');
  const toggledPricing = await run('window.__settingsScreenTest.state().lastEngineSettingsPatch.patch.providers.priced.pricing');
  return { pricingTargetProtected, toggledPricing, legacy, malformedRowSurvives, dualAuth, retryError, locked, saved, fixedId, edited, invalidText, conflictSkipped, partial, retried, clearedOnSwitch, overflow };
}

async function runProviderSessionIsolation(webContents) {
  const run = (code) => webContents.executeJavaScript(code);
  const click = async (label) => {
    await run(`(() => { const b = [...document.querySelectorAll('button')].find(e => e.textContent.trim() === ${JSON.stringify(label)}); if (!b) throw new Error('missing button'); b.click(); })()`);
    await delay(35);
  };
  const fill = async (label, value) => {
    await run(`window.__settingsScreenTest.setFieldValue(${JSON.stringify(`[aria-label="${label}"]`)}, ${JSON.stringify(value)})`);
    await delay(35);
  };
  await run('window.__settingsScreenTest.setLayeredSnapshot({user:{},project:{},local:{}})');
  await run('window.__settingsScreenTest.selectPage("custom-providers")');
  await click('＋ 新增 Provider');
  await fill('Profile 名称', 'old-project-draft');
  await run('window.__settingsScreenTest.setActiveSession("session-b")');
  await delay(50);
  const draftCleared = await run('!document.querySelector(\'[aria-label="Profile 名称"]\')');
  if (!draftCleared) return { draftCleared };
  await click('＋ 新增 Provider');
  await fill('Profile 名称', 'pending-key');
  await fill('baseUrl', 'https://example.com');
  await fill('API Key', 'test-only-key');
  await fill('模型 ID 1', 'model');
  await run('window.__settingsScreenTest.holdSettingsWrite()');
  await click('保存 Provider');
  await run('window.__settingsScreenTest.setActiveSession("session-c")');
  await delay(50);
  await run('window.__settingsScreenTest.releaseSettingsWrite()');
  await delay(80);
  return { draftCleared, state: await run('window.__settingsScreenTest.state()') };
}

async function runConfigurationNavigation(window, webContents) {
  const run = (code) => webContents.executeJavaScript(code);
  const click = async (selector) => {
    await run(`(() => { const element = document.querySelector(${JSON.stringify(selector)}); if (!element) throw new Error('missing selector: ' + ${JSON.stringify(selector)}); element.click(); })()`);
    await delay(60);
  };
  const button = async (label) => {
    await run(`(() => { const element = Array.from(document.querySelectorAll('button')).find(b => b.textContent.trim() === ${JSON.stringify(label)}); if (!element) throw new Error('missing button: ' + ${JSON.stringify(label)}); element.click(); })()`);
    await delay(60);
  };
  const capture = async (name) => {
    const dir = process.env.LINGXI_SETTINGS_SCREENSHOT_DIR;
    if (!dir) return;
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, `${process.env.LINGXI_SETTINGS_SCREEN_THEME || 'light'}-${name}.png`), (await window.capturePage()).toPNG());
  };
  await run('window.__settingsScreenTest.setLayeredSnapshot({user:{},project:{},local:{}})');
  const results = {};
  for (const page of ['skills', 'mcp']) {
    await run(`window.__settingsScreenTest.selectPage(${JSON.stringify(page)})`);
    await delay(80);
    const initialList = page === 'skills'
      ? await run(`Boolean(document.querySelector('.extension-hub-page')) && !document.querySelector('.extension-detail-dialog')`)
      : await run(`Boolean(document.querySelector('.configuration-list')) && !document.querySelector('.configuration-detail')`);
    const searchLabel = page === 'skills' ? '搜索 skills' : '搜索 MCP 服务器';
    const query = page === 'skills' ? 'release-notes' : 'context7';
    await run(`window.__settingsScreenTest.setFieldValue(${JSON.stringify(`[aria-label="${searchLabel}"]`)}, ${JSON.stringify(query)})`);
    await delay(40);
    await capture(`${page}-list`);
    await click(page === 'skills' ? '.extension-hub-row-open' : '.configuration-entry .extension-hub-row-open');
    const detailOnly = page === 'skills'
      ? await run(`Boolean(document.querySelector('.extension-detail-dialog'))`)
      : await run(`Boolean(document.querySelector('.configuration-detail')) && !document.querySelector('.configuration-list')`);
    await capture(`${page}-detail`);
    await click(page === 'skills' ? '[aria-label="Close details"]' : '.configuration-back');
    const searchRetained = await run(`document.querySelector(${JSON.stringify(`[aria-label="${searchLabel}"]`)}).value === ${JSON.stringify(query)}`);
    await button('Add');
    await capture(`${page}-create`);
    const nameLabel = page === 'skills' ? 'skill-create-name' : 'mcp-name';
    const createOpened = page === 'skills'
      ? await run(`Boolean(document.querySelector(${JSON.stringify(`[aria-label="${nameLabel}"]`)})) && Boolean(document.querySelector('.extension-detail-dialog'))`)
      : await run(`Boolean(document.querySelector(${JSON.stringify(`[aria-label="${nameLabel}"]`)})) && !document.querySelector('.configuration-list')`);
    await run(`window.__settingsScreenTest.setFieldValue(${JSON.stringify(`[aria-label="${nameLabel}"]`)}, 'draft-test')`);
    await delay(40);
    await click(page === 'skills' ? '[aria-label="Close details"]' : '.configuration-back');
    const guarded = page === 'skills'
      ? await run(`Boolean(document.querySelector('.extension-detail-dialog')) && document.body.textContent.includes('丢弃并切换')`)
      : await run(`Boolean(document.querySelector('.configuration-detail')) && document.body.textContent.includes('丢弃并切换')`);
    await button('继续编辑');
    const draftRetained = await run(`document.querySelector(${JSON.stringify(`[aria-label="${nameLabel}"]`)}).value === 'draft-test'`);
    await click(page === 'skills' ? '[aria-label="Close details"]' : '.configuration-back');
    await button('丢弃并切换');
    const returned = page === 'skills'
      ? await run(`Boolean(document.querySelector('.extension-hub-page')) && !document.querySelector('.extension-detail-dialog')`)
      : await run(`Boolean(document.querySelector('.configuration-list')) && !document.querySelector('.configuration-detail')`);
    results[page] = { initialList, detailOnly, searchRetained, createOpened, guarded, draftRetained, returned };
  }
  await run('window.__settingsScreenTest.selectPage("hooks")');
  await delay(80);
  const hooksList = await run(`Boolean(document.querySelector('.configuration-list')) && !document.querySelector('.hooks-editor')`);
  await capture('hooks-list');
  await button('添加');
  const hooksDetail = await run(`Boolean(document.querySelector('[aria-label="hook command"]')) && !document.querySelector('.configuration-list')`);
  await run(`window.__settingsScreenTest.setFieldValue('[aria-label="hook command"]', 'echo hook-test')`);
  await delay(50);
  await capture('hooks-create');
  await click('.configuration-back');
  await click('.configuration-entry');
  const hooksGuarded = await run(`document.body.textContent.includes('丢弃并切换')`);
  await button('继续编辑');
  const hooksDraftRetained = await run(`document.querySelector('[aria-label="hook command"]').value === 'echo hook-test'`);
  results.hooks = { initialList: hooksList, detailOnly: hooksDetail, guarded: hooksGuarded, draftRetained: hooksDraftRetained };
  return results;
}

async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 1000, height: 720, webPreferences: { sandbox: true } });
  try {
    await window.loadURL(url);
    const { webContents } = window;
    webContents.focus();
    await waitFor(webContents, 'Boolean(window.__settingsScreenTest?.state().settingsSnapshotReady && document.querySelector(\'[role="dialog"]\'))');
    const scenario = process.env.LINGXI_SETTINGS_SCREEN_SCENARIO ?? 'layer-switcher';
    const result = scenario === 'provider-region' ? await runProviderRegionScenario(window, webContents)
      : scenario === 'configuration-navigation' ? await runConfigurationNavigation(window, webContents)
      : scenario === 'provider-session-isolation' ? await runProviderSessionIsolation(webContents)
      : scenario === 'custom-providers' ? await runCustomProvidersScenario(window, webContents)
      : scenario === 'visual-admin' ? await runVisualAdminScenario(window, webContents)
      : scenario === 'project-tabs' ? await runProjectTabsScenario(webContents)
      : scenario === 'layer-project' ? await runLayerProjectScenario(webContents)
      : scenario === 'no-engine-banner' ? await runNoEngineBannerScenario(webContents)
      : scenario === 'malformed-snapshot' ? await runMalformedSnapshotScenario(webContents)
      : scenario === 'session-loading-guard' ? await runSessionLoadingGuardScenario(webContents)
      : scenario === 'focus-trap' ? await runFocusTrapScenario(webContents)
      : scenario === 'back-click' ? await runBackClickScenario(webContents)
      : scenario === 'page-content' ? await runPageContentScenario(webContents)
      : scenario === 'window-chrome' ? await runWindowChromeScenario(webContents)
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
