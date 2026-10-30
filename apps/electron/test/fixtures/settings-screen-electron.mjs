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
  await waitFor(webContents, `Boolean(document.querySelector('[aria-label="模型区域"]'))`);
  const initial = await run('document.body.textContent.includes("当前运行：国际") && !document.body.textContent.includes("应用并重连")');
  await run(`(() => { const select=document.querySelector('[aria-label="模型区域"]'); select.value='china_mainland'; select.dispatchEvent(new Event('change',{bubbles:true})); })()`);
  await waitFor(webContents, 'window.__settingsScreenTest.state().lastEngineSettingsPatch?.patch.providerRegion === "china_mainland"');
  const saved = await run('window.__settingsScreenTest.state().lastEngineSettingsPatch');
  await run('window.__settingsScreenTest.setLayeredSnapshot({user:{providerRegion:"china_mainland"},project:{},local:{}},{providerRegion:"international"})');
  await waitFor(webContents, 'document.body.textContent.includes("应用并重连")');
  const pending = await run('document.body.textContent.includes("当前运行：国际")');
  const folder=process.env.LINGXI_SETTINGS_SCREENSHOT_DIR;
  if(folder){window.showInactive(); await delay(350); mkdirSync(folder,{recursive:true});writeFileSync(join(folder,'provider-region.png'),(await window.capturePage()).toPNG()); window.hide();}
  await run(`[...document.querySelectorAll('button')].find(b=>b.textContent==='应用并重连').click()`);
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
  await waitFor(webContents, 'Boolean(document.querySelector("[data-testid=settings-snapshot-error]"))');
  const state = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  const errorTop = await webContents.executeJavaScript('document.querySelector("[data-testid=settings-snapshot-error]").getBoundingClientRect().top');
  return { state, errorTop };
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

