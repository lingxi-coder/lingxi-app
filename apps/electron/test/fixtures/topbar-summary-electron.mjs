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
    await click('[aria-label="Hide right panel"]');
    await waitFor(wc, `!document.querySelector('.runtime-inspector')`);
    await wc.debugger.sendCommand('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-reduced-motion', value: 'reduce' }] });
    await click('[aria-label="Toggle right panel"]');
    assert.equal(await js(`document.querySelector('.runtime-inspector').getAnimations({subtree:true}).length`), 0);
    await click('[aria-label="Hide right panel"]');
    assert.equal(await js(`document.querySelector('.runtime-inspector')`), null);
    await wc.debugger.sendCommand('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-reduced-motion', value: 'no-preference' }] });


    assert.equal(await js(`Boolean(document.querySelector('.desktop-topbar [aria-label="More chat actions"], .desktop-topbar .git-topbar'))`), false);
    await click('[aria-label="Toggle pinned summary"]');
    await click('[aria-label="Open context summaries"]');
    await waitFor(wc, `Boolean(document.querySelector('#context-summary-panel'))`);
    await waitFor(wc, `document.activeElement?.getAttribute('aria-label') === 'Close context summaries'`);
    await js(`document.querySelectorAll('[role="option"]')[1].click()`);
    await waitFor(wc, `document.querySelector('.context-summary-markdown')?.textContent.includes('Provider routing')`);
    await click('[aria-label="Close context summaries"]');
    await waitFor(wc, `document.activeElement?.getAttribute('aria-label') === 'Open context summaries'`);
    await click('[aria-label="Open context summaries"]');
    await waitFor(wc, `document.activeElement?.getAttribute('aria-label') === 'Close context summaries'`);
    await capture('context-detail');
    wc.sendInputEvent({type:'keyDown',keyCode:'Escape'});
    wc.sendInputEvent({type:'keyUp',keyCode:'Escape'});
    await waitFor(wc, `!document.querySelector('#context-summary-panel') && Boolean(document.querySelector('#runtime-center-overview'))`);
    await click('[aria-label="Compact conversation"]');
    await waitFor(wc, `document.querySelector('.desktop-shell').dataset.compactCalls === '1'`);
    assert.equal(await js(`document.querySelector('[aria-label="Compact conversation"]').disabled`), true);
    await click('[aria-label="Compact conversation"]');
    assert.equal(await js(`document.querySelector('.desktop-shell').dataset.compactCalls`), '1');
    checks.push('context browsing focus and guarded compaction in summary');

    await waitFor(wc, `Boolean(document.querySelector('#runtime-center-overview'))`);
    await settleFixtureAnimations(wc, '#runtime-center-overview');
    const overview = await js(`(() => { const panel = document.querySelector('#runtime-center-overview'); const row = panel.parentElement; return { width: panel.getBoundingClientRect().width, height: panel.getBoundingClientRect().height, rowHeight: row.getBoundingClientRect().height, scrollHeight: panel.scrollHeight, clientHeight: panel.clientHeight, radius: getComputedStyle(panel).borderRadius, sections: [...panel.querySelectorAll('h2')].map(el=>el.textContent), fourthResource: panel.textContent.includes('acceptance.md') }; })()`);
    assert.equal(overview.width, 300);
    assert.equal(overview.radius, '20px');
    assert.deepEqual(overview.sections, ['Context', 'Subagents', 'Resources', 'Plan']);
    assert.equal(overview.fourthResource, false);
    // The rail hugs its sections instead of stretching to the transcript row.
    assert.ok(overview.height < overview.rowHeight - 20, `summary rail height ${overview.height} fills row ${overview.rowHeight}`);
    assert.equal(overview.scrollHeight, overview.clientHeight, 'content-sized rail needs no internal scroll');
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
    checks.push('pinned overview hugs content bounded scroll and resource limit');

    await click('[aria-label="Toggle right panel"]');
    await waitFor(wc, `document.querySelector('.runtime-inspector-landing')?.textContent.includes('Subagents')`);
    await clickText('.runtime-inspector-landing', 'Todos');
    await waitFor(wc, `document.querySelector('#runtime-inspector-panel')?.textContent.includes('Inspect the existing desktop layout')`);
    assert.equal(await js(`Boolean(document.querySelector('#runtime-center-overview'))`), true);
    assert.equal(await js(`document.querySelectorAll('[role="tab"]').length`), 1);
    await clickText('#runtime-center-overview [aria-label="Plan"]', 'Desktop workspace');
    await clickText('#runtime-center-overview [aria-label="Plan"]', 'Desktop workspace');
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
    assert.deepEqual(heights, { chat: 56, detail: 56, panel: 390 });
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

    await clickText('#runtime-center-overview [aria-label="Resources"]', 'View all');
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
    // Exercise actual wheel input, so an unconstrained overflow:auto child cannot pass.
    await click('[aria-label="Hide right panel"]');
    await js(`window.dispatchEvent(new CustomEvent('fixture-reset',{detail:{longSummaries:true}}))`);
    await click('[aria-label="Open context summaries"]');
    await waitFor(wc, `document.querySelectorAll('.context-summary-item').length === 13`);
    const panes = ['.context-summary-list', '.context-summary-markdown'];
    const measure = () => js(`(() => {
      const panel = document.querySelector('#context-summary-panel');
      const bounds = panel.getBoundingClientRect();
      return {
        viewport: { width: innerWidth, height: innerHeight },
        bounds: { left: bounds.left, top: bounds.top, right: bounds.right, bottom: bounds.bottom },
        panes: ${JSON.stringify(panes)}.map(selector => {
          const element = document.querySelector(selector);
          return { top: element.scrollTop, height: element.clientHeight, contentHeight: element.scrollHeight, width: element.clientWidth, contentWidth: element.scrollWidth };
        }),
        headers: ['.context-summary-heading', '.context-summary-detail-heading'].map(selector => {
          const rect = document.querySelector(selector).getBoundingClientRect();
          return { top: rect.top, bottom: rect.bottom };
        }),
        overflow: ['#context-summary-panel', '.context-summary-sidebar', '.context-summary-detail'].some(selector => {
          const element = document.querySelector(selector);
          return element.scrollWidth > element.clientWidth + 1;
        }),
      };
    })()`);
    for (const [name, width, height] of [['regular', 1400, 900], ['narrow', 600, 600], ['short', 900, 360]]) {
      window.setContentSize(width, height);
      await delay(150);
      await js(`${JSON.stringify(panes)}.forEach(selector => { document.querySelector(selector).scrollTop = 0; })`);
      await capture(`context-long-${name}`);
      const initial = await measure();
      assert.ok(initial.bounds.left >= 0 && initial.bounds.top >= 0 && initial.bounds.right <= width && initial.bounds.bottom <= height, `${name}: dialog fits viewport`);
      assert.equal(initial.overflow, false, `${name}: no horizontal container overflow`);
      for (const [index, selector] of panes.entries()) {
        assert.ok(initial.panes[index].height > 0 && initial.panes[index].contentHeight > initial.panes[index].height, `${name}: ${selector} is a bounded scroll pane`);
        // Let smooth scrolling and Chromium's previous wheel gesture settle.
        await delay(250);
        const before = await measure();
        const point = await js(`(() => { const rect = document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect(); return { x: Math.round(rect.left + rect.width / 2), y: Math.round(rect.top + Math.min(35, rect.height / 2)) }; })()`);
        wc.sendInputEvent({ type: 'mouseMove', ...point });
        wc.sendInputEvent({ type: 'mouseWheel', ...point, deltaX: 0, deltaY: -240, canScroll: true });
        await waitFor(wc, `document.querySelector(${JSON.stringify(selector)}).scrollTop > ${before.panes[index].top}`);
        await delay(250);
        const after = await measure();
        assert.equal(after.panes[1 - index].top, before.panes[1 - index].top, `${name}: panes scroll independently`);
        assert.deepEqual(after.headers, initial.headers, `${name}: headers stay pinned`);
        assert.ok(after.headers.every(header => header.top >= after.bounds.top && header.bottom <= after.bounds.bottom), `${name}: headers remain visible`);
      }
      assert.equal((await measure()).panes[1].contentWidth <= initial.panes[1].width + 1, true, `${name}: long inline paths wrap inside markdown`);
    }
    checks.push('long context bounded layout independent wheel scrolling and pinned headers at three viewport sizes');
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
