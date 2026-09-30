import assert from 'node:assert/strict';
import { app, BrowserWindow } from 'electron';
import { mkdir, writeFile } from 'node:fs/promises';
import { dirname } from 'node:path';

const url = process.argv.find(arg => arg.startsWith('http://'));
if (!url || !process.env.LINGXI_TEST_USER_DATA) throw new Error('isolated fixture required');
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);

async function main() {
  await app.whenReady();
  const win = new BrowserWindow({ show: false, width: 700, height: 450, webPreferences: { sandbox: true } });
  const evaluate = code => win.webContents.executeJavaScript(code);
  const settle = () => new Promise(resolve => setTimeout(resolve, 50));
  const act = async code => { await evaluate(code); await settle(); };
  const focus = () => act(`document.querySelector('.context-window-trigger').focus()`);
  const blur = () => act(`document.querySelector('#other').focus()`);
  const escape = () => act(`window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))`);
  const hover = async inside => {
    const point = inside ? await evaluate(`(() => {
      const rect = document.querySelector('.context-window-trigger').getBoundingClientRect();
      return { x: Math.round(rect.left + rect.width / 2), y: Math.round(rect.top + rect.height / 2) };
    })()`) : { x: 10, y: 10 };
    win.webContents.sendInputEvent({ type: 'mouseMove', ...point });
    await settle();
  };
  const expectOpen = async (expected, message) => {
    const state = await evaluate(`(() => {
      const button = document.querySelector('.context-window-trigger');
      const tooltip = document.querySelector('[role="tooltip"]');
      return { open: Boolean(tooltip), described: button.getAttribute('aria-describedby'), id: tooltip?.id };
    })()`);
    assert.equal(state.open, expected, message);
    assert.equal(state.described, expected ? state.id : null, `${message}: accessible description follows visibility`);
  };

  try {
    await win.loadURL(url);
    win.webContents.focus();
    for (let n = 0; n < 100 && !await evaluate(`Boolean(document.querySelector('.context-window-trigger'))`); n++) await settle();
    await hover(false);
    await expectOpen(false, 'initially closed');
    await focus();
    await expectOpen(true, 'keyboard focus opens');
    await hover(true);
    await hover(false);
    await expectOpen(true, 'mouse leave preserves keyboard focus');
    await blur();
    await expectOpen(false, 'closes when neither focused nor hovered');
    await hover(true);
    await focus();
    await blur();
    await expectOpen(true, 'blur preserves pointer hover');
    await hover(false);
    await expectOpen(false, 'final hover exit closes');

    await focus();
    await hover(true);
    await escape();
    await expectOpen(false, 'Escape dismisses while both active');
    assert.equal(await evaluate(`document.activeElement.matches('.context-window-trigger')`), true, 'Escape retains focus');
    assert.equal(await evaluate(`document.querySelector('.context-window-trigger').matches(':hover')`), true, 'Escape retains hover');
    await blur();
    await expectOpen(false, 'blur does not undo Escape dismissal');
    await focus();
    await expectOpen(true, 'fresh focus reopens');
    await escape();
    await hover(false);
    await expectOpen(false, 'mouse leave does not undo Escape dismissal');
    await hover(true);
    await expectOpen(true, 'fresh pointer entry reopens');
    await escape();
    await act(`document.querySelector('.context-window-trigger').click()`);
    await expectOpen(true, 'click reopens after Escape');
    await hover(false);
    await expectOpen(true, 'reopened tooltip preserves physical focus');

    // Placeholders retain measured usage; successful compaction invalidates it.
    const tooltipText = () => evaluate(`document.querySelector('[role="tooltip"]')?.textContent ?? ''`);
    assert.match(await tooltipText(), /79k \/ 475k tokens used/, 'live snapshot shown');
    assert.doesNotMatch(await tooltipText(), /≈/, 'token line is not prefixed as an estimate');
    await act(`window.sendUsageEvent({ type: 'usage_update', input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_creation_tokens: 0 })`);
    assert.match(await tooltipText(), /79k \/ 475k tokens used/, 'a request placeholder keeps the last snapshot');
    assert.doesNotMatch(await tooltipText(), /Usage unavailable/, 'no flash back to the unknown state');
    await act(`window.sendUsageEvent({ type: 'usage_update', input_tokens: 12000, output_tokens: 32000, cache_read_tokens: 0, cache_creation_tokens: 0 })`);
    assert.match(await tooltipText(), /12k \/ 475k tokens used/, 'the next snapshot replaces the retained one');

    await act(`window.sendUsageEvent({ type: 'compaction_status', phase: 'complete' })`);
    assert.match(await tooltipText(), /Usage unavailable/, 'compaction clears the pre-compaction measurement');
    assert.doesNotMatch(await tooltipText(), /12k/, 'component cannot resurrect stale usage');

    const screenshot = process.env.LINGXI_CONTEXT_WINDOW_SCREENSHOT;
    if (screenshot) {
      await mkdir(dirname(screenshot), { recursive: true });
      await writeFile(screenshot, (await win.webContents.capturePage()).toPNG());
    }
    await blur();
    await expectOpen(false, 'all interactions complete with tooltip closed');
    process.stdout.write(JSON.stringify({ passed: true }) + '\n');
  } finally { win.destroy(); app.quit(); }
}

main().catch(error => { console.error(error); app.exit(1); });