async function runWindowChromeScenario(window, webContents) {
  const run = (expression) => webContents.executeJavaScript(expression);
  const theme = new URL(url).searchParams.get('theme') ?? 'light';
  const capture = async (position) => {
    const screenshotDir = process.env.LINGXI_SETTINGS_SCREENSHOT_DIR;
    if (!screenshotDir) return;
    window.showInactive();
    await delay(120);
    mkdirSync(screenshotDir, { recursive: true });
    writeFileSync(join(screenshotDir, `settings-voice-${theme}-${position}.png`), (await window.capturePage()).toPNG());
    window.hide();
  };
  const measure = () => run(`(() => {
    const dialog = document.querySelector('[role="dialog"]');
    const contentScroll = document.querySelector('.settings-content-scroll');
    const inner = document.querySelector('.settings-content-inner');
    const sidebarScroll = document.querySelector('.settings-nav-scroll');
    const sidebar = sidebarScroll.parentElement;
    const rect = (element) => element.getBoundingClientRect();
    const dialogRect = rect(dialog);
    const scrollRect = rect(contentScroll);
    const innerRect = rect(inner);
    const headerRect = rect(inner.firstElementChild);
    const lastChild = inner.lastElementChild;
    const sidebarRect = rect(sidebar);
    const sidebarScrollRect = rect(sidebarScroll);
    const sidebarStyle = getComputedStyle(sidebar);
    const style = getComputedStyle(contentScroll);
    // Padding belongs to the shared content wrapper. Nested details/card
    // margins can collapse, so their rectangle difference is clearance,
    // not a reliable way to infer the wrapper's CSS padding.
    const pixels = (value) => Math.round(value * 100) / 100;
    return {
      viewportInsets: {
        top: pixels(scrollRect.top - dialogRect.top),
        right: pixels(dialogRect.right - scrollRect.right),
        bottom: pixels(dialogRect.bottom - scrollRect.bottom),
      },
      contentInsets: {
        left: pixels(headerRect.left - innerRect.left),
        right: pixels(innerRect.right - headerRect.right),
        bottom: parseFloat(getComputedStyle(inner).paddingBottom),
      },
      contentBottomClearance: pixels(innerRect.bottom - rect(lastChild).bottom),
      sidebarInsets: {
        left: pixels(sidebarScrollRect.left - sidebarRect.left - parseFloat(sidebarStyle.borderLeftWidth)),
        right: pixels(sidebarRect.right - parseFloat(sidebarStyle.borderRightWidth) - sidebarScrollRect.right),
        bottom: pixels(sidebarRect.bottom - sidebarScrollRect.bottom),
      },
      scrollPaddingTop: style.paddingTop,
      scrollPaddingBottom: style.paddingBottom,
      scrollTop: contentScroll.scrollTop,
      atBottom: Math.abs(contentScroll.scrollHeight - contentScroll.clientHeight - contentScroll.scrollTop) <= 1,
      windowWidth: window.innerWidth,
      scrollbars: [contentScroll, sidebarScroll].map((element) => {
        const thumb = getComputedStyle(element, '::-webkit-scrollbar-thumb');
        const track = getComputedStyle(element, '::-webkit-scrollbar-track');
        return { thumbBorder: thumb.borderTopWidth, trackMargins: [track.marginTop, track.marginBottom] };
      }),
    };
  })()`);
  await run('window.__settingsScreenTest.selectPage("voice")');
  await waitFor(webContents, 'Boolean(document.querySelector(\'[aria-label="语音会话模式"]\'))');
  const top = await measure();
  await capture('top');
  await run('document.querySelector(".settings-content-scroll").scrollTop = document.querySelector(".settings-content-scroll").scrollHeight');
  await delay(50);
  const bottom = await measure();
  await capture('bottom');
  await run('window.__settingsScreenTest.selectPage("general")');
  await waitFor(webContents, 'document.querySelector(".settings-content-scroll").scrollTop === 0');
  const switched = await measure();
  await run('window.__settingsScreenTest.selectPage("voice")');
  await waitFor(webContents, 'Boolean(document.querySelector(\'[aria-label="语音会话模式"]\')) && document.querySelector(".settings-content-scroll").scrollTop === 0');
  const returned = await measure();
  window.setSize(760, 720);
  await waitFor(webContents, 'window.innerWidth === 760');
  const narrow = await measure();
  await capture('narrow');
  const chrome = await run(`(() => {
    const dragRegion = document.querySelector('.settings-window-drag-region');
    const dragStyle = dragRegion && getComputedStyle(dragRegion);
    return {
      hasDragRegion: Boolean(dragRegion),
      dragRegionMode: dragStyle?.webkitAppRegion ?? null,
      dragRegionHeight: dragStyle?.height ?? null,
    };
  })()`);
  return { ...chrome, layout: { top, bottom, switched, returned, narrow } };
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
  await webContents.executeJavaScript(`[...document.querySelectorAll('.extension-catalog-tabs button')].find(el => el.textContent === '个人').click()`);
  await webContents.executeJavaScript(`(() => {
    const row = document.querySelector('.configuration-entry .extension-hub-row-open');
    if (!row) throw new Error('missing installed plugin configuration row');
    row.click();
  })()`);
  await waitFor(webContents, 'Boolean(document.querySelector(".extension-plugin-page"))');
  await webContents.executeJavaScript(`document.querySelector('.extension-plugin-configuration summary').click()`);
  const configInitial = await webContents.executeJavaScript('window.__settingsScreenTest.getFieldValue(\'[aria-label="a@b 配置"]\')');
  await webContents.executeJavaScript('window.__settingsScreenTest.setFieldValue(\'[aria-label="a@b 配置"]\', "not even json")');
  const configDirty = await webContents.executeJavaScript('window.__settingsScreenTest.getFieldValue(\'[aria-label="a@b 配置"]\')');
  await webContents.executeJavaScript('window.__settingsScreenTest.clickLayerTab("project")');
  await waitFor(webContents, '!document.querySelector(".extension-plugin-page")');
  await webContents.executeJavaScript(`[...document.querySelectorAll('.extension-catalog-tabs button')].find(el => el.textContent === '个人').click()`);
  await webContents.executeJavaScript(`document.querySelector('.configuration-entry .extension-hub-row-open').click()`);
  await webContents.executeJavaScript(`document.querySelector('.extension-plugin-configuration summary').click()`);
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
    const button = document.querySelector('[aria-label="返回应用"]');
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
  await webContents.executeJavaScript(`document.querySelector('[aria-label="返回应用"]').click()`);
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
      await webContents.executeJavaScript(`[...document.querySelectorAll('.extension-catalog-tabs button')].find(el => el.textContent === '个人').click()`);
      await webContents.executeJavaScript(`(() => {
        const row = document.querySelector('.configuration-entry .extension-hub-row-open');
        if (!row) throw new Error('missing MCP configuration row');
        row.click();
      })()`);
      await delay(50);
    }
    if (page === 'plugins') {
      await webContents.executeJavaScript(`[...document.querySelectorAll('.extension-catalog-tabs button')].find(el => el.textContent === '个人').click()`);
      await webContents.executeJavaScript(`(() => {
        const row = document.querySelector('.configuration-entry .extension-hub-row-open');
        if (!row) throw new Error('missing installed plugin configuration row');
        row.click();
      })()`);
      await waitFor(webContents, 'Boolean(document.querySelector("[aria-label=\\\"REGION option\\\"]"))');
    }
    if (page === 'hooks') {
      for (const label of ['PreToolUse', '分组 1', '处理器 1']) {
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
        structuredHookEditor: Boolean(document.querySelector('[aria-label="执行类型"]')),
        advancedHookJson: Boolean(document.querySelector('[aria-label="当前层 hooks JSON"]')),
        structuredMcpEditor: Boolean(document.querySelector('[aria-label="MCP 传输类型"]')),
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
  await fill('API 密钥', 'test-only-secret-1234');
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
  const dualAuth = await run('({ mode:document.querySelector(\'[aria-label="认证方式"]\').value, keyPresent:Boolean(document.querySelector(\'[aria-label="API 密钥"]\')) })');
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
  await fill('API 密钥', 'test-only-key');
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
    await run(`(() => { const element = Array.from(document.querySelectorAll('button')).find(b => b.textContent.trim() === ${JSON.stringify(label)}); if (!element) throw new Error('missing button: ' + ${JSON.stringify(label)} + '; visible text: ' + document.body.innerText); element.click(); })()`);
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
    if (page === 'mcp') await button('个人');
    const initialList = page === 'skills'
      ? await run(`Boolean(document.querySelector('.extension-hub-page')) && !document.querySelector('.extension-detail-dialog')`)
      : await run(`Boolean(document.querySelector('.configuration-list')) && !document.querySelector('.configuration-detail')`);
    const searchLabel = page === 'skills' ? '搜索 Skills' : '搜索 MCP 服务器';
    const query = page === 'skills' ? 'release-notes' : 'context7';
    await run(`window.__settingsScreenTest.setFieldValue(${JSON.stringify(`[aria-label="${searchLabel}"]`)}, ${JSON.stringify(query)})`);
    await delay(40);
    await capture(`${page}-list`);
    await click(page === 'skills' ? '.extension-hub-row-open' : '.configuration-entry .extension-hub-row-open');
    const detailOnly = page === 'skills'
      ? await run(`Boolean(document.querySelector('.extension-detail-dialog'))`)
      : await run(`Boolean(document.querySelector('.configuration-detail')) && !document.querySelector('.configuration-list')`);
    await capture(`${page}-detail`);
    await click(page === 'skills' ? '[aria-label="关闭详情"]' : '.configuration-back');
    const searchRetained = await run(`document.querySelector(${JSON.stringify(`[aria-label="${searchLabel}"]`)}).value === ${JSON.stringify(query)}`);
    await button('添加');
    await capture(`${page}-create`);
    const nameLabel = page === 'skills' ? '称呼' : 'MCP 服务器名称';
    const createOpened = page === 'skills'
      ? await run(`Boolean(document.querySelector(${JSON.stringify(`[aria-label="${nameLabel}"]`)})) && Boolean(document.querySelector('.extension-detail-dialog'))`)
      : await run(`Boolean(document.querySelector(${JSON.stringify(`[aria-label="${nameLabel}"]`)})) && !document.querySelector('.configuration-list')`);
    await run(`window.__settingsScreenTest.setFieldValue(${JSON.stringify(`[aria-label="${nameLabel}"]`)}, 'draft-test')`);
    await delay(40);
    await click(page === 'skills' ? '[aria-label="关闭详情"]' : '.configuration-back');
    const guarded = page === 'skills'
      ? await run(`Boolean(document.querySelector('.extension-detail-dialog')) && document.body.textContent.includes('丢弃并切换')`)
      : await run(`Boolean(document.querySelector('.configuration-detail')) && document.body.textContent.includes('丢弃并切换')`);
    await button('继续编辑');
    const draftRetained = await run(`document.querySelector(${JSON.stringify(`[aria-label="${nameLabel}"]`)}).value === 'draft-test'`);
    await click(page === 'skills' ? '[aria-label="关闭详情"]' : '.configuration-back');
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
  const hooksDetail = await run(`Boolean(document.querySelector('[aria-label="命令"]')) && !document.querySelector('.configuration-list')`);
  await run(`window.__settingsScreenTest.setFieldValue('[aria-label="命令"]', 'echo hook-test')`);
  await delay(50);
  await capture('hooks-create');
  await click('.configuration-back');
  await click('.configuration-entry');
  const hooksGuarded = await run(`document.body.textContent.includes('丢弃并切换')`);
  await button('继续编辑');
  const hooksDraftRetained = await run(`document.querySelector('[aria-label="命令"]').value === 'echo hook-test'`);
  results.hooks = { initialList: hooksList, detailOnly: hooksDetail, guarded: hooksGuarded, draftRetained: hooksDraftRetained };
  return results;
}

async function runCustomizeScenario(window, webContents) {
  window.setSize(1512, 982);
  const run = (code) => webContents.executeJavaScript(code);
  const click = async (selector) => {
    await run(`(() => { const el = document.querySelector(${JSON.stringify(selector)}); if (!el) throw new Error('missing ' + ${JSON.stringify(selector)}); el.click(); })()`);
    await delay(40);
  };
  const button = async (text) => {
    await run(`(() => { const el = [...document.querySelectorAll('button')].find(el => el.textContent.trim() === ${JSON.stringify(text)}); if (!el) throw new Error('missing button ' + ${JSON.stringify(text)}); el.click(); })()`);
    await delay(40);
  };
  const capture = async (name) => {
    const dir = process.env.LINGXI_SETTINGS_SCREENSHOT_DIR;
    if (!dir) return;
    window.showInactive();
    await delay(120);
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, `${new URL(url).searchParams.get('theme') || 'light'}-customize-${name}.png`), (await window.capturePage()).toPNG());
  };
  const measureShell = (page) => run(`(() => {
    const dialog = document.querySelector('[role="dialog"][aria-label="设置"]');
    const scroll = document.querySelector('.settings-content-scroll');
    const inner = document.querySelector('.settings-content-inner');
    const navScroll = document.querySelector('.settings-nav-scroll');
    const title = document.querySelector('.settings-page-title');
    if (!scroll || !inner || !navScroll || !title) throw new Error('missing shared settings shell on ' + ${JSON.stringify(page)});
    const sidebar = navScroll.parentElement;
    const navLink = dialog.querySelector('[data-nav-page="' + ${JSON.stringify(page)} + '"]');
    const bounds = dialog.getBoundingClientRect();
    const viewport = scroll.getBoundingClientRect();
    const side = sidebar.getBoundingClientRect();
    const contentStyle = getComputedStyle(inner);
    const titleStyle = getComputedStyle(title);
    const pixels = (value) => Math.round(value * 100) / 100;
    const visible = (element) => Boolean(element && element.getClientRects().length && getComputedStyle(element).visibility !== 'hidden');
    const layerSwitcher = dialog.querySelector('[data-testid="layer-switcher"]');
    return {
      page: ${JSON.stringify(page)},
      geometry: {
        sidebarWidth: pixels(side.width),
        viewportInsets: { top: pixels(viewport.top - bounds.top), right: pixels(bounds.right - viewport.right), bottom: pixels(bounds.bottom - viewport.bottom) },
        contentPadding: [contentStyle.paddingLeft, contentStyle.paddingRight, contentStyle.paddingBottom],
        titleTypography: [titleStyle.fontSize, titleStyle.fontWeight, titleStyle.letterSpacing, titleStyle.lineHeight],
        navPages: [...navScroll.querySelectorAll('[data-nav-page]')].map((element) => element.dataset.navPage),
      },
      title: title.textContent.trim(),
      navigationLabel: navLink?.textContent.trim(),
      selectedNavigation: navLink?.getAttribute('aria-current'),
      settingsSearchVisible: visible(dialog.querySelector('[aria-label="搜索设置"]')),
      backButtonCount: dialog.querySelectorAll('[aria-label="返回应用"]').length,
      contentScrollCount: dialog.querySelectorAll('.settings-content-scroll').length,
      legacyChromeCount: dialog.querySelectorAll('.extension-customize-window, .extension-customize-rail, .extension-sidebar, .extension-window-navigation').length,
      duplicateHeadingCount: dialog.querySelectorAll('.extension-hub-header h1').length,
      nestedScrollCount: [...dialog.querySelectorAll('.extension-hub-main, .extension-hub-page')].filter((element) => ['auto', 'scroll'].includes(getComputedStyle(element).overflowY)).length,
      layerControlCount: dialog.querySelectorAll('[data-testid="layer-switcher"]').length,
      layerControlsVisible: visible(layerSwitcher),
      horizontalOverflow: scroll.scrollWidth > scroll.clientWidth + 1,
    };
  })()`);
  await click('[data-nav-page="general"]');
  const baseline = await measureShell('general');
  await capture('general');
  const shell = {};
  const names = ['Accessibility', 'Agent Browser', 'Apple Design', 'Artifacts Builder', 'Code Review', 'Frontend Design', 'Release Notes', 'Project Review'];
  const descriptions = ['Audit and improve web accessibility following WCAG guidelines.', 'Browser automation for everyday tasks and testing.', 'Build clear interfaces with thoughtful motion and typography.', 'Create interactive artifacts for your workflow.', 'Review changes for correctness and maintainability.', 'Design distinctive, production-ready interfaces.', 'Turn commits into release notes for your users.', 'Review changes in the current project.'];
  const entries = names.map((name, index) => ({ id: `skill-${index}`, name, source: index === 7 ? 'project' : 'user', rootDir: '/test/skills', directory: `/test/skills/${index}`, writable: true, description: descriptions[index] }));
  const markdown = '---\ndescription: Audit and improve accessibility.\n---\nComprehensive accessibility guidelines for usable interfaces.\n\n### Principles\n\n| Principle | Description |\n| --- | --- |\n| Perceivable | Present information in ways users can perceive. |\n| Operable | Make all functionality available from a keyboard. |\n| Understandable | Use clear labels and predictable interactions. |\n| Robust | Support assistive technologies. |\n\n### Review checklist\n\n- Check keyboard navigation and visible focus.\n- Verify text contrast and meaningful labels.\n- Test reduced motion and zoom.\n\n### Conformance\n\n| Level | Requirement |\n| --- | --- |\n| A | Essential accessibility |\n| AA | Standard compliance |\n';
  const installed = [
    { id: 'secure@acme', name: 'Secure Tools', display_name: 'Secure Tools', version: '1.2.0', description: 'Project automation with secure configuration.' },
    { id: 'design@acme', name: 'Design', version: '1.0.0', description: 'Design tools for your workspace.' },
  ];
  const available = [
    { id: 'review@acme', name: 'Code Review', marketplace: 'Popular', description: 'Review pull requests and find actionable improvements.', version: '2.0.0' },
    { id: 'browser@acme', name: 'Browser Tools', marketplace: 'Popular', description: 'Navigate, test, and automate the web.' },
    { ...installed[0], marketplace: 'Popular', installed: true },
    { id: 'docs@acme', name: 'Documents', marketplace: 'Popular', description: 'Create and organize documents in your workspace.' },
    { id: 'notes@acme', name: 'Release Notes', marketplace: 'Popular', description: 'Turn your changes into clear release notes.' },
    { id: 'diagram@acme', name: 'Diagrams', marketplace: 'Popular', description: 'Explore ideas with visual diagrams.' },
    { ...installed[1], marketplace: 'New & Noteworthy', installed: true },
    { id: 'terminal@acme', name: 'Terminal Tools', marketplace: 'New & Noteworthy', description: 'Work with your local development tools.' },
  ];
  await run(`window.__settingsScreenTest.setExtensionCatalogs(${JSON.stringify({ skills: { entries, trash: [] }, document: { ...entries[0], revision: 'a'.repeat(64), markdown }, plugins: { installed, available, revisions: { user: 'a'.repeat(64), project: 'b'.repeat(64) } } })})`);
  await run('window.__settingsScreenTest.selectPage("plugins")');
  await waitFor(webContents, 'Boolean(document.querySelector(".extension-hub-list"))');
  await capture('plugins');
  shell.plugins = await measureShell('plugins');
  const publicCatalog = await run(`${JSON.stringify(available.map(({ name, marketplace }) => ({ name, marketplace })))}.every(entry => {
    const group = [...document.querySelectorAll('.extension-catalog-section')].find(section => section.querySelector('h2')?.textContent.includes(entry.marketplace));
    return [...(group?.querySelectorAll('.extension-hub-row-title') ?? [])].some(title => title.textContent.trim() === entry.name);
  })`);
  const twoColumns = await run('getComputedStyle(document.querySelector(".extension-hub-list")).gridTemplateColumns.split(" ").length === 2');
  await run(`(() => { const group = [...document.querySelectorAll('.extension-catalog-section')].find(section => section.querySelector('h2')?.textContent.includes('Popular')); const row = [...group.querySelectorAll('.extension-hub-row-open')].find(button => button.textContent.includes('Code Review')); if (!row) throw new Error('missing fixture Code Review plugin'); row.click(); })()`);
  await delay(40);
  const inlineDetail = await run('Boolean(document.querySelector(".extension-plugin-page")) && !document.querySelector(".extension-detail-dialog")');
  await capture('plugin-detail');
  await button('安装插件');
  const beforeConfirm = await run('window.__settingsScreenTest.state().extensionCalls');
  await button('确认执行');
  const afterConfirm = await run('window.__settingsScreenTest.state().extensionCalls');
  await click('.extension-breadcrumb button');
  await click('[data-nav-page="skills"]');
  await capture('skills');
  shell.skills = await measureShell('skills');
  const installedPreview = await run('document.querySelectorAll(".extension-skills-installed .extension-hub-row").length === 6');
  await click('.extension-skills-installed .extension-see-more');
  const expanded = await run('document.querySelectorAll(".extension-skills-installed .extension-hub-row").length === 8');
  await run(`[...document.querySelectorAll('.extension-skills-sources .extension-catalog-tabs button')].find(button => button.textContent.trim() === '项目').click()`);
  await delay(40);
  const sourceFilter = await run('document.querySelectorAll(".extension-skills-sources .extension-hub-row").length === 1 && document.querySelector(".extension-skills-sources").textContent.includes("Project Review")');
  await run('window.__settingsScreenTest.setFieldValue(\'[aria-label="搜索 Skills"]\', "contrast-missing")');
  await delay(40);
  const emptySearch = await run('document.querySelectorAll(".extension-hub-main .extension-hub-row").length === 0 && document.body.textContent.includes("没有匹配的 Skill")');
  await run('window.__settingsScreenTest.setFieldValue(\'[aria-label="搜索 Skills"]\', "")');
  await delay(40);
  await click('.extension-skills-installed .extension-hub-row');
  await capture('skill-detail');
  const renderedMarkdown = await run('Boolean(document.querySelector(".extension-skill-preview table")) && !document.querySelector(".extension-skill-preview").textContent.includes("description:") && !document.querySelector(".extension-skill-dialog textarea")');
  const modalCentered = await run('(() => {const rect=document.querySelector(".extension-skill-dialog").getBoundingClientRect();return Math.abs(rect.x + rect.width / 2 - innerWidth / 2) < 2;})()');
  await webContents.sendInputEvent({ type: 'keyDown', keyCode: 'Tab' });
  await webContents.sendInputEvent({ type: 'keyUp', keyCode: 'Tab' });
  await delay(50);
  const trappedFocus = await run('document.querySelector(".extension-skill-dialog").contains(document.activeElement)');
  await click('[aria-label="Skill 操作"]');
  await button('编辑 Skill');
  await run('window.__settingsScreenTest.setFieldValue(\'[aria-label="Accessibility 的 Markdown"]\', "# Edited instructions")');
  await delay(40);
  await click('[aria-label="关闭详情"]');
  const dirtyGuard = await run('Boolean(document.querySelector(".extension-skill-dialog")) && document.body.textContent.includes("丢弃并切换")');
  await button('继续编辑');
  const retainedDraft = await run('document.querySelector(\'[aria-label="Accessibility 的 Markdown"]\').value === "# Edited instructions"');
  await click('[data-nav-page="general"]');
  const skillNavigationGuard = await run('document.body.textContent.includes("丢弃并切换") && Boolean(document.querySelector(".extension-skill-dialog")) && document.querySelector(".settings-page-title").textContent === "Skills"');
  await button('继续编辑');
  const skillNavigationRetainedDraft = await run('document.querySelector(\'[aria-label="Accessibility 的 Markdown"]\').value === "# Edited instructions"');
  await click('[aria-label="关闭详情"]');
  await button('丢弃并切换');
  await click('[data-nav-page="mcp"]');
  await button('个人');
  await capture('tools');
  shell.mcp = await measureShell('mcp');
  const tools = await run('document.querySelector(".settings-page-title").textContent === "MCP 服务器" && Boolean(document.querySelector(".configuration-entry"))');
  await button('添加');
  await run('window.__settingsScreenTest.setFieldValue(\'[aria-label="MCP 服务器名称"]\', "draft-mcp")');
  await delay(40);
  await click('[data-nav-page="plugins"]');
  const mcpNavigationGuard = await run('document.body.textContent.includes("丢弃并切换") && Boolean(document.querySelector(".configuration-detail")) && document.querySelector(".settings-page-title").textContent === "MCP 服务器"');
  await button('继续编辑');
  const mcpNavigationRetainedDraft = await run('document.querySelector(\'[aria-label="MCP 服务器名称"]\').value === "draft-mcp"');
  await click('[data-nav-page="general"]');
  const mcpNormalNavigationGuard = await run('document.body.textContent.includes("丢弃并切换") && document.querySelector(".settings-page-title").textContent === "MCP 服务器"');
  await button('丢弃并切换');
  const normalNavigation = await run('document.querySelector(".settings-page-title").textContent === "通用"');
  await click('[data-nav-page="plugins"]');
  await button('个人');
  await run('(() => { const row = [...document.querySelectorAll(".configuration-entry .extension-hub-row-open")].find(el => el.textContent.includes("Secure Tools")); if (!row) throw new Error("missing Personal catalog Secure Tools row"); row.click(); })()');
  await delay(60);
  const personalCatalogDetail = await run('document.querySelector(".extension-plugin-title-row h1")?.textContent === "Secure Tools"');
  await click('.extension-plugin-configuration summary');
  await run('window.__settingsScreenTest.setFieldValue(\'[aria-label="secure@acme 配置"]\', \'{"option":"draft"}\')');
  await delay(40);
  await click('[data-nav-page="general"]');
  const pluginNormalNavigationGuard = await run('document.body.textContent.includes("丢弃并切换") && Boolean(document.querySelector(".extension-plugin-page")) && document.querySelector(".settings-page-title").textContent === "插件与市场"');
  await button('继续编辑');
  const pluginNavigationRetainedDraft = await run('document.querySelector(\'[aria-label="secure@acme 配置"]\').value.includes("draft")');
  await click('[data-nav-page="skills"]');
  const pluginDraftGuard = await run('document.body.textContent.includes("丢弃并切换") && Boolean(document.querySelector(".extension-plugin-page"))');
  await button('丢弃并切换');
  window.setSize(760, 720);
  await delay(80);
  await capture('narrow');
  const narrow = await run('getComputedStyle(document.querySelector(".extension-hub-list")).gridTemplateColumns.split(" ").length === 1 && document.querySelector(".extension-hub-main").scrollWidth <= document.querySelector(".extension-hub-main").clientWidth + 1');
  shell.narrowSkills = await measureShell('skills');
  await click('[data-nav-page="plugins"]');
  await capture('narrow-plugins');
  shell.narrowPlugins = await measureShell('plugins');
  const narrowPlugins = await run('getComputedStyle(document.querySelector(".extension-hub-list")).gridTemplateColumns.split(" ").length === 1');
  await click('[data-nav-page="mcp"]');
  await capture('narrow-mcp');
  shell.narrowMcp = await measureShell('mcp');
  return { baseline, shell, publicCatalog, twoColumns, inlineDetail, beforeConfirm, afterConfirm, installedPreview, expanded, sourceFilter, emptySearch, renderedMarkdown, modalCentered, trappedFocus, dirtyGuard, retainedDraft, skillNavigationGuard, skillNavigationRetainedDraft, tools, mcpNavigationGuard, mcpNavigationRetainedDraft, mcpNormalNavigationGuard, normalNavigation, personalCatalogDetail, pluginNormalNavigationGuard, pluginNavigationRetainedDraft, pluginDraftGuard, narrow, narrowPlugins };
}

