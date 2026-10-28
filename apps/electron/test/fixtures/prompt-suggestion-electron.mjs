import { app, BrowserWindow } from 'electron';
import assert from 'node:assert/strict';
import { writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
const url = process.argv.find(a => a.startsWith('http://'));
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
app.commandLine.appendSwitch('disable-gpu');
const delay = ms => new Promise(r => setTimeout(r, ms));
async function main() {
await app.whenReady();
const window = new BrowserWindow({ show: false, width: 900, height: 700, webPreferences: { sandbox: true } });
const web = window.webContents;
const js = source => web.executeJavaScript(source);
async function until(source) {
  for (let i=0;i<160;i++) { if (await js(source)) return; await delay(25); }
  const state = await js(`({
    draft: document.querySelector('[aria-label="Prompt"]')?.textContent,
    suggestion: document.querySelector('.composer-prompt-suggestion span')?.textContent,
    focus: document.activeElement?.getAttribute('aria-label'),
    keys: window.__suggestionTestKeys,
    feedback: window.__composerDraftTest?.suggestionFeedback(),
  })`);
  throw Error(`Timeout: ${source}\nState: ${JSON.stringify(state)}`);
}
const session = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';
const show = (text, turn) => js(`window.__composerDraftTest.setSuggestion(${JSON.stringify(text)},${turn},${JSON.stringify(session)})`);
const shown = `Boolean(document.querySelector('.composer-prompt-suggestion'))`;
async function waitForSuggestion(text, turn) {
  await until(`document.querySelector('.composer-prompt-suggestion span')?.textContent === ${JSON.stringify(text)}
    && window.__composerDraftTest.suggestionFeedback().some(e => e.uuid === 'suggestion-${turn}' && e.action === 'shown')`);
}
async function setDraft(text) {
  await js(`(()=>{const e=document.querySelector('[aria-label="Prompt"]');e.textContent=${JSON.stringify(text)};e.dispatchEvent(new InputEvent('input',{bubbles:true,inputType:'insertText'}));e.focus();})()`);
}
async function press(keyCode) { web.sendInputEvent({ type:'keyDown',keyCode }); web.sendInputEvent({ type:'keyUp',keyCode }); await delay(40); }
try {
  await window.loadURL(url);
  console.error('suggestion fixture: loaded');
  await until(`Boolean(window.__composerDraftTest && document.querySelector('[aria-label="Prompt"]'))`);
  await js(`window.__suggestionTestKeys = []; document.addEventListener('keydown', event => {
    window.__suggestionTestKeys.push({ key: event.key, target: event.target instanceof Element ? event.target.getAttribute('aria-label') : null });
  }, true)`);
  await show('运行测试', 1);
  await waitForSuggestion('运行测试', 1);
  assert.equal(await js(`document.querySelector('[aria-label="Prompt"]').textContent`), '');
  await js(`document.querySelector('[aria-label="Prompt"]').focus()`);
  await press('Enter');
  assert.equal(await js(`window.__composerDraftTest.sendPending()`), false);
  await press('Tab');
  await until(`document.querySelector('[aria-label="Prompt"]').textContent === '运行测试'`);
  assert.equal(await js(`window.__composerDraftTest.sendPending()`), false);
  await setDraft('');
  await waitForSuggestion('运行测试', 1);
  await show('提交修改', 2);
  // The previous suggestion is still visible until React commits the new one.
  await waitForSuggestion('提交修改', 2);
  await press('Right');
  await until(`document.querySelector('[aria-label="Prompt"]').textContent === '提交修改'`);
  await setDraft('');
  await waitForSuggestion('提交修改', 2);
  await show('再次运行', 3);
  await waitForSuggestion('再次运行', 3);
  await web.insertText('my own draft');
  await until(`!${shown}`);
  const contextsBeforeClear = await js(`window.__composerDraftTest.suggestionContexts().length`);
  await setDraft('');
  await until(`window.__composerDraftTest.suggestionContexts().slice(${contextsBeforeClear}).some(c => c.session_id === ${JSON.stringify(session)} && c.input_empty === true)`);
  await waitForSuggestion('再次运行', 3);
  assert.equal(await js(`window.__composerDraftTest.suggestionFeedback().filter(e=>e.uuid==='suggestion-3' && e.action==='shown').length`), 1);
  await press('Escape');
  assert.equal(await js(shown), true);
  // Clearing the draft keeps a shown suggestion available. Cancellation of
  // pending generation belongs to the host, driven by composer context.
  await show('', 4);
  await until(`!${shown}`);
  // The fixture's context callback is stable, so no session cleanup can
  // produce a false context between this boundary and the next typed update.
  const contextsBeforeTyping = await js(`window.__composerDraftTest.suggestionContexts().length`);
  await setDraft('typed first');
  await until(`document.querySelector('.composer-send-presence')?.dataset.visible === 'true'
    && window.__composerDraftTest.suggestionContexts().slice(${contextsBeforeTyping}).some(c => c.session_id === ${JSON.stringify(session)} && c.input_empty === false)`);
  await show('late suggestion', 4);
  await until(`window.__composerDraftTest.suggestionFeedback().some(e=>e.uuid==='suggestion-4' && e.action==='dismissed')`);
  const contextsBeforeSecondClear = await js(`window.__composerDraftTest.suggestionContexts().length`);
  await setDraft('');
  await until(`document.querySelector('[aria-label="Prompt"]').textContent === ''
    && document.querySelector('.composer-send-presence')?.dataset.visible === 'false'
    && window.__composerDraftTest.suggestionContexts().slice(${contextsBeforeSecondClear}).some(c => c.session_id === ${JSON.stringify(session)} && c.input_empty === true)`);
  assert.equal(await js(shown), false);
  await until(`window.__composerDraftTest.suggestionContexts().some(c=>!c.input_empty)`);
  assert.equal(await js(`window.__composerDraftTest.suggestionFeedback().filter(e=>e.action==='accepted').length`), 2);
  assert.deepEqual(await js(`window.__composerDraftTest.suggestionFeedback().filter(e=>e.action==='accepted').map(e=>e.uuid)`), ['suggestion-1', 'suggestion-2']);
  await show('检查差异', 5);
  await waitForSuggestion('检查差异', 5);
  await js(`window.__composerDraftTest.setSuggestionState(false, 'Showing fewer prompt suggestions · use one to bring them back')`);
  assert.equal(await js(shown), true);
  await until(`document.querySelector('.composer-prompt-suggestion-notice')?.textContent === 'Showing fewer prompt suggestions · use one to bring them back'`);
  await js(`window.__composerDraftTest.setSuggestionState(true, null)`);
  await waitForSuggestion('检查差异', 5);
  assert.equal(await js(`window.__composerDraftTest.suggestionFeedback().filter(e=>e.uuid==='suggestion-5' && e.action==='shown').length`), 1);
  await js(`window.__composerDraftTest.switchSession('bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb')`);
  await until(`!${shown}`);
  await js(`window.__composerDraftTest.switchSession(${JSON.stringify(session)})`);
  await waitForSuggestion('检查差异', 5);
  await writeFile(join(tmpdir(), 'lingxi-prompt-suggestion-desktop.png'), (await web.capturePage()).toPNG());
  await js(`document.querySelector('.composer-prompt-suggestion').click()`);
  await until(`document.querySelector('[aria-label="Prompt"]').textContent === '检查差异'`);
  console.log(JSON.stringify({ suggestions:true }));
  window.destroy(); app.exit(0);
} catch(error) { console.error(error); window.destroy(); app.exit(1); }

}
void main().catch(error => { console.error(error); app.exit(1); });
