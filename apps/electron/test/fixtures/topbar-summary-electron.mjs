import { configureFixtureWindow, settleFixtureAnimations } from './electron-test-environment.mjs';
import { app, BrowserWindow } from 'electron';
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';

const url = process.argv.find((argument) => argument.startsWith('http://') || argument.startsWith('https://'));
if (!url) throw new Error('fixture URL is required');
if (process.env.LINGXI_TEST_USER_DATA) app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));
async function waitFor(webContents, expression, timeout = 8_000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await webContents.executeJavaScript(expression)) return;
    await delay(25);
  }
  throw new Error(`timed out waiting for: ${expression}`);
}

async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 1400, height: 900, webPreferences: { sandbox: true, backgroundThrottling: false } });
  const checks = [];
  try {
    await window.loadURL(url);
    await configureFixtureWindow(window);
    const wc = window.webContents;
    const js = (expression) => wc.executeJavaScript(expression);
    const click = async (selector) => { await js(`document.querySelector(${JSON.stringify(selector)}).click()`); await delay(40); };
    const pointerClick = async (selector) => {
      await js(`document.querySelector(${JSON.stringify(selector)}).scrollIntoView({ block: 'nearest', inline: 'nearest', behavior: 'instant' })`);
      await js(`new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))`);
      const point = await js(`(() => {
        const button = document.querySelector(${JSON.stringify(selector)});
        const rect = button.getBoundingClientRect();
        const x = Math.round(rect.left + rect.width / 2);
        const y = Math.round(rect.top + rect.height / 2);
        if (!rect.width || !rect.height || !button.contains(document.elementFromPoint(x, y))) throw new Error('Button is not reachable: ' + ${JSON.stringify(selector)});
        return { x, y };
      })()`);
      wc.sendInputEvent({ type: 'mouseMove', ...point });
      wc.sendInputEvent({ type: 'mouseDown', button: 'left', clickCount: 1, ...point });
      wc.sendInputEvent({ type: 'mouseUp', button: 'left', clickCount: 1, ...point });
      await delay(40);
    };
    const clickText = async (scope, text) => {
      await js(`(() => { const target = [...document.querySelectorAll(${JSON.stringify(scope + ' button')})].find(el => el.textContent.includes(${JSON.stringify(text)})); if(!target) throw new Error('Missing button: '+${JSON.stringify(text)}); target.click(); })()`);
      await delay(40);
    };
    const capture = async (name) => {
      const directory = process.env.LINGXI_TOPBAR_SCREENSHOT_DIR;
      if (!directory) return;
      await js(`new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))`);
      await delay(120);
      await mkdir(directory, { recursive: true });
      await writeFile(join(directory, name + '.png'), (await wc.capturePage()).toPNG());
    };
    await waitFor(wc, `Boolean(document.querySelector('[aria-label="Toggle pinned summary"]'))`);
    assert.equal(await js(`document.querySelector('[aria-label="Toggle theme"]') === null && document.querySelector('#runtime-center-overview') === null && document.querySelector('.runtime-inspector') === null`), true);
    checks.push('initial closed controls');

    await click('[aria-label="Toggle right panel"]');
    assert.equal(await js(`document.querySelector('.runtime-inspector').getAnimations().length > 0`), true);
    await click('[aria-label="Toggle right panel"]');
    assert.equal(await js(`document.querySelector('.runtime-inspector').inert`), true);
    await click('[aria-label="Toggle right panel"]');
    await waitFor(wc, `getComputedStyle(document.querySelector('.runtime-inspector')).opacity === '1'`);
    assert.equal(await js(`document.querySelector('.runtime-inspector').inert`), false);
    assert.equal(await js(`getComputedStyle(document.querySelector('.runtime-inspector')).opacity`), '1');
    assert.equal(await js(`getComputedStyle(document.querySelector('.runtime-inspector-header')).getPropertyValue('-webkit-app-region')`), 'no-drag');
    for (const width of [900, 1400]) {
      window.setContentSize(width, 900);
      await delay(150);
      assert.equal(await js(`getComputedStyle(document.querySelector('[aria-label="Toggle right panel"]')).display`), 'none', `${width}px: the open panel owns its only visible toggle`);
      assert.equal(await js(`document.querySelector('[aria-label="Hide right panel"]').getBoundingClientRect().width > 0`), true);
    }
    await js(`document.activeElement?.blur()`);
    await capture('tools-light');
    await pointerClick('.runtime-inspector-tool');
    await waitFor(wc, `Boolean(document.querySelector('[data-runtime-inspector-active="true"]'))`);
    await pointerClick('[aria-label="Open tools"]');
    await waitFor(wc, `Boolean(document.querySelector('.runtime-inspector-landing'))`);
    await pointerClick('.runtime-inspector-resource-grid button');
    await waitFor(wc, `document.querySelector('#runtime-inspector-panel')?.textContent.includes('Reference content')`);
    await pointerClick('[aria-label="Open tools"]');
    await waitFor(wc, `Boolean(document.querySelector('.runtime-inspector-landing'))`);
    await js(`window.dispatchEvent(new CustomEvent('fixture-reset', {detail:{theme:'dark'}}))`);
    await capture('tools-dark');
    window.setContentSize(900, 900);
    await capture('tools-narrow');
    window.setContentSize(1400, 900);
    await js(`window.dispatchEvent(new CustomEvent('fixture-reset', {detail:{theme:'light'}}))`);
    await pointerClick('[aria-label="Close Review"]');
    await pointerClick('[aria-label="Close desktop-reference.png"]');
    assert.equal(await js(`['Open tools', 'Expand right panel', 'Hide right panel'].every(label => document.querySelector('.desktop-topbar [aria-label="' + label + '"]'))`), true);
    assert.equal(await js(`document.querySelectorAll('.runtime-inspector-header button[aria-label="Open tools"], .runtime-inspector-header .runtime-panel-hide').length`), 0);
    await pointerClick('[aria-label="Open pinned summary"]');
    await waitFor(wc, `Boolean(document.querySelector('#runtime-center-overview'))`);
    await click('[aria-label="Toggle pinned summary"]');
    await pointerClick('[aria-label="Expand right panel"]');
    await waitFor(wc, `Boolean(document.querySelector('[aria-label="Restore panel size"]'))`);
    assert.equal(await js(`document.querySelector('.runtime-inspector').style.width`), '100%');
    await pointerClick('[aria-label="Restore panel size"]');
    assert.equal(await js(`document.querySelector('.runtime-inspector').style.width`), '390px');
    await pointerClick('[aria-label="Hide right panel"]');
    await waitFor(wc, `!document.querySelector('.runtime-inspector')`);
    if (process.argv.includes('--inspector-controls-only')) {
      console.log(JSON.stringify({ controls: ['summary', 'expand', 'restore', 'hide'] }));
      return;
    }
    await wc.debugger.sendCommand('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-reduced-motion', value: 'reduce' }] });
    await click('[aria-label="Toggle right panel"]');
    assert.equal(await js(`document.querySelector('.runtime-inspector').getAnimations({subtree:true}).length`), 0);
    await click('[aria-label="Hide right panel"]');
    assert.equal(await js(`document.querySelector('.runtime-inspector')`), null);
    await wc.debugger.sendCommand('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-reduced-motion', value: 'no-preference' }] });


    assert.equal(await js(`Boolean(document.querySelector('.desktop-topbar [aria-label="More chat actions"], .desktop-topbar .git-topbar'))`), false);
    await click('[aria-label="Toggle pinned summary"]');
    assert.equal(await js(`Boolean(document.querySelector('#runtime-center-overview [aria-label="Context"], #runtime-center-overview [aria-label="Changes"]'))`), false);
    checks.push('summary excludes context and changes');

    await waitFor(wc, `Boolean(document.querySelector('#runtime-center-overview'))`);
    await settleFixtureAnimations(wc, '#runtime-center-overview');
    const overview = await js(`(() => {
      const panel = document.querySelector('#runtime-center-overview');
      const row = panel.parentElement;
      const bounds = panel.getBoundingClientRect();
      const natural = panel.cloneNode(true);
      natural.removeAttribute('id');
      Object.assign(natural.style, { position: 'absolute', width: bounds.width + 'px', height: 'auto', maxHeight: 'none', margin: '0', visibility: 'hidden', animation: 'none' });
      row.append(natural);
      const naturalHeight = natural.getBoundingClientRect().height;
      natural.remove();
      return { width: bounds.width, height: bounds.height, naturalHeight, rowHeight: row.getBoundingClientRect().height, scrollHeight: panel.scrollHeight, clientHeight: panel.clientHeight, radius: getComputedStyle(panel).borderRadius, sections: [...panel.querySelectorAll('h2')].map(el=>el.textContent), fourthResource: panel.textContent.includes('acceptance.md') };
    })()`);
    assert.equal(overview.width, 300);
    assert.equal(overview.radius, '20px');
    assert.deepEqual(overview.sections, ['Outputs', 'Subagents', 'Sources']);
    assert.equal(overview.fourthResource, false);
    // macOS may constrain the requested window height to the available screen.
    // The rail hugs its content until it reaches the row's 20px margin budget.
    const expectedHeight = Math.min(overview.naturalHeight, overview.rowHeight - 20);
    assert.ok(Math.abs(overview.height - expectedHeight) <= 1, `summary rail height ${overview.height} differs from content/cap height ${expectedHeight}`);
    if (overview.naturalHeight <= overview.rowHeight - 20) {
      assert.equal(overview.scrollHeight, overview.clientHeight, 'content-sized rail needs no internal scroll');
    } else {
      assert.ok(overview.scrollHeight > overview.clientHeight, 'screen-constrained rail scrolls internally');
    }
    window.setContentSize(1400, 420);
    await delay(150);
    const cramped = await js(`(() => { const panel = document.querySelector('#runtime-center-overview'); const row = panel.parentElement; return { height: panel.getBoundingClientRect().height, rowHeight: row.getBoundingClientRect().height, scrollHeight: panel.scrollHeight, clientHeight: panel.clientHeight, overflow: getComputedStyle(panel).overflowY }; })()`);
    assert.ok(cramped.height <= cramped.rowHeight - 20, `cramped summary rail ${cramped.height} exceeds row ${cramped.rowHeight}`);
    assert.equal(cramped.overflow, 'auto');
    assert.ok(cramped.scrollHeight > cramped.clientHeight, 'cramped summary rail scrolls internally');
    window.setContentSize(1400, 900);
    await delay(150);
    await js(`document.querySelector('[data-fixture-chat]').dispatchEvent(new PointerEvent('pointerdown',{bubbles:true}))`);
    assert.equal(await js(`Boolean(document.querySelector('#runtime-center-overview'))`), true);
    await capture('summary-light');
    await js(`window.dispatchEvent(new CustomEvent('fixture-reset', { detail: { showSides: true } }))`);
    await waitFor(wc, `Boolean(document.querySelector('#runtime-center-overview [aria-label="Side chats"]'))`);
    await capture('summary-populated');
    await js(`window.dispatchEvent(new CustomEvent('fixture-reset', { detail: { showSides: false } }))`);
    checks.push('pinned overview hugs content bounded scroll and resource limit');

    await click('[aria-label="Toggle right panel"]');
    await waitFor(wc, `document.querySelector('.runtime-inspector-landing')?.textContent.includes('Subagents')`);
    await clickText('.runtime-inspector-landing', 'Todos');
    await waitFor(wc, `document.querySelector('#runtime-inspector-panel')?.textContent.includes('Inspect the existing desktop layout')`);
    assert.equal(await js(`Boolean(document.querySelector('#runtime-center-overview'))`), true);
    assert.equal(await js(`document.querySelectorAll('[role="tab"]').length`), 1);
    await clickText('#runtime-center-overview [aria-label="Outputs"]', 'Desktop workspace');
    await clickText('#runtime-center-overview [aria-label="Outputs"]', 'Desktop workspace');
    assert.equal(await js(`document.querySelectorAll('[role="tab"]').length`), 2);
    await waitFor(wc, `document.querySelector('#runtime-inspector-panel')?.textContent.includes('Bring the desktop toolbar')`);
    await js(`document.querySelector('[data-runtime-inspector-active="true"]').focus()`);
    wc.sendInputEvent({type:'keyDown',keyCode:'Left'});
    wc.sendInputEvent({type:'keyUp',keyCode:'Left'});
    await waitFor(wc, `document.querySelector('[data-runtime-inspector-active="true"]')?.textContent.includes('Todos')`);
    wc.sendInputEvent({type:'keyDown',keyCode:'End'});
    wc.sendInputEvent({type:'keyUp',keyCode:'End'});
    await waitFor(wc, `document.querySelector('[data-runtime-inspector-active="true"]')?.textContent.includes('Plan')`);
    await js(`Promise.all(document.querySelector('.runtime-inspector').getAnimations({ subtree: true }).map(animation => animation.finished.catch(() => {})))`);
    await capture('details-light');
    const heights = await js(`({ chat: document.querySelector('.desktop-topbar').getBoundingClientRect().height, detail: document.querySelector('.runtime-inspector-header').getBoundingClientRect().height, panel: document.querySelector('.runtime-inspector').getBoundingClientRect().width })`);
    assert.deepEqual(heights, { chat: 40, detail: 40, panel: 390 });
    checks.push('independent panels tabs and plan todos separation');

    await click('[aria-label="Hide right panel"]');
    await waitFor(wc, `!document.querySelector('.runtime-inspector')`);
    await waitFor(wc, `document.activeElement?.getAttribute('aria-label') === 'Toggle right panel'`);
    await click('[aria-label="Toggle right panel"]');
    assert.equal(await js(`document.querySelectorAll('[role="tab"]').length`), 2);
    await click('[aria-label="Close Plan"]');
    await click('[aria-label="Close Todos"]');
    await waitFor(wc, `Boolean(document.querySelector('.runtime-inspector-landing'))`);
    checks.push('hide preserves tabs last close shows landing');

    await clickText('#runtime-center-overview [aria-label="Sources"]', 'View all');
    await waitFor(wc, `document.querySelector('#runtime-inspector-panel')?.textContent.includes('acceptance.md')`);
    await js(`document.querySelector('#runtime-center-overview button').focus()`);
    wc.sendInputEvent({type:'keyDown',keyCode:'Escape'});
    wc.sendInputEvent({type:'keyUp',keyCode:'Escape'});
    await waitFor(wc, `!document.querySelector('#runtime-center-overview')`);
    await waitFor(wc, `document.activeElement?.getAttribute('aria-label') === 'Toggle pinned summary'`);
    assert.equal(await js(`Boolean(document.querySelector('.runtime-inspector'))`), true);
    await click('[aria-label="Toggle pinned summary"]');
    await js(`document.querySelector('#runtime-center-overview button').focus()`);
    // Shift+Tab from the first summary button must escape to the toolbar.
    wc.sendInputEvent({type:'keyDown',keyCode:'Tab',modifiers:['shift']});
    wc.sendInputEvent({type:'keyUp',keyCode:'Tab',modifiers:['shift']});
    await waitFor(wc, `!document.querySelector('#runtime-center-overview').contains(document.activeElement)`);
    checks.push('escape focus and nonmodal keyboard navigation');

    await js(`window.dispatchEvent(new CustomEvent('fixture-reset',{detail:{theme:'dark'}}))`);
    await delay(100);
    await capture('details-dark');
    window.setSize(900, 760);
    await delay(150);
    assert.equal(await js(`getComputedStyle(document.querySelector('.runtime-inspector')).position`), 'absolute');
    assert.equal(await js(`document.documentElement.scrollWidth <= innerWidth`), true);
    await capture('details-narrow');
    await click('[aria-label="Hide right panel"]');
    assert.equal(await js(`Boolean(document.querySelector('#runtime-center-overview'))`), true);
    checks.push('dark theme narrow overlay and no overflow');

    window.setSize(1400, 900);
    await js(`window.dispatchEvent(new CustomEvent('fixture-reset',{detail:{theme:'light',sessionKey:'session-b',empty:true}}))`);
    await waitFor(wc, `!document.querySelector('#runtime-center-overview') && !document.querySelector('.runtime-inspector')`);
    await click('[aria-label="Toggle pinned summary"]');
    await click('[aria-label="Toggle right panel"]');
    assert.equal(await js(`document.querySelectorAll('#runtime-center-overview h2').length`), 1);
    assert.equal(await js(`document.querySelector('#runtime-center-overview').textContent.includes('No submitted plan yet.')`), false);
    assert.equal(await js(`document.querySelector('#runtime-center-overview').textContent.includes('Desktop workspace')`), false);
    await capture('empty-light');
    checks.push('session reset and empty categories stay hidden');
    await click('[aria-label="Hide right panel"]');
    await js(`window.dispatchEvent(new CustomEvent('fixture-reset', { detail: { showSides: true } }))`);
    await waitFor(wc, `Boolean(document.querySelector('#runtime-center-overview [aria-label="Side chats"]'))`);
    await clickText('#runtime-center-overview [aria-label="Side chats"]', 'Side chat');
    assert.equal(await js(`document.querySelector('.desktop-shell').dataset.selectedSide`), 'side-1');
    assert.equal(await js(`document.querySelector('#runtime-center-overview')`), null);
    await click('[aria-label="Toggle pinned summary"]');
    await pointerClick('.runtime-summary-create');
    assert.equal(await js(`document.querySelector('.desktop-shell').dataset.outputDraft`), 'Help me create a file or site.');
    assert.equal(await js(`document.querySelector('#runtime-center-overview')`), null);
    checks.push('side chat selection and output draft close the summary');
    if (process.env.LINGXI_TOPBAR_SCREENSHOT) {
      await mkdir(dirname(process.env.LINGXI_TOPBAR_SCREENSHOT), { recursive: true });
      await writeFile(process.env.LINGXI_TOPBAR_SCREENSHOT, (await wc.capturePage()).toPNG());
    }
    process.stdout.write(`${JSON.stringify({ checks, overview, heights })}\n`);
  } finally {
    if (!window.isDestroyed()) window.destroy();
    app.quit();
  }
}
main().catch((error) => { process.stderr.write(`${error instanceof Error ? error.stack : String(error)}\n`); app.exit(1); });
