import { app, BrowserWindow } from 'electron';
import { writeFile } from 'node:fs/promises';
const url = process.argv.find(arg => arg.startsWith('http://'));
if (!url || !process.env.LINGXI_TEST_USER_DATA) throw new Error('fixture URL and isolated user data required');
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function main() {
  await app.whenReady();
  const win = new BrowserWindow({ show: false, width: 1300, height: 700, webPreferences: { sandbox: true, backgroundThrottling: false } });
  try {
    const run = script => win.webContents.executeJavaScript(script);
    const waitFor = async script => {
      const deadline = Date.now() + 6000;
      while (!await run(script)) {
        if (Date.now() > deadline) throw new Error(`Fixture condition timed out: ${script}`);
        await delay(20);
      }
    };
    await win.loadURL(url);
    await waitFor('Boolean(document.querySelector(".turn-file-review"))');
    const result = await run(`({ files: document.querySelectorAll('.turn-file-summary li').length, initiallyClosed: !document.querySelector('#runtime-inspector'), inlineDiffs: document.querySelectorAll('main .turn-file-diffs').length })`);
    await run(`document.querySelector('.turn-file-review').click()`);
    await waitFor(`document.querySelector('#runtime-inspector-panel')?.textContent.includes('Historical stylesheet change')`);
    await waitFor(`document.querySelector('#runtime-inspector').getAnimations().length === 0`);
    result.allFilesVisible = await run(`['Historical component change', 'Historical stylesheet change'].every(text => document.querySelector('#runtime-inspector-panel').textContent.includes(text))`);
    result.rightPanel = await run(`document.querySelector('#runtime-inspector').getBoundingClientRect().left >= document.querySelector('main').getBoundingClientRect().right - 1`);
    await writeFile('/tmp/lingxi-turn-files-light.png', (await win.webContents.capturePage()).toPNG());
    await run(`document.querySelectorAll('.turn-file-path')[1].click()`);
    await waitFor(`!document.querySelector('#runtime-inspector-panel')?.textContent.includes('Historical component change')`);
    result.selectedFileOnly = await run(`document.querySelector('#runtime-inspector-panel').textContent.includes('Historical stylesheet change') && !document.querySelector('#runtime-inspector-panel').textContent.includes('Historical component change')`);
    await run(`document.querySelector('[role="tab"]').click()`);
    await delay(30);
    result.selectedFileSurvivesTab = await run(`!document.querySelector('#runtime-inspector-panel').textContent.includes('Historical component change')`);
    await run(`document.querySelectorAll('.turn-file-path')[1].click(); document.querySelector('.turn-file-review').click()`);
    await waitFor(`document.querySelector('#runtime-inspector-panel')?.textContent.includes('Historical component change')`);
    result.reviewResetsAll = await run(`['Historical component change', 'Historical stylesheet change'].every(text => document.querySelector('#runtime-inspector-panel').textContent.includes(text))`);
    result.tabCount = await run(`document.querySelectorAll('.runtime-inspector-tabs [role="tab"]').length`);
    await run(`document.querySelector('[aria-label="Hide right panel"]').click()`);
    await waitFor(`!document.querySelector('#runtime-inspector')`);
    result.closedWithoutInlineDiffs = await run(`!document.querySelector('#runtime-inspector') && !document.querySelector('main .turn-file-diffs')`);
    win.setSize(360, 480);
    await win.loadURL(`${url}?dark`);
    await waitFor('Boolean(document.querySelector(".turn-file-review"))');
    await run(`document.querySelector('.turn-file-review').click()`);
    await waitFor(`document.querySelector('#runtime-inspector-panel')?.textContent.includes('Historical stylesheet change')`);
    await waitFor(`document.querySelector('#runtime-inspector').getAnimations().length === 0`);
    result.darkNarrowVisible = await run(`document.querySelector('#runtime-inspector').getBoundingClientRect().right <= innerWidth + 1 && document.querySelector('#runtime-inspector').getBoundingClientRect().left >= -1`);
    result.darkHeadingMatchesTheme = await run(`getComputedStyle(document.querySelector('.turn-file-diffs h3')).color === getComputedStyle(document.querySelector('.turn-file-summary-heading strong')).color`);
    result.narrowOverflow = await run(`document.documentElement.scrollWidth > innerWidth`);
    await writeFile('/tmp/lingxi-turn-files-dark.png', (await win.webContents.capturePage()).toPNG());
    process.stdout.write(`${JSON.stringify(result)}\n`);
  } finally { win.destroy(); app.quit(); }
}
main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); app.exit(1); });
