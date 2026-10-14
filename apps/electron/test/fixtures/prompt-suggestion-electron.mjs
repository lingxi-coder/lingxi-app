import { app, BrowserWindow } from 'electron';
import assert from 'node:assert/strict';
import { writeFile } from 'node:fs/promises';
const url = process.argv.find(a => a.startsWith('http://'));
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
app.commandLine.appendSwitch('disable-gpu');
const delay = ms => new Promise(r => setTimeout(r, ms));
async function main() {
await app.whenReady();
const window = new BrowserWindow({ show: false, width: 900, height: 700, webPreferences: { sandbox: true } });
const web = window.webContents;
const js = source => web.executeJavaScript(source);
async function until(source) { for (let i=0;i<160;i++) { if (await js(source)) return; await delay(25); } throw Error(`Timeout: ${source}`); }
const session = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';
const show = (text, turn) => js(`window.__composerDraftTest.setSuggestion(${JSON.stringify(text)},${turn},${JSON.stringify(session)})`);
const shown = `Boolean(document.querySelector('.composer-prompt-suggestion'))`;
async function setDraft(text) {
  await js(`(()=>{const e=document.querySelector('[aria-label="Prompt"]');e.textContent=${JSON.stringify(text)};e.dispatchEvent(new InputEvent('input',{bubbles:true,inputType:'insertText'}));e.focus();})()`);
}
async function press(keyCode) { web.sendInputEvent({ type:'keyDown',keyCode }); web.sendInputEvent({ type:'keyUp',keyCode }); await delay(40); }
try {
  await window.loadURL(url);
  console.error('suggestion fixture: loaded');
  await until(`Boolean(window.__composerDraftTest && document.querySelector('[aria-label="Prompt"]'))`);
  await show('运行测试', 1);
  await until(shown);
  assert.equal(await js(`document.querySelector('[aria-label="Prompt"]').textContent`), '');
  await js(`document.querySelector('[aria-label="Prompt"]').focus()`);
  await press('Enter');
  assert.equal(await js(`window.__composerDraftTest.sendPending()`), false);
  await press('Tab');
  await until(`document.querySelector('[aria-label="Prompt"]').textContent === '运行测试'`);
  assert.equal(await js(`window.__composerDraftTest.sendPending()`), false);
  await setDraft('');
  await until(shown);
  await show('提交修改', 2);
  await until(shown);
  await press('Right');
  await until(`document.querySelector('[aria-label="Prompt"]').textContent === '提交修改'`);
  await setDraft('');
  await show('再次运行', 3);
  await until(shown);
  await web.insertText('my own draft');
  await until(`!${shown}`);
  await setDraft('');
  await until(shown);
  assert.equal(await js(`window.__composerDraftTest.suggestionFeedback().filter(e=>e.uuid==='suggestion-3' && e.action==='shown').length`), 1);
  await press('Escape');
  assert.equal(await js(shown), true);
  // Clearing the draft keeps a shown suggestion available. Cancellation of
  // pending generation belongs to the host, driven by composer context.
  await show('', 4);
  await setDraft('typed first');
  await show('late suggestion', 4);
  await until(`window.__composerDraftTest.suggestionFeedback().some(e=>e.uuid==='suggestion-4' && e.action==='dismissed')`);
  await setDraft('');
  assert.equal(await js(shown), false);
  await until(`window.__composerDraftTest.suggestionContexts().some(c=>!c.input_empty)`);
  assert.equal(await js(`window.__composerDraftTest.suggestionFeedback().filter(e=>e.action==='accepted').length`), 2);
  await show('检查差异', 5);
  await until(shown);
  await js(`window.__composerDraftTest.setSuggestionState(false, 'Showing fewer prompt suggestions · use one to bring them back')`);
  assert.equal(await js(shown), true);
  await until(`document.querySelector('.composer-prompt-suggestion-notice')?.textContent === 'Showing fewer prompt suggestions · use one to bring them back'`);
  await js(`window.__composerDraftTest.setSuggestionState(true, null)`);
  await until(shown);
  assert.equal(await js(`window.__composerDraftTest.suggestionFeedback().filter(e=>e.uuid==='suggestion-5' && e.action==='shown').length`), 1);
  await js(`window.__composerDraftTest.switchSession('bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb')`);
  await until(`!${shown}`);
  await js(`window.__composerDraftTest.switchSession(${JSON.stringify(session)})`);
  await until(shown);
  await writeFile('/tmp/lingxi-prompt-suggestion-desktop.png', (await web.capturePage()).toPNG());
  await js(`document.querySelector('.composer-prompt-suggestion').click()`);
  await until(`document.querySelector('[aria-label="Prompt"]').textContent === '检查差异'`);
  console.log(JSON.stringify({ suggestions:true }));
  window.destroy(); app.exit(0);
} catch(error) { console.error(error); window.destroy(); app.exit(1); }

}
void main().catch(error => { console.error(error); app.exit(1); });
