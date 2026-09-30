import { app, BrowserWindow } from 'electron';
import { writeFileSync } from 'node:fs';
const url = process.argv.find(value => value.startsWith('http://'));
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function main() {
await app.whenReady();
const window = new BrowserWindow({ show: false, width: 1000, height: 800, webPreferences: { sandbox: true } });
try {
  await window.loadURL(url);
  const run = script => window.webContents.executeJavaScript(script);
  const wait = async expression => {
    const deadline = Date.now() + 6000;
    while (Date.now() < deadline) { if (await run(expression)) return; await delay(20); }
    throw new Error(`Timed out: ${expression}\n${await run('document.body.innerText')}`);
  };
  await wait('window.firstSessionFixture?.ready');
  const selector = '[data-session-id="new-session"]';
  const result = { emptyDraftHidden: await run(`!document.querySelector('${selector}')`) };
  await run(`document.querySelector('button[title="/fixture"]').click()`);
  await run(`document.querySelector('#send').click()`);
  await wait(`document.querySelector('${selector}') === document.activeElement`);
  result.immediateFocus = await run(`document.activeElement.getAttribute('aria-current') === 'page' && document.activeElement.textContent.includes('First message appears immediately')`);
  await run('window.firstSessionFixture.refresh()');
  result.staleRefresh = await run(`document.querySelectorAll('${selector}').length === 1`);
  writeFileSync('/tmp/first-session-sidebar.png', (await window.webContents.capturePage()).toPNG());
  await run(`document.querySelector('#composer').focus()`);
  await run('window.firstSessionFixture.persist()');
  await wait(`document.querySelector('${selector}')?.textContent.includes('Saved title')`);
  result.savedOnce = await run(`document.querySelectorAll('${selector}').length === 1`);
  result.focusNotStolen = await run(`document.activeElement.id === 'composer'`);
  result.beyondFiveVisible = await run(`document.querySelector('${selector}').getAttribute('aria-current') === 'page'`);
  console.log(JSON.stringify(result));
} catch (error) { console.error(error); process.exitCode = 1; }
finally { window.destroy(); app.quit(); }

}
void main().catch(error => { console.error(error); app.exit(1); });