async function runMacosAudit(window, webContents) {
  const run = (expression) => webContents.executeJavaScript(expression);
  const pages = await run('[...document.querySelectorAll("[data-nav-page]")].map(el => el.dataset.navPage)');
  const results = [];
  const dir = process.env.LINGXI_SETTINGS_SCREENSHOT_DIR;
  if (dir) mkdirSync(dir, { recursive: true });
  for (const width of [1000, 760]) {
    window.setContentSize(width, 720);
    for (const page of pages) {
      await run(`window.__settingsScreenTest.selectPage(${JSON.stringify(page)})`);
      await delay(100);
      const state = await run(`(() => {
        const scroll = document.querySelector('.settings-content-scroll');
        const visible = el => el.getClientRects().length && getComputedStyle(el).visibility !== 'hidden';
        const switches = [...document.querySelectorAll('[role="switch"]')].filter(visible);
        return {
          overflow: scroll.scrollWidth > scroll.clientWidth + 1,
          unnamedSwitches: switches.filter(el => !el.getAttribute('aria-label') && !el.getAttribute('aria-labelledby')).length,
          title: document.querySelector('.settings-page-title')?.textContent ?? '',
          placeholder: !!document.querySelector('[data-testid="page-placeholder"]'),
          overflowElements: [...scroll.querySelectorAll('*')].filter(el => visible(el) && el.getBoundingClientRect().right > scroll.getBoundingClientRect().right + 1).slice(0, 8).map(el => el.tagName + '.' + el.className),
        };
      })()`);
      if (dir) {
        window.showInactive();
        await delay(60);
        writeFileSync(join(dir, `${new URL(url).searchParams.get('theme') || 'light'}-${width}-${page}.png`), (await window.capturePage()).toPNG());
      }
      const bottomOverflow = await run(`(() => { const scroll = document.querySelector('.settings-content-scroll'); scroll.scrollTop = scroll.scrollHeight; return scroll.scrollWidth > scroll.clientWidth + 1; })()`);
      results.push({ page, width, ...state, bottomOverflow });
      if (dir && ['voice', 'tools-agent', 'permissions', 'diagnostics', 'provider-credentials'].includes(page)) {
        await delay(40);
        writeFileSync(join(dir, `${new URL(url).searchParams.get('theme') || 'light'}-${width}-${page}-bottom.png`), (await window.capturePage()).toPNG());
      }
    }
  }
  return results;
}

