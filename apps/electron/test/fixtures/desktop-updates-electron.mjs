import assert from 'node:assert/strict';
import { writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { app, BrowserWindow } from 'electron';

app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
app.commandLine.appendSwitch('disable-gpu');
const url = process.argv.find(argument => argument.startsWith('http://'));
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function waitFor(contents, expression) {
  const until = Date.now() + 8_000;
  while (Date.now() < until) {
    if (await contents.executeJavaScript(expression)) return;
    await delay(25);
  }
  throw new Error(`Timed out: ${expression}`);
}

async function main() {
await app.whenReady();
const window = new BrowserWindow({ show: false, width: 900, height: 650, webPreferences: { sandbox: true, backgroundThrottling: false } });
try {
  for (const theme of ['light', 'dark']) {
    await window.loadURL(`${url}?theme=${theme}`);
    const wc = window.webContents;
    const run = expression => wc.executeJavaScript(expression);
    await waitFor(wc, 'window.updateFixture?.listeners() === 1');
    assert.equal(await run('Boolean(document.querySelector(".desktop-update-trigger"))'), false);
    await run('window.updateFixture.show()');
    await waitFor(wc, 'Boolean(document.querySelector(".desktop-update-trigger"))');
    const style = await run(`(() => {
      const button = document.querySelector('.desktop-update-trigger');
      const rect = button.getBoundingClientRect(); const css = getComputedStyle(button);
      return { width: rect.width, height: rect.height, radius: css.borderRadius, background: css.backgroundColor, title: button.title };
    })()`);
    assert.equal(style.width, 40); assert.equal(style.height, 40); assert.equal(style.radius, '50%');
    assert.equal(style.background, 'rgb(52, 127, 255)'); assert.equal(style.title, '发现新版本 0.2.0');
    await run('document.querySelector(".desktop-update-trigger").click()');
    await waitFor(wc, 'document.activeElement?.classList.contains("desktop-update-download")');
    assert.equal(await run('document.querySelector("[role=dialog]").textContent.includes("0.1.0")'), true);
    assert.equal(await run(`(() => {
      const rect = document.querySelector('[role=dialog]').getBoundingClientRect();
      return rect.top >= 0 && rect.left >= 0 && rect.bottom <= innerHeight && rect.right <= innerWidth;
    })()`), true);
    if (process.env.LINGXI_UPDATE_SCREENSHOT_DIR) {
      const bounds = await run(`(() => {
        const rect = document.querySelector('[role=dialog]').getBoundingClientRect();
        const y = Math.max(0, Math.floor(rect.top - 16));
        return { x: 0, y, width: Math.ceil(rect.right + 16), height: innerHeight - y };
      })()`);
      writeFileSync(join(process.env.LINGXI_UPDATE_SCREENSHOT_DIR, `desktop-update-${theme}.png`), (await wc.capturePage(bounds)).toPNG());
    }
    await run('document.querySelector(".desktop-update-download").dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }))');
    await waitFor(wc, '!document.querySelector("[role=dialog]")');
    assert.equal(await run('document.activeElement?.classList.contains("desktop-update-trigger")'), true);
    await run('document.querySelector(".desktop-update-trigger").click()');
    await waitFor(wc, 'Boolean(document.querySelector("[role=dialog]"))');
    await run('window.updateFixture.fail(true); document.querySelector(".desktop-update-download").click()');
    await waitFor(wc, 'Boolean(document.querySelector("[role=alert]"))');
    assert.equal(await run('window.updateFixture.downloads()'), 0);
    await run('window.updateFixture.fail(false); document.querySelector(".desktop-update-download").click()');
    await waitFor(wc, '!document.querySelector("[role=dialog]")');
    assert.equal(await run('window.updateFixture.downloads()'), 1);
    assert.equal(await run('Boolean(document.querySelector(".desktop-update-trigger"))'), true);
    await run('document.querySelector(".desktop-update-trigger").click()');
    await waitFor(wc, 'Boolean(document.querySelector("[role=dialog]"))');
    await run('document.querySelector("#outside").dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }))');
    await waitFor(wc, '!document.querySelector("[role=dialog]")');
    await run('document.querySelector(".desktop-update-trigger").click()');
    await waitFor(wc, 'Boolean(document.querySelector("[role=dialog]"))');
    await run('document.querySelector(".desktop-update-actions button").click()');
    await waitFor(wc, '!document.querySelector(".desktop-update-trigger")');
    await run('window.updateFixture.unmount()');
    assert.equal(await run('window.updateFixture.listeners()'), 0);
  }
  console.log(JSON.stringify({ light: true, dark: true, downloadRetry: true, snooze: true, keyboard: true, cleanup: true }));
} catch (error) { console.error(error); process.exitCode = 1; }
finally { window.destroy(); app.quit(); }
}
void main().catch(error => { console.error(error); app.exit(1); });
