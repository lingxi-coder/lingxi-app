import { app, BrowserWindow, protocol, session, webContents } from 'electron';
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { configureFixtureWindow, waitForFixture } from './electron-test-environment.mjs';

const url = process.argv.find(argument => argument.startsWith('http://'));
if (!url || !process.env.LINGXI_TEST_USER_DATA || !process.env.LINGXI_VISUALIZATION_GUEST_PRELOAD) throw new Error('isolated fixture configuration required');
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
protocol.registerSchemesAsPrivileged([{ scheme: 'lingxi-viz', privileges: { standard: true, secure: true, supportFetchAPI: true } }]);
const delay = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds));
const trace = message => process.stderr.write(`visualization fixture: ${message}\n`);
const shell = `<!doctype html><html><head><meta charset="UTF-8"><style>
  * { box-sizing: border-box; } body { margin: 0; padding: 24px; font: 13px -apple-system,BlinkMacSystemFont,sans-serif; color: var(--color-text-primary,#1d1d1f); background: var(--color-background-primary,#fff); }
  .eyebrow { color: var(--color-text-secondary,#636366); font-size: 11px; } h2 { margin: 7px 0 6px; font-size: 24px; font-weight: 550; letter-spacing: -.6px; } p { margin: 0; color: var(--color-text-secondary,#636366); }
  .chart { display: grid; grid-template-columns: repeat(5,minmax(0,1fr)); gap: 18px; align-items: end; margin: 22px 0 18px; height: 105px; } .day { display: grid; gap: 9px; text-align: center; font-size: 10px; color: var(--color-text-secondary,#636366); } .bar { border-radius: 5px; background: #71978b; }
  footer { display: flex; align-items: center; gap: 10px; } button { border: 1px solid #8885; color: inherit; background: transparent; padding: 7px 10px; border-radius: 7px; font: inherit; } output { font-variant-numeric: tabular-nums; }
</style></head><body><div class="eyebrow">YOUR WEEK, AT A GLANCE</div><h2>Time well spent.</h2><p>A little more space for focused work.</p><div class="chart"><div class="day"><div class="bar" style="height:54px"></div>Mon</div><div class="day"><div class="bar" style="height:76px"></div>Tue</div><div class="day"><div class="bar" style="height:64px"></div>Wed</div><div class="day"><div class="bar" style="height:85px"></div>Thu</div><div class="day"><div class="bar" style="height:46px"></div>Fri</div></div><footer><button id="increment">Add focus hour</button><output id="count">0</output><span>hours added</span></footer><script>
  let generation = 0;
  window.fixtureState = { count: 0, mountMessages: 0, expanded: false, instance: Math.random().toString(36) };
  const post = message => window.lingxiVisualization.postMessage(JSON.stringify({ generation, ...message }));
  const theme = value => { for (const [key, color] of Object.entries(value.tokens)) document.documentElement.style.setProperty(key, color); };
  document.querySelector('#increment').addEventListener('click', () => { window.fixtureState.count++; document.querySelector('#count').textContent = String(window.fixtureState.count); });
  window.lingxiVisualization.onHostMessage(raw => {
    const message = JSON.parse(raw);
    if (message.type === 'mount') { generation = message.generation; window.fixtureState.mountMessages++; window.fixtureState.expanded = message.expanded; post({ type: 'ready' }); post({ type: 'resize', height: 304, clamped: false }); }
    if (message.type === 'expanded') window.fixtureState.expanded = message.expanded;
    if (message.type === 'theme') theme(message.theme);
  });
  post({ type: 'shell.ready' });
</script></body></html>`;

