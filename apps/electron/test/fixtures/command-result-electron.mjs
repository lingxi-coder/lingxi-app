import { app, BrowserWindow } from 'electron';
import { mkdir, writeFile } from 'node:fs/promises';
const url = process.argv.find((arg) => arg.startsWith('http://'));
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 1000, height: 850, webPreferences: { sandbox: true } });
  try {
    await window.loadURL(url);
    const wc = window.webContents;
    for (let n = 0; n < 160 && !await wc.executeJavaScript('!!document.querySelector("#open")'); n++) await delay(25);
    await wc.executeJavaScript('document.querySelector("#open").focus(); document.querySelector("#open").click()');
    await delay(180);
    const inspect = () => wc.executeJavaScript(`(() => { const dialog = document.querySelector('[role=dialog]'); const box = dialog.getBoundingClientRect(); const composer = document.querySelector('#composer').getBoundingClientRect(); return { open: !!dialog, nonModal: !document.querySelector(':modal'), anchored: Math.abs(box.left - composer.left) < 1 && Math.abs(box.bottom + 9 - composer.top) < 1 && box.top >= 60, inputAvailable: !document.querySelector('#prompt').closest('[inert]'), text: dialog.textContent, focus: document.activeElement.id, overflow: dialog.scrollWidth > dialog.clientWidth, truncated: [...dialog.querySelectorAll('dd')].some(e => e.scrollWidth > e.clientWidth) }; })()`);
    const desktop = await inspect();
    const screenshotDir = process.env.LINGXI_COMMAND_SCREENSHOTS;
    if (screenshotDir) { await mkdir(screenshotDir, { recursive: true }); await writeFile(screenshotDir + '/usage-desktop.png', (await wc.capturePage()).toPNG()); }
    window.setSize(375, 850); await delay(100);
    const narrow = await inspect();
    if (screenshotDir) await writeFile(screenshotDir + '/usage-narrow.png', (await wc.capturePage()).toPNG());
    wc.sendInputEvent({ type: 'keyDown', keyCode: 'Escape' }); wc.sendInputEvent({ type: 'keyUp', keyCode: 'Escape' }); await delay(100);
    const dismissed = await wc.executeJavaScript('({ closed: !document.querySelector("[role=dialog]"), focus: document.activeElement.id })');
    await wc.executeJavaScript('document.querySelector("#open").click()'); await delay(50);
    await wc.executeJavaScript('document.querySelector("[aria-label=\\"Close command result\\"]").click()'); await delay(50);
    const buttonClosed = await wc.executeJavaScript('!document.querySelector("[role=dialog]")');
    await wc.executeJavaScript('document.querySelector("#open").click()'); await delay(50);
    await wc.executeJavaScript('document.querySelector("#outside").dispatchEvent(new PointerEvent("pointerdown", { bubbles: true })); document.querySelector("#outside").focus(); document.querySelector("#outside").click()'); await delay(50);
    const outside = await wc.executeJavaScript('({ closed: !document.querySelector("[role=dialog]"), focus: document.activeElement.id, acted: document.body.dataset.outsideClicked === "true" })');
    console.log(JSON.stringify({ desktop, narrow, dismissed, buttonClosed, outside }));
  } finally { window.destroy(); }
}
main().then(() => app.exit(0), (error) => { console.error(error); app.exit(1); });
