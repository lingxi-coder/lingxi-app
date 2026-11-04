import { app, BrowserWindow } from 'electron';
import assert from 'node:assert/strict';
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
app.commandLine.appendSwitch('disable-gpu');
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function main() {
await app.whenReady();
const window = new BrowserWindow({ show: false, width: 900, height: 700, webPreferences: { sandbox: true } });
const web = window.webContents;
const js = source => web.executeJavaScript(source);
const value = () => js(`document.querySelector('[aria-label="Prompt"]').textContent`);
async function press(keyCode, modifiers = []) {
  web.sendInputEvent({ type: 'keyDown', keyCode, modifiers });
  web.sendInputEvent({ type: 'keyUp', keyCode, modifiers });
  await delay(50);
}
async function draft(text, offset = text.length) {
  await js(`(() => {
    const e = document.querySelector('[aria-label="Prompt"]');
    e.textContent = ${JSON.stringify(text)};
    e.dispatchEvent(new InputEvent('input', { bubbles: true }));
    e.focus();
    const range = document.createRange();
    range.selectNodeContents(e);
    ${text ? `range.setStart(e.firstChild, ${offset});` : ''}
    range.collapse(true);
    getSelection().removeAllRanges(); getSelection().addRange(range);
  })()`);
  await delay(50);
}
try {
  await window.loadURL(process.argv.find(arg => arg.startsWith('http://')));
  for (let i = 0; i < 160; i++) {
    if (await js(`Boolean(window.__composerDraftTest && document.querySelector('[aria-label="Prompt"]'))`)) break;
    await delay(25);
  }
  await js(`window.__composerDraftTest.setHistoryItems([
    { type:'narration', id:'u1', role:'user', text:'oldest prompt' },
    { type:'narration', id:'a1', role:'assistant', text:'never recall assistant' },
    { type:'narration', id:'u2', role:'user', text:'first line\\nsecond line' },
    { type:'narration', id:'u3', role:'user', text:'latest prompt' },
    { type:'narration', id:'u4', role:'user', text:'' }
  ])`);
  await draft('my draft', 0);
  await press('Up');
  assert.equal(await value(), 'my draft');
  await draft('');
  await press('Down');
  assert.equal(await value(), '');
  await press('Up', ['shift']);
  assert.equal(await value(), '');
  // IME arrows must not recall history.
  assert.equal(await js(`(() => { const e=document.querySelector('[aria-label="Prompt"]'); const event=new KeyboardEvent('keydown',{key:'ArrowUp',isComposing:true,bubbles:true,cancelable:true}); e.dispatchEvent(event); return event.defaultPrevented; })()`), false);
  await press('Up');
  assert.equal(await value(), 'latest prompt');
  await press('Up');
  assert.equal(await value(), 'first line\nsecond line');
  // Cursor movement inside recalled multiline text takes precedence.
  await js(`(() => { const e=document.querySelector('[aria-label="Prompt"]'); const r=document.createRange(); r.setStart(e.firstChild, 15); r.collapse(true); getSelection().removeAllRanges(); getSelection().addRange(r); })()`);
  await press('Up');
  assert.equal(await value(), 'first line\nsecond line');
  await js(`(() => { const e=document.querySelector('[aria-label="Prompt"]'); const r=document.createRange(); r.selectNodeContents(e); r.collapse(true); getSelection().removeAllRanges(); getSelection().addRange(r); })()`);
  await press('Up');
  assert.equal(await value(), 'oldest prompt');
  await press('Up');
  assert.equal(await value(), 'oldest prompt');
  await press('Down');
  assert.equal(await value(), 'first line\nsecond line');
  await press('Down');
  assert.equal(await value(), 'latest prompt');
  await press('Down');
  assert.equal(await value(), '');
  await press('Up');
  await web.insertText(' edited');
  await press('Up');
  assert.equal(await value(), 'latest prompt edited');
  await draft('');
  await press('Up');
  await js(`window.__composerDraftTest.switchSession('bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb')`);
  await delay(100);
  await draft('new session draft');
  await press('Down');
  assert.equal(await value(), 'new session draft');
  assert.equal(await js(`window.__composerDraftTest.sendPending()`), false);
  console.log(JSON.stringify({ history: true }));
  window.destroy(); app.exit(0);
} catch (error) {
  console.error(error); window.destroy(); app.exit(1);
}

}
void main().catch(error => { console.error(error); app.exit(1); });