async function runMacosControls(window, webContents) {
  const run = expression => webContents.executeJavaScript(expression);
  const click = async selector => {
    await run(`document.querySelector(${JSON.stringify(selector)}).click()`);
    await delay(40);
  };
  await run('document.querySelector(\'[aria-label="返回应用"]\').focus()');
  await webContents.sendInputEvent({ type: 'keyDown', keyCode: 'Tab' });
  await webContents.sendInputEvent({ type: 'keyUp', keyCode: 'Tab' });
  await waitFor(webContents, 'document.activeElement.matches(\'[aria-label="搜索设置"]\')');
  const searchFocus = await run(`document.activeElement.matches('[aria-label="搜索设置"]') && getComputedStyle(document.querySelector('.settings-search')).outlineStyle === 'solid'`);
  await run('window.__settingsScreenTest.selectPage("tools-agent")');
  await delay(50);
  await run('window.__settingsScreenTest.holdSettingsWrite()');
  await click('[role="switch"][aria-label="disableArtifact"]');
  const savingDisabled = await run('document.querySelector(\'[aria-label="disableArtifact"]\').disabled');
  await run('window.__settingsScreenTest.releaseSettingsWrite()');
  await waitFor(webContents, '!document.querySelector(\'[aria-label="disableArtifact"]\').disabled');
  const selectedScope = await run('document.querySelector(\'[data-layer="user"]\').getAttribute("aria-pressed") === "true"');
  const scheme = await run('getComputedStyle(document.querySelector(".settings-screen")).colorScheme');
  webContents.debugger.attach('1.3');
  try {
    await webContents.debugger.sendCommand('Emulation.setEmulatedMedia', { features: [
      { name: 'prefers-reduced-motion', value: 'reduce' },
      { name: 'prefers-contrast', value: 'more' },
      { name: 'prefers-reduced-transparency', value: 'reduce' },
    ] });
    const accessibility = await run(`(() => {
      const toggle = document.querySelector('.settings-toggle');
      const card = document.querySelector('.settings-card');
      return {
        motion: [getComputedStyle(toggle).transitionDuration, getComputedStyle(toggle.firstElementChild).transitionDuration],
        border: getComputedStyle(card).borderTopColor,
        text: getComputedStyle(document.querySelector('.settings-screen')).color,
        selectedNavOutline: getComputedStyle(document.querySelector('[aria-current="page"]')).outlineStyle,
      };
    })()`);
    await run('window.__settingsScreenTest.selectPage("mcp")');
    await delay(60);
    const primaryContrast = await run(`(() => {
      const button = document.querySelector('.extension-hub-primary');
      const canvas = document.createElement('canvas'); canvas.width = canvas.height = 1;
      const ctx = canvas.getContext('2d');
      const luminance = color => {
        ctx.fillStyle = color; ctx.fillRect(0, 0, 1, 1);
        const values = [...ctx.getImageData(0, 0, 1, 1).data].slice(0, 3).map(v => { v /= 255; return v <= .04045 ? v / 12.92 : ((v + .055) / 1.055) ** 2.4; });
        return values[0] * .2126 + values[1] * .7152 + values[2] * .0722;
      };
      const style = getComputedStyle(button), a = luminance(style.color), b = luminance(style.backgroundColor);
      return (Math.max(a, b) + .05) / (Math.min(a, b) + .05);
    })()`);
    return { searchFocus, savingDisabled, selectedScope, scheme, accessibility, primaryContrast };
  } finally { webContents.debugger.detach(); }
}

