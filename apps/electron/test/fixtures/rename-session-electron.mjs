import { configureFixtureWindow, settleFixtureAnimations } from './electron-test-environment.mjs';
import assert from 'node:assert/strict';
import { app, BrowserWindow } from 'electron';
import { writeFileSync } from 'node:fs';
const url = process.argv.find(value => value.startsWith('http://'));
if (!url || !process.env.LINGXI_TEST_USER_DATA) throw new Error('isolated fixture required');
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function main() {
  await app.whenReady();
  const win = new BrowserWindow({ show: false, width: 900, height: 550, webPreferences: { sandbox: true, backgroundThrottling: false } });
  try {
    await win.loadURL(url);
    await configureFixtureWindow(win);
    const run = code => win.webContents.executeJavaScript(code);
    for (let n = 0; n < 100 && !await run(`Boolean(document.querySelector('input'))`); n++) await delay(20);
    assert.equal(await run(`document.activeElement.tagName`), 'INPUT');
    await settleFixtureAnimations(win.webContents, '[role=dialog]');
    const gap = await run(`document.querySelector('.desktop-dialog-actions').getBoundingClientRect().top - document.querySelector('input').getBoundingClientRect().bottom`);
    assert.ok(gap >= 20, `input/action spacing: ${gap}px`);
    if (process.env.LINGXI_RENAME_SCREENSHOT) writeFileSync(process.env.LINGXI_RENAME_SCREENSHOT, (await win.webContents.capturePage()).toPNG());
    await run(`(() => { const input = document.querySelector('input'); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, '  New chat name  '); input.dispatchEvent(new Event('input', { bubbles: true })); })()`);
    await delay(30);
    const point = await run(`(() => { const r = document.querySelector('button[type=submit]').getBoundingClientRect(); return { x: Math.round(r.x+r.width/2), y: Math.round(r.y+r.height/2) }; })()`);
    win.webContents.sendInputEvent({ type: 'mouseDown', ...point, button: 'left', clickCount: 1 });
    win.webContents.sendInputEvent({ type: 'mouseUp', ...point, button: 'left', clickCount: 1 });
    await delay(50);
    assert.equal(await run(`document.querySelector('#calls').textContent`), '1');
    assert.equal(await run(`document.querySelector('#saved-title').textContent`), 'New chat name');
    assert.equal(await run(`document.querySelector('button[type=submit]').disabled`), true);
    await run(`document.querySelector('form').dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }))`);
    assert.equal(await run(`document.querySelector('#calls').textContent`), '1', 'busy form must not submit twice');
    await run(`window.renameFixture.fail()`); await delay(30);
    assert.match(await run(`document.querySelector('[role=alert]').textContent`), /Unable to save/);
    await run(`document.querySelector('button[type=submit]').click()`); await delay(30);
    assert.equal(await run(`document.querySelector('#calls').textContent`), '2');
    await run(`window.renameFixture.succeed()`); await delay(30);
    assert.equal(await run(`Boolean(document.querySelector('[role=dialog]'))`), false);
    process.stdout.write(JSON.stringify({ passed: true, gap })+'\n');
  } finally { win.destroy(); app.quit(); }
}
main().catch(error => { process.stderr.write(String(error.stack)+'\n'); app.exit(1); });
