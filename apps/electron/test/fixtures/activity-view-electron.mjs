import { writeFile } from 'node:fs/promises';
import { configureFixtureWindow, waitForFixture } from './electron-test-environment.mjs';
import { app, BrowserWindow } from 'electron';
import assert from 'node:assert/strict';
const url = process.argv.find(arg => arg.startsWith('http://'));
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 1100, height: 760, webPreferences: { sandbox: true, backgroundThrottling: false } });
  try {
    await window.loadURL(url);
    await configureFixtureWindow(window);
    const wc = window.webContents;
    const js = expression => wc.executeJavaScript(expression);
    const wait = () => new Promise(resolve => setTimeout(resolve, 80));
    await waitForFixture(wc, `document.querySelectorAll('.tool-group-trigger').length === 2`);
    const read = `[data-tool-group-id='tool-group:read-1']`;
    const shell = `[data-tool-group-id='tool-group:shell-1']`;
    const click = async selector => {
      const point = await js(`(() => { const el = document.querySelector(${JSON.stringify(selector)}); el.scrollIntoView({block:'nearest'}); const r = el.getBoundingClientRect(); const x = Math.round(r.left+r.width/2), y = Math.round(r.top+r.height/2); if(!el.contains(document.elementFromPoint(x,y))) throw new Error('Unreachable control'); return {x,y}; })()`);
      wc.sendInputEvent({ type: 'mouseMove', ...point });
      wc.sendInputEvent({ type: 'mouseDown', button: 'left', clickCount: 1, ...point });
      wc.sendInputEvent({ type: 'mouseUp', button: 'left', clickCount: 1, ...point });
      await wait();
    };
    const content = selector => js(`document.querySelector(${JSON.stringify(selector)}).textContent`);
    const capture = async path => {
      const bounds = await js(`(() => {
        const rows = document.querySelectorAll('.transcript-row:not([data-thinking-slot])');
        const first = rows[0].getBoundingClientRect(), last = rows[rows.length - 1].getBoundingClientRect();
        return { x: Math.max(0, Math.floor(first.left - 16)), y: Math.max(0, Math.floor(first.top - 20)), width: Math.ceil(first.width + 32), height: Math.ceil(last.bottom - first.top + 40) };
      })()`);
      await writeFile(path, (await wc.capturePage(bounds)).toPNG());
    };
    const expanded = selector => js(`document.querySelector(${JSON.stringify(selector + ' > button')}).getAttribute('aria-expanded')`);
    const leaveRows = async () => {
      wc.sendInputEvent({ type: 'mouseMove', x: 1, y: 1 });
      await js('document.activeElement?.blur()');
    };
    const checkChevron = (selector, visible) => waitForFixture(wc, `getComputedStyle(document.querySelector(${JSON.stringify(selector + ' > .transcript-disclosure-chevron')})).opacity === '${visible ? 1 : 0}'`);
    const checkAdjacentChevron = async selector => {
      const gap = await js(`(() => {
        const chevron = document.querySelector(${JSON.stringify(selector + ' > .transcript-disclosure-chevron')});
        return chevron.getBoundingClientRect().left - chevron.previousElementSibling.getBoundingClientRect().right;
      })()`);
      assert.ok(gap >= 0 && gap <= 10, `disclosure stays beside its summary (${gap}px)`);
    };
    const single = '[aria-controls="disclosure-body-read-4"]';
    await leaveRows();
    await checkChevron(read + ' > button', false);
    await checkChevron(single, false);
    await checkAdjacentChevron(read + ' > button');
    await checkAdjacentChevron(single);
    assert.equal(await expanded(read), 'false');
    assert.equal(await content(read), 'Read files · 3 calls');
    assert.match(await content(shell), /1 running.*npm test/);
    assert.doesNotMatch(await content(shell), /npm run typecheck/);
    assert.equal(await js(`document.querySelectorAll('.transcript-agent-row').length`), 1);
    assert.equal(await js(`document.querySelector('.transcript-agent-row').hasAttribute('aria-expanded')`), false, 'opening an agent is not an inline disclosure');
    assert.equal(await js(`!!document.querySelector('.transcript-agent-row .transcript-disclosure-chevron')`), false);
    assert.equal(await js(`!!document.querySelector('.transcript-tool-row[data-status="running"] .transcript-disclosure-chevron')`), false, 'a call with no output has no disclosure');
    assert.doesNotMatch(await content('.desktop-stage'), /SubagentHandback|unsuccessful|failed/);
    await click('.transcript-agent-row');
    assert.equal(await js(`document.querySelector('[data-opened-agent]').dataset.openedAgent`), 'agent:guide');
    for (const selector of ['.transcript-agent-row', read + ' > button']) {
      await js(`document.querySelector(${JSON.stringify(selector)}).focus()`);
      assert.equal(await js(`getComputedStyle(document.querySelector(${JSON.stringify(selector)})).backgroundColor`), 'rgba(0, 0, 0, 0)');
    }
    await click(read + ' > button');
    assert.equal(await expanded(read), 'true');
    await checkChevron(read + ' > button', true);
    await leaveRows();
    await checkChevron(read + ' > button', false);
    assert.equal(await js(`document.querySelectorAll(${JSON.stringify(read + ' .transcript-tool-row')}).length`), 3);
    assert.match(await content(read), /Call unsuccessful/);
    assert.doesNotMatch(await content(read), /File was unavailable/, 'raw errors require their own disclosure');
    await click(read + ' [data-status="error"] button');
    assert.match(await content(read), /File was unavailable/);
    await click(read + ' > button');
    assert.doesNotMatch(await content(read), /unsuccessful/);
    // Native keyboard activation, not a scripted click, toggles the same control.
    wc.sendInputEvent({ type: 'keyDown', keyCode: 'Tab' });
    wc.sendInputEvent({ type: 'keyUp', keyCode: 'Tab' });
    await js(`document.querySelector(${JSON.stringify(read + ' > button')}).focus()`);
    await checkChevron(read + ' > button', true);
    wc.sendInputEvent({ type: 'keyDown', keyCode: 'Space' });
    wc.sendInputEvent({ type: 'keyUp', keyCode: 'Space' });
    await wait();
    assert.equal(await expanded(read), 'true');
    await click(read + ' > button');
    await click(shell + ' > button');
    await click('#finish');
    assert.equal(await expanded(shell), 'true', 'settling does not reset a disclosure the user opened');
    assert.equal(await js(`document.querySelectorAll(${JSON.stringify(shell + ' .transcript-tool-row')}).length`), 2);
    await click(shell + ' > button');
    await click('#toggle');
    assert.equal(await js(`!!document.querySelector('.activity-view')`), false);
    assert.equal(await content(read), 'Read files · 3 calls', 'Chat and Code share the same presentation');
    await click('#all-failed');
    assert.match(await content(read), /Calls unsuccessful/);
    await click('#all-failed');
    await click('#input');
    assert.equal(await js(`!!document.querySelector('.session-needs-input')`), false);
    await click('#input');
    await click('#locale');
    assert.equal(await content(read), '读取文件 · 3 次');
    await capture('/tmp/lingxi-activity-light.png');
    await click('#theme');
    await click(read + ' > button');
    await capture('/tmp/lingxi-activity-dark.png');
    window.setSize(480, 760);
    await wait();
    assert.equal(await js(`Array.from(document.querySelectorAll('.transcript-agent-row, .tool-group-trigger, .tool-call-summary')).every(el => el.scrollWidth <= el.clientWidth + 1)`), true, 'narrow layouts keep controls within their row');
    await wc.debugger.sendCommand('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-reduced-motion', value: 'reduce' }] });
    assert.equal(await js(`getComputedStyle(document.querySelector('.tool-group-trigger')).transitionDuration`), '0s');
    await checkAdjacentChevron(single);
    await capture('/tmp/lingxi-activity-narrow.png');
    await window.loadURL(new URL('subagent-activity-fixture.html', url).href);
    await waitForFixture(wc, `!!document.querySelector('.transcript-agent-group-trigger')`);
    await leaveRows();
    await checkChevron('.transcript-agent-group-trigger', false);
    await click('.transcript-agent-group-trigger');
    await checkChevron('.transcript-agent-group-trigger', true);
    assert.equal(await js(`document.querySelectorAll('.transcript-agent-members .transcript-disclosure-chevron').length`), 0);
    await leaveRows();
    await checkChevron('.transcript-agent-group-trigger', false);
    console.log(JSON.stringify({ checks: 'pointer expand/collapse, focus-only adjacent disclosures, no empty disclosures, keyboard, retained history, attempt outcomes, agent merging, themes, narrow layout, reduced motion, view toggle' }));
  } finally { window.destroy(); app.quit(); }
}
main().catch(error => { console.error(error); app.exit(1); });