async function runMacosEditors(window, webContents) {
  const run = expression => webContents.executeJavaScript(expression);
  const results = [];
  const dir = process.env.LINGXI_SETTINGS_SCREENSHOT_DIR;
  const theme = new URL(url).searchParams.get('theme') || 'light';
  window.setContentSize(760, 600);
  await run('window.__settingsScreenTest.setHasProject(true)');
  for (const page of ['custom-providers', 'provider-credentials', 'fusion', 'mcp', 'hooks', 'skills']) {
    await run(`window.__settingsScreenTest.selectPage(${JSON.stringify(page)})`);
    await delay(50);
    await run(`(() => {
      const page = ${JSON.stringify(page)};
      const target = page === 'custom-providers' ? [...document.querySelectorAll('button')].find(el => el.textContent.includes('新增 Provider'))
        : page === 'provider-credentials' ? document.querySelector('.settings-provider-row')
        : page === 'fusion' ? document.querySelector('.extension-hub-primary')
        : page === 'mcp' ? document.querySelector('.extension-hub-row-open')
        : page === 'skills' ? document.querySelector('.extension-hub-row')
        : [...document.querySelectorAll('button')].find(el => el.textContent.trim() === '添加');
      if (!target) throw new Error('missing editor ' + page);
      target.click();
    })()`);
    await delay(80);
    await run(`document.querySelectorAll('.settings-content-inner details:not(.extension-hub-menu)').forEach(el => { el.open = true; })`);
    const state = await run(`(() => {
      const scroll = document.querySelector('.settings-content-scroll');
      const dialog = document.querySelector('.extension-detail-dialog');
      return { overflow: scroll.scrollWidth > scroll.clientWidth + 1,
        dialogFits: !dialog || (dialog.getBoundingClientRect().left >= 0 && dialog.getBoundingClientRect().right <= innerWidth && dialog.getBoundingClientRect().bottom <= innerHeight),
        editor: !!document.querySelector('.configuration-detail, .extension-detail-dialog, .settings-provider-back, [aria-label="Profile 名称"]'),
        fieldHeights: [...document.querySelectorAll('input[type="password"], input[aria-label="enabledTools"]')].map(el => el.getBoundingClientRect().height),
      };
    })()`);
    results.push({ page, ...state });
    if (dir) {
      mkdirSync(dir, { recursive: true }); window.showInactive(); await delay(60);
      writeFileSync(join(dir, `${theme}-editor-${page}.png`), (await window.capturePage()).toPNG());
    }
    if (page === 'custom-providers') {
      // Leave the newly opened draft through its own Cancel action so the next
      // page audit still exercises the real unsaved-draft navigation guard.
      await run(`[...document.querySelectorAll('.settings-content-inner button')].find(button => button.textContent.trim() === '取消').click()`);
      await waitFor(webContents, `!document.querySelector('[aria-label="Profile 名称"]')`);
    } else if (page === 'skills') {
      await run('document.querySelector(\'[aria-label="关闭详情"]\').click()');
    }
  }
  return results;
}

