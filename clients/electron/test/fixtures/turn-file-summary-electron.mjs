import { app, BrowserWindow } from 'electron';
import { writeFile } from 'node:fs/promises';
const url = process.argv.find(arg => arg.startsWith('http://'));
if (!url || !process.env.LINGXI_TEST_USER_DATA) throw new Error('fixture URL and isolated user data required');
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function main() {
  await app.whenReady();
  const win = new BrowserWindow({ show: false, width: 850, height: 380, webPreferences: { sandbox: true, backgroundThrottling: false } });
  try {
    await win.loadURL(url);
    const run = script => win.webContents.executeJavaScript(script);
    const deadline = Date.now() + 6000;
    while (!await run('Boolean(document.querySelector(".turn-file-review"))')) {
      if (Date.now() > deadline) throw new Error('Summary did not render');
      await delay(20);
    }
    await delay(100);
    await writeFile('/tmp/lingxi-turn-files-light.png', (await win.webContents.capturePage()).toPNG());
    const result = await run(`({ files: document.querySelectorAll('.turn-file-summary li').length, initialDiffs: document.querySelectorAll('.turn-file-diffs h3').length })`);
    await run(`document.querySelector('.turn-file-review').click()`);
    await delay(60);
    result.openDiffs = await run(`document.querySelectorAll('.turn-file-diffs h3').length`);
    result.expanded = await run(`document.querySelector('.turn-file-review').getAttribute('aria-expanded')`);
    await run(`document.querySelector('.turn-file-review').click()`);
    await delay(60);
    result.closedDiffs = await run(`document.querySelectorAll('.turn-file-diffs h3').length`);
    win.setSize(360, 480);
    await delay(100);
    result.narrowOverflow = await run(`document.documentElement.scrollWidth > innerWidth`);
    await win.loadURL(`${url}?dark`);
    await delay(200);
    await writeFile('/tmp/lingxi-turn-files-dark.png', (await win.webContents.capturePage()).toPNG());
    process.stdout.write(`${JSON.stringify(result)}\n`);
  } finally { win.destroy(); app.quit(); }
}
main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); app.exit(1); });
