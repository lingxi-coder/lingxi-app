import assert from 'node:assert/strict';
import { app, BrowserWindow } from 'electron';
import { writeFileSync } from 'node:fs';
const url = process.argv.find(value => value.startsWith('http://'));
if (!url || !process.env.LINGXI_TEST_USER_DATA) throw new Error('isolated fixture required');
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function main() {
  await app.whenReady();
  const win = new BrowserWindow({ show: false, width: 900, height: 1000, webPreferences: { sandbox: true, backgroundThrottling: false } });
  const run = code => win.webContents.executeJavaScript(code);
  const capture = async name => {
    if (process.env.LINGXI_NOTIFICATION_SCREENSHOT_DIR) writeFileSync(`${process.env.LINGXI_NOTIFICATION_SCREENSHOT_DIR}/${name}.png`, (await win.webContents.capturePage()).toPNG());
  };
  try {
    await win.loadURL(url);
    for (let n = 0; n < 100 && !await run(`Boolean(document.querySelector('[role=alert]'))`); n++) await delay(30);
    assert.ok(await run(`Boolean(document.querySelector('[role=alert]'))`));
    // Keep browser focus independent of other apps and packaging windows.
    // DOM focus/blur still reaches the real notification component.
    win.webContents.debugger.attach('1.3');
    await win.webContents.debugger.sendCommand('Emulation.setFocusEmulationEnabled', { enabled: true });
    assert.equal(await run('document.hasFocus()'), true);
    await run(`document.querySelector('.desktop-notification-detail').focus()`);
    assert.equal(await run(`document.activeElement.className`), 'desktop-notification-detail');
    await delay(300);
    const top = await run(`document.querySelector('#conversation').getBoundingClientRect().top`);
    await capture('notification-light');
    await run(`window.notificationFixture.setDark(true)`); await delay(60);
    await capture('notification-dark');
    await delay(5100);
    assert.equal(await run(`document.activeElement.className`), 'desktop-notification-detail');
    assert.ok(await run(`Boolean(document.querySelector('[role=alert]'))`), 'focused notification must not auto-dismiss');
    await run(`document.querySelector('.desktop-notification-close').click()`); await delay(40);
    assert.equal(await run(`Boolean(document.querySelector('[role=alert]'))`), false);
    assert.equal(await run(`document.querySelector('#conversation').getBoundingClientRect().top`), top, 'dismissal must not shift transcript');
    await run(`window.notificationFixture.setError('Long error: ' + '消息'.repeat(500))`); await delay(60);
    assert.ok(await run(`(() => { const p=document.querySelector('.desktop-notification-detail'); return p.scrollHeight > p.clientHeight && p.scrollWidth <= p.clientWidth; })()`));
    await run(`document.querySelector('.desktop-notification-detail').focus(); document.activeElement.blur()`);
    assert.equal(await run(`document.querySelector('[role=alert]').contains(document.activeElement)`), false);
    await delay(5100);
    assert.equal(await run(`Boolean(document.querySelector('[role=alert]'))`), false, 'notification auto-dismisses after focus leaves it');
    await run(`window.notificationFixture.setSettings(true)`); await delay(80);
    await capture('notification-settings-dark');
    await run(`window.notificationFixture.setDark(false)`); await delay(60);
    await capture('notification-settings-light');
    await run(`document.querySelectorAll('.notification-preview-options button')[1].click()`); await delay(40);
    assert.match(await run(`document.querySelector('.notification-preview-card').textContent`), /需要你的许可/);
    await run(`document.querySelector('[role=switch]').click()`); await delay(40);
    assert.ok(await run(`document.querySelector('fieldset').disabled`));
    assert.ok(await run(`document.querySelector('[data-idle-threshold]').matches(':disabled')`));
    await run(`document.querySelector('[role=switch]').click()`); await delay(40);
    await run(`document.querySelector('[data-idle-threshold="180000"]').click()`); await delay(40);
    assert.equal(await run(`document.querySelector('[data-idle-threshold="180000"]').getAttribute('aria-pressed')`), 'true');
    win.setSize(420, 900); await delay(60);
    assert.ok(await run(`document.querySelector('main').scrollWidth <= document.querySelector('main').clientWidth`), 'settings fit a narrow viewport');
    console.log(JSON.stringify({ passed: true }));
  } finally { win.destroy(); }
}
main().then(() => app.exit(0)).catch(error => { console.error(error); app.exit(1); });