async function main() {
  trace('starting');
  await app.whenReady();
  trace('app ready');
  const partition = session.fromPartition('lingxi-visualization');
  partition.protocol.handle('lingxi-viz', () => new Response(shell, { headers: { 'Content-Type': 'text/html; charset=utf-8' } }));
  const window = new BrowserWindow({ show: false, width: 1080, height: 850, webPreferences: { sandbox: true, contextIsolation: true, webviewTag: true, backgroundThrottling: false } });
  const contents = window.webContents;
  contents.on('will-attach-webview', (_event, preferences) => {
    preferences.preload = process.env.LINGXI_VISUALIZATION_GUEST_PRELOAD;
    preferences.sandbox = true;
    preferences.contextIsolation = true;
    preferences.nodeIntegration = false;
  });
  contents.on('did-attach-webview', (_event, guest) => {
    guest.on('before-input-event', (event, input) => {
      if (input.type === 'keyDown' && input.key === 'Escape') {
        event.preventDefault();
        void contents.executeJavaScript(`window.visualizationFixture.guestEvent(${JSON.stringify({ webContentsId: guest.id, reason: 'escape' })})`);
      }
    });
  });
  contents.on('console-message', event => { if (event.level >= 3) process.stderr.write(`${event.message}\n`); });
  const run = script => contents.executeJavaScript(script);
  const wait = expression => waitForFixture(contents, expression);
  const click = selector => run(`document.querySelector(${JSON.stringify(selector)}).click()`);
  const key = async (target, keyCode) => {
    target.sendInputEvent({ type: 'keyDown', keyCode });
    target.sendInputEvent({ type: 'keyUp', keyCode });
    await delay(50);
  };
  const isExpanded = 'Boolean(document.querySelector(".visualization-panel:modal"))';
  const expand = async () => {
    await run('document.querySelector("[aria-label=\\"Expand visualization\\"]").focus()');
    await click('[aria-label="Expand visualization"]');
    await wait(isExpanded);
  };
  const collapse = async () => {
    await click('[aria-label="Close expanded visualization"]');
    await wait(`!${isExpanded}`);
  };
  const checkBounds = async () => {
    const bounds = await run(`(() => {
      const selectors = ['.visualization-panel', '.visualization-toolbar', '.visualization-action'];
      return { width: innerWidth, height: innerHeight, overflow: document.querySelector('main').scrollWidth > innerWidth, elements: selectors.map(selector => {
        const element = document.querySelector(selector); const rect = element.getBoundingClientRect();
        return { selector, left: rect.left, right: rect.right, top: rect.top, bottom: rect.bottom, width: rect.width, height: rect.height };
      }) };
    })()`);
    assert.equal(bounds.overflow, false, JSON.stringify(bounds));
    for (const element of bounds.elements) {
      assert.ok(element.width > 0 && element.height > 0, JSON.stringify(element));
      assert.ok(element.left >= -1 && element.right <= bounds.width + 1, JSON.stringify(bounds));
      if (await run(isExpanded)) assert.ok(element.top >= -1 && element.bottom <= bounds.height + 1, JSON.stringify(bounds));
    }
    assert.ok(bounds.elements[1].height <= 60, 'header must stay compact');
  };
  const capture = async (name, rect) => {
    if (!process.env.LINGXI_VISUALIZATION_SCREENSHOTS) return;
    await delay(100);
    await mkdir(process.env.LINGXI_VISUALIZATION_SCREENSHOTS, { recursive: true });
    const image = await contents.capturePage(rect);
    await writeFile(join(process.env.LINGXI_VISUALIZATION_SCREENSHOTS, `${name}.png`), image.toPNG());
  };
  try {
    await window.loadURL(url);
    trace('page loaded');
    await configureFixtureWindow(window);
    trace('window configured');
    window.showInactive();
    await wait('Boolean(window.visualizationFixture)');
    await wait('document.querySelector(".visualization-title")?.textContent === "A calmer week"');
    await wait('document.querySelector(".visualization-content")?.getAttribute("aria-busy") === "false"');
    trace('guest ready');
    assert.equal(await run('document.activeElement?.id'), 'outside-action', 'inline visualization must not steal conversation focus');

    const skill = await run(`(() => { const row = document.querySelector('.transcript-tool-row'); const button = row.querySelector('button'); return { expanded: button.getAttribute('aria-expanded'), height: row.getBoundingClientRect().height, body: row.innerText.includes('END_OF_SKILL_INSTRUCTIONS') }; })()`);
    assert.equal(skill.expanded, 'false');
    assert.equal(skill.body, false);
    assert.ok(skill.height <= 56, JSON.stringify(skill));
    await click('.tool-disclosure-trigger');
    await wait('document.querySelector(".transcript-tool-row").innerText.includes("END_OF_SKILL_INSTRUCTIONS")');
    assert.equal(await run('document.querySelector(".tool-disclosure-trigger").getAttribute("aria-expanded")'), 'true');
    const headings = await run('Array.from(document.querySelectorAll(".tool-skill-instructions h1, .tool-skill-instructions h2")).map(element => Number.parseFloat(getComputedStyle(element).fontSize))');
    assert.ok(headings.length >= 2 && headings.every(size => size <= 14), JSON.stringify(headings));
    await capture('desktop-light-skill-expanded');
    await click('.tool-disclosure-trigger');

    const guestId = await run('window.originalGuest = document.querySelector("webview"); window.originalGuest.getWebContentsId()');
    const guest = webContents.fromId(guestId);
    assert.ok(guest);
    await guest.executeJavaScript('document.querySelector("#increment").click()');
    const original = await guest.executeJavaScript('window.fixtureState');
    assert.equal(original.count, 1);
    assert.equal(original.mountMessages, 1);
    assert.equal(await run('document.querySelector("[aria-label=\\"Expand visualization\\"]") !== null'), true, 'even a short widget has an Expand action');
    await checkBounds();
    await capture('desktop-light-inline');
    const previewBounds = await run(`(() => {
      const skill = document.querySelector('.transcript-tool-row').getBoundingClientRect();
      const panel = document.querySelector('.visualization-panel').getBoundingClientRect();
      return { x: Math.floor(panel.left - 8), y: Math.floor(skill.top - 8), width: Math.ceil(panel.width + 16), height: Math.ceil(panel.bottom - skill.top + 16) };
    })()`);
    await capture('desktop-light-preview', previewBounds);

    await expand();
    assert.equal(await run('document.querySelector("webview") === window.originalGuest'), true);
    assert.equal(await run('document.querySelector("webview").getWebContentsId()'), guestId);
    await wait('document.activeElement?.getAttribute("aria-label") === "Close expanded visualization"');
    await run('document.querySelector("#outside-action").focus()');
    assert.equal(await run('document.activeElement?.id === "outside-action"'), false, 'modal makes background controls inert');
    await guest.executeJavaScript('document.querySelector("#increment").click()');
    assert.equal((await guest.executeJavaScript('window.fixtureState')).count, 2);
    await checkBounds();
    await capture('desktop-light-expanded');
    await collapse();
    await wait('document.activeElement?.getAttribute("aria-label") === "Expand visualization"');
    assert.equal(await run('document.querySelector("webview").getWebContentsId()'), guestId);
    const restored = await guest.executeJavaScript('window.fixtureState');
    assert.equal(restored.count, 2);
    assert.equal(restored.instance, original.instance);
    assert.equal(restored.mountMessages, 1);

    await expand();
    await key(contents, 'Escape');
    await wait(`!${isExpanded}`);
    await wait('document.activeElement?.getAttribute("aria-label") === "Expand visualization"');
    await expand();
    await guest.executeJavaScript('document.querySelector("#increment").focus()');
    await key(guest, 'Escape');
    await wait(`!${isExpanded}`);

    await run('window.visualizationFixture.theme(true)');
    await wait('document.querySelector("main").style.background === "rgb(30, 30, 32)"');
    await waitForFixture(guest, 'getComputedStyle(document.body).backgroundColor === "rgb(30, 30, 32)"');
    assert.equal(await run('document.querySelector("webview").getWebContentsId()'), guestId);
    await checkBounds();
    await capture('desktop-dark-inline');
    window.setContentSize(390, 844);
    await wait('innerWidth === 390');
    await checkBounds();
    await capture('desktop-dark-narrow');
    await expand();
    await checkBounds();
    await capture('desktop-dark-narrow-expanded');
    await collapse();
    window.setContentSize(1080, 820);
    await wait('innerWidth === 1080');

    for (const mode of ['loading', 'unavailable']) {
      await run(`window.visualizationFixture.reset(${JSON.stringify(mode)}, true)`);
      await wait(isExpanded);
      await wait(`document.querySelector('.visualization-state')?.innerText.includes(${JSON.stringify(mode === 'loading' ? 'Loading visualization' : 'Visualization unavailable')})`);
      assert.equal(await run('document.querySelector("[aria-label=\\"Close expanded visualization\\"]") !== null'), true);
      await checkBounds();
      await capture(`desktop-${mode}-expanded`);
      await key(contents, 'Escape');
      await wait(`!${isExpanded}`);
      await expand();
      await collapse();
    }

    await run('window.visualizationFixture.reset("ready")');
    await wait('document.querySelector("[data-visualization-id=weekly] .visualization-title")?.textContent === "A calmer week"');
    await run('window.visualizationFixture.pool(4)');
    await wait('document.querySelectorAll("[data-visualization-id^=pool-]").length === 4');
    await wait('!document.querySelector("[data-visualization-id=weekly] webview")');
    await expand();
    await wait('document.querySelector("[data-visualization-id=weekly] .visualization-content")?.getAttribute("aria-busy") === "false"');
    const reactivatedGuest = await run('document.querySelector("[data-visualization-id=weekly] webview").getWebContentsId()');
    assert.notEqual(reactivatedGuest, guestId, 'expanding an evicted card starts a new guest');
    await run('window.visualizationFixture.pool(7)');
    await delay(1900);
    assert.equal(await run('document.querySelector("[data-visualization-id=weekly] webview")?.getWebContentsId()'), reactivatedGuest, 'expanded guest must remain pinned while other cards compete for slots');
    await collapse();
    await run('window.visualizationFixture.pool(0)');
    process.stdout.write(JSON.stringify({ passed: true, preservedGuest: true, guestId, screenshots: process.env.LINGXI_VISUALIZATION_SCREENSHOTS ?? null }) + '\n');
  } finally {
    window.destroy();
    app.quit();
  }
}
main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); app.exit(1); });
