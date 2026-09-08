import assert from 'node:assert/strict';
import { app, BrowserWindow } from 'electron';
import { mkdir, writeFile } from 'node:fs/promises';
import { dirname } from 'node:path';
const url = process.argv.find((arg) => arg.startsWith('http://'));
if (!url || !process.env.LINGXI_TEST_USER_DATA) throw new Error('isolated fixture required');
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
async function main() {
  await app.whenReady();
  const win = new BrowserWindow({ show: false, width: 900, height: 600, webPreferences: { sandbox: true } });
  const evaluate = (code) => win.webContents.executeJavaScript(code);
  const wait = () => new Promise((resolve) => setTimeout(resolve, 100));
  const state = () => evaluate(`({open: document.querySelector('.desktop-stage [aria-expanded]').getAttribute('aria-expanded'), checked: document.querySelector('[role="switch"]').getAttribute('aria-checked'), text: document.querySelector('.desktop-stage').textContent})`);
  try {
    await win.loadURL(url);
    for (let n = 0; n < 80 && !await evaluate('Boolean(window.thoughtFixture)'); n++) await wait();
    assert.equal((await state()).open, 'false');
    assert.equal((await state()).checked, 'true');
    assert.doesNotMatch((await state()).text, /Streaming reasoning/);
    await evaluate(`document.querySelector('[role="switch"]').click()`); await wait();
    assert.equal((await state()).open, 'true');
    assert.equal((await state()).checked, 'false');
    await evaluate(`document.querySelector('[role="switch"]').click()`); await wait();
    assert.equal((await state()).open, 'false');
    await evaluate(`document.querySelector('.desktop-stage [aria-expanded]').click()`); await wait();
    assert.match((await state()).text, /Streaming reasoning/);
    await evaluate('window.thoughtFixture.finish()'); await wait();
    assert.equal((await state()).open, 'true');
    assert.match((await state()).text, /Finished reasoning/);
    await evaluate(`document.querySelector('.desktop-stage [aria-expanded]').click()`); await wait();
    await evaluate(`document.querySelector('[role="switch"]').click()`); await wait();
    assert.equal((await state()).open, 'false', 'manual collapse wins over changed preference');
    await evaluate('window.thoughtFixture.switchSession()'); await wait();
    assert.equal((await state()).open, 'true', 'new session uses preference, not previous item choice');
    if (process.env.LINGXI_THOUGHT_SCREENSHOT) {
      await mkdir(dirname(process.env.LINGXI_THOUGHT_SCREENSHOT), { recursive: true });
      await writeFile(process.env.LINGXI_THOUGHT_SCREENSHOT, (await win.webContents.capturePage()).toPNG());
    }
    process.stdout.write(JSON.stringify({ passed: true }) + '\n');
  } finally { win.destroy(); app.quit(); }
}
main().catch((error) => { console.error(error); app.exit(1); });