async function runLocalizationScenario(window, webContents) {
  const run = expression => webContents.executeJavaScript(expression);
  const rows = [];
  window.setContentSize(760, 600);
  for (const locale of ['en', 'ja', 'ko', 'zh-Hant', 'zh-Hans']) {
    await run(`(() => { const select = document.querySelector('.settings-content-inner select'); window.__localeSelect = select; select.value = ${JSON.stringify(locale)}; select.dispatchEvent(new Event('change', { bubbles: true })); })()`);
    await waitFor(webContents, `document.documentElement.lang === ${JSON.stringify(locale)}`);
    rows.push(await run(`({locale: document.documentElement.lang, navigation: document.querySelector('[data-nav-page="general"]').textContent.trim(), search: document.querySelector('.settings-search input').placeholder, sameSelect: window.__localeSelect === document.querySelector('.settings-content-inner select'), selected: document.querySelector('.settings-content-inner select').value, overflow: document.querySelector('.settings-content-scroll').scrollWidth > document.querySelector('.settings-content-scroll').clientWidth + 1})`));
  }
  await run(`(() => { const search = document.querySelector('.settings-search input'); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(search, '音声'); search.dispatchEvent(new Event('input', { bubbles: true })); })()`);
  await run(`(() => { const select = document.querySelector('.settings-content-inner select'); select.value = 'en'; select.dispatchEvent(new Event('change', { bubbles: true })); })()`);
  await waitFor(webContents, 'document.documentElement.lang === "en"');
  const beforeSearch = await run(`Boolean(document.querySelector('[data-nav-page="voice"]'))`);
  await run(`(() => { const select = document.querySelector('.settings-content-inner select'); select.value = 'ja'; select.dispatchEvent(new Event('change', { bubbles: true })); })()`);
  await waitFor(webContents, `Boolean(document.querySelector('[data-nav-page="voice"]'))`);
  const localizedSearch = await run(`({ before: ${beforeSearch}, after: Boolean(document.querySelector('[data-nav-page="voice"]')), query: document.querySelector('.settings-search input').value })`);
  await run(`(() => { const search = document.querySelector('.settings-search input'); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(search, ''); search.dispatchEvent(new Event('input', { bubbles: true })); })()`);
  await run(`(() => { const select = document.querySelector('.settings-content-inner select'); select.value = 'en'; select.dispatchEvent(new Event('change', { bubbles: true })); })()`);
  await waitFor(webContents, 'document.documentElement.lang === "en"');
  await webContents.loadURL(url);
  await waitFor(webContents, 'Boolean(window.__settingsScreenTest?.state().settingsSnapshotReady && document.documentElement.lang === "en")');
  const restored = await run(`document.querySelector('.settings-content-inner select').value`);
  await run(`(() => { const select = document.querySelector('.settings-content-inner select'); Object.defineProperty(navigator, 'languages', { configurable: true, value: ['fr-FR', 'ja-JP'] }); select.value = ''; select.dispatchEvent(new Event('change', { bubbles: true })); window.dispatchEvent(new Event('languagechange')); })()`);
  await waitFor(webContents, 'document.documentElement.lang === "ja"');
  const system = await run(`({lang: document.documentElement.lang, preference: document.querySelector('.settings-content-inner select').value})`);
  await run(`localStorage.setItem('lingxi.desktop.language', 'ko'); window.dispatchEvent(new StorageEvent('storage', { key: 'lingxi.desktop.language' }));`);
  await waitFor(webContents, 'document.documentElement.lang === "ko"');
  return { rows, localizedSearch, restored, system, synced: await run('document.documentElement.lang') };
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
    const result = scenario === 'localization' ? await runLocalizationScenario(window, webContents)
      : scenario === 'macos-editors' ? await runMacosEditors(window, webContents)
      : scenario === 'macos-controls' ? await runMacosControls(window, webContents)
      : scenario === 'macos-audit' ? await runMacosAudit(window, webContents)
      : scenario === 'provider-region' ? await runProviderRegionScenario(window, webContents)
      : scenario === 'customize' ? await runCustomizeScenario(window, webContents)
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
      : scenario === 'window-chrome' ? await runWindowChromeScenario(window, webContents)
      : scenario === 'layer-reseed' ? await runLayerReseedScenario(webContents)
      : scenario === 'permission-rule-dispatch' ? await runPermissionRuleDispatchScenario(webContents)
      : scenario === 'remount-on-layer-switch' ? await runRemountOnLayerSwitchScenario(webContents)
      : await runLayerSwitcherScenario(webContents);
    await new Promise(resolve => process.stdout.write(`${JSON.stringify(result)}\n`, resolve));
  } finally {
    if (!window.isDestroyed()) window.destroy();
    if (app.isReady()) await app.quit();
  }
}

main().catch((error) => { process.stderr.write(`${error instanceof Error ? error.stack : String(error)}\n`); app.exit(1); });
