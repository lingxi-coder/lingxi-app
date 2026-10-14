import { app, BrowserWindow } from 'electron';
import assert from 'node:assert/strict';
import { writeFile } from 'node:fs/promises';

const url = process.argv.find((argument) => argument.startsWith('http://'));
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
app.commandLine.appendSwitch('disable-gpu');
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 900, height: 700, webPreferences: { sandbox: true } });
  const web = window.webContents;
  const js = (source) => web.executeJavaScript(source);
  const shown = `document.querySelector('.composer-slash-argument-hint')`;
  const prompt = `document.querySelector('[aria-label="Prompt"]')`;
  async function until(source) {
    for (let attempt = 0; attempt < 160; attempt++) { if (await js(source)) return; await delay(25); }
    throw new Error(`Timeout: ${source}`);
  }
  async function setDraft(text, offset = text.length) {
    await js(`(() => {
      const editor = ${prompt}; editor.textContent = ${JSON.stringify(text)}; editor.focus();
      const range = document.createRange(); range.setStart(editor.firstChild, ${offset}); range.collapse(true);
      const selection = getSelection(); selection.removeAllRanges(); selection.addRange(range);
      editor.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText' }));
    })()`);
    await delay(35);
  }
  async function press(keyCode) { web.sendInputEvent({ type: 'keyDown', keyCode }); web.sendInputEvent({ type: 'keyUp', keyCode }); await delay(40); }
  const hintText = () => js(`${shown}?.textContent`);
  const reviewHint = '[low|medium|high|xhigh|max|ultra] [--fix] [--comment] [<pr#>|<branch>|<path>] [--max-findings <n>|all]';
  try {
    await window.loadURL(`${url}?slash-hints`);
    await until(`Boolean(window.__composerDraftTest && ${prompt})`);
    await setDraft('/code-review ');
    await until(`Boolean(${shown})`);
    assert.equal(await hintText(), reviewHint);
    assert.equal(await js(`${prompt}.textContent`), '/code-review ');
    const geometry = await js(`(() => {
      const editor = ${prompt}, hint = ${shown};
      const end = document.createRange(); end.selectNodeContents(editor.firstChild); end.collapse(false);
      const caret = end.getBoundingClientRect(), box = hint.getBoundingClientRect();
      return { x: Math.abs(caret.x - box.x), y: Math.abs(caret.y - box.y), visible: getComputedStyle(hint).visibility,
        pointer: getComputedStyle(hint).pointerEvents, overflow: getComputedStyle(hint).textOverflow, truncated: hint.scrollWidth > hint.clientWidth };
    })()`);
    assert.ok(geometry.x < 1 && geometry.y < 1, JSON.stringify(geometry));
    assert.deepEqual({ ...geometry, x: 0, y: 0 }, { x: 0, y: 0, visible: 'visible', pointer: 'none', overflow: 'ellipsis', truncated: true });
    await writeFile('/tmp/lingxi-slash-argument-hint-desktop-dark.png', (await web.capturePage()).toPNG());
    await setDraft('/code-review ', 2);
    assert.equal(await hintText(), reviewHint);
    assert.equal(await js(`Boolean(document.querySelector('#slash-command-results'))`), false);
    await setDraft('/code-review ', 0);
    await until(`!${shown}`);
    await setDraft('/code-review  ');
    assert.equal(await hintText(), undefined);
    await setDraft('/code-review high ');
    assert.equal(await hintText(), undefined);
    await setDraft('/deploy "staging west" ');
    assert.equal(await hintText(), '[version] [region]');
    await setDraft('/deploy staging\nv1 ');
    assert.equal(await hintText(), '[version] [region]');
    await setDraft(`/deploy staging${'\nv1'.repeat(12)} `);
    await js(`${prompt}.scrollTop = ${prompt}.scrollHeight`);
    await until(`getComputedStyle(${shown}).visibility === 'visible'`);
    assert.equal(await hintText(), '[version] [region]');
    await setDraft('/raw ');
    assert.equal(await hintText(), '  <文件>\t[选项]  ');
    await js(`${prompt}.dispatchEvent(new CompositionEvent('compositionstart', { bubbles: true }))`);
    await until(`!${shown}`);
    await js(`${prompt}.dispatchEvent(new CompositionEvent('compositionend', { bubbles: true }))`);
    await until(`Boolean(${shown})`);
    await setDraft('/code-rev');
    await until(`Boolean(document.querySelector('#slash-command-results'))`);
    await press('Tab');
    await until(`${prompt}.textContent === '/code-review '`);
    assert.equal(await hintText(), reviewHint);
    // The hint is neither selectable/copyable source text nor a submission.
    const copied = await js(`(() => {
      const editor = ${prompt}; const range = document.createRange(); range.selectNodeContents(editor);
      const selection = getSelection(); selection.removeAllRanges(); selection.addRange(range);
      const data = new DataTransfer(); editor.dispatchEvent(new ClipboardEvent('copy', { bubbles: true, cancelable: true, clipboardData: data }));
      return data.getData('text/plain');
    })()`);
    assert.equal(copied, '/code-review ');
    await setDraft('/code-review ');
    await press('Right');
    assert.equal(await js(`${prompt}.textContent`), '/code-review ');
    assert.equal(await js(`window.__composerDraftTest.sendPending()`), false);
    await js(`window.__composerDraftTest.setTheme('light')`);
    window.setSize(480, 700);
    await delay(100);
    assert.equal(await js(`getComputedStyle(${shown}).visibility`), 'visible');
    await writeFile('/tmp/lingxi-slash-argument-hint-desktop-light-narrow.png', (await web.capturePage()).toPNG());
    await press('Enter');
    await until(`window.__composerDraftTest.slashCommands().length === 1`);
    assert.deepEqual(await js(`window.__composerDraftTest.slashCommands()`), ['/code-review']);
    console.log(JSON.stringify({ slashArgumentHints: true }));
    window.destroy(); app.exit(0);
  } catch (error) { console.error(error); window.destroy(); app.exit(1); }
}
void main().catch((error) => { console.error(error); app.exit(1); });
