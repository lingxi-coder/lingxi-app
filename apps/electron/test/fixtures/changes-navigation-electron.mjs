import assert from 'node:assert/strict';
import { app, BrowserWindow } from 'electron';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const url = process.argv.find(arg => arg.startsWith('http://'));
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
async function main() {
await app.whenReady();
const win = new BrowserWindow({ show: false, width: 1280, height: 850, webPreferences: { sandbox: true, backgroundThrottling: false } });
const errors = [];
win.webContents.on('console-message', details => { if (details.level === 'error' || details.level === 3) errors.push(details.message); });
const run = script => win.webContents.executeJavaScript(script);
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function wait(expression) {
  for (let i = 0; i < 250; i++) { if (await run(expression)) return; await delay(20); }
  throw new Error(`Timed out: ${expression}\n${await run('document.body.innerText')}`);
}
const click = selector => run(`document.querySelector(${JSON.stringify(selector)}).click()`);
const review = '.desktop-rail-changes';
const result = {};
try {
  await win.loadURL(url);
  await wait(`document.querySelector('${review} .desktop-rail-count')?.textContent === '1'`);
  assert.deepEqual(await run(`Array.from(document.querySelectorAll('.desktop-nav-rail button')).map(button => button.getAttribute('aria-label'))`), ['Home', 'Apps', 'Scheduled tasks', 'Review changes', 'Settings']);
  await run(`window.changesFixture.showSideChat()`);
  await wait(`document.querySelector('[aria-label="Reopen Side Chat"]')`);
  await click('[aria-label="Reopen Side Chat"]');
  await wait(`document.querySelector('.side-chat-panel')`);
  await run(`(() => { const editor = document.querySelector('.side-chat-panel textarea'); Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set.call(editor, 'Keep my side draft'); editor.dispatchEvent(new Event('input', { bubbles: true })); })()`);
  await run(`(() => { const editor = document.querySelector('.beta-rich-prompt'); editor.textContent = 'Keep my draft'; editor.dispatchEvent(new InputEvent('input', { bubbles: true })); })()`);
  await click(review);
  await wait(`document.querySelector('.desktop-changes-page .git-hunk')`);
  assert.equal(await run(`document.querySelector('${review}').getAttribute('aria-current') === 'page' && !document.querySelector('.desktop-nav-rail [aria-label="Home"]').hasAttribute('data-active') && !document.querySelector('.side-chat-panel')`), true);
  result.sideChatDoesNotHideReview = true;
  await click('.desktop-nav-rail [aria-label="Home"]');
  await wait(`document.querySelector('.side-chat-panel')`);
  assert.equal(await run(`document.querySelector('.beta-rich-prompt').textContent === 'Keep my draft'`), true);
  assert.equal(await run(`document.querySelector('.side-chat-panel textarea').value === 'Keep my side draft'`), true);
  result.chatStatePreserved = true;

  await win.loadURL(`${url}?scenario=draft`);
  await wait(`document.querySelector('${review} .desktop-rail-count')`);
  await click(review);
  await wait(`document.querySelector('.desktop-changes-page .git-hunk')`);
  assert.equal(await run(`window.changesFixture.scopes.every(scope => scope.sessionId === '__draft__')`), true);
  result.reviewWithoutSession = true;
  await run('window.changesFixture.clean()');
  await wait(`!document.querySelector('${review} .desktop-rail-count') && document.querySelector('.desktop-changes-page').textContent.includes('No file changes yet')`);
  result.liveCountAndCleanState = true;

  await win.loadURL(`${url}?scenario=empty`);
  await wait(`document.querySelector('${review}') && document.querySelector('.beta-rich-prompt')`);
  await click(review);
  await wait(`document.querySelector('.desktop-changes-page [role="status"]')?.textContent.includes('Select a project')`);
  assert.equal(await run('window.changesFixture.scopes.length'), 0);
  result.noProjectFeedback = true;

  await win.loadURL(`${url}?scenario=non-repository`);
  await wait(`document.querySelector('${review}') && window.changesFixture.scopes.length > 0`);
  await click(review);
  await wait(`document.querySelector('.desktop-changes-page .git-primary')`);
  result.nonRepositoryFeedback = true;

  const output = process.env.LINGXI_VISUAL_OUTPUT;
  if (output) {
    mkdirSync(output, { recursive: true });
    for (const theme of ['light', 'dark']) {
      await win.loadURL(`${url}?scenario=draft&theme=${theme}`);
      await wait(`document.querySelector('${review} .desktop-rail-count')`);
      await click(review);
      await wait(`document.querySelector('.desktop-changes-page .git-hunk')`);
      await delay(150);
      writeFileSync(join(output, `changes-${theme}.png`), (await win.webContents.capturePage()).toPNG());
    }
  }
  assert.deepEqual(errors, []);
  console.log(JSON.stringify(result));
  win.destroy(); app.exit(0);
} catch (error) { console.error(error); win.destroy(); app.exit(1); }
}
void main().catch(error => { console.error(error); app.exit(1); });
