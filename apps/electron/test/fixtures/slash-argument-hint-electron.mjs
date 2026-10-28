import { app, BrowserWindow } from 'electron';
import assert from 'node:assert/strict';
import { writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

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
  const mirror = `document.querySelector('.composer-command-mirror')`;
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
    // The editor is changed synchronously, but the command mirror reflects
    // the draft React actually committed. Do not inspect a previous draft.
    await until(`${mirror}?.textContent === ${JSON.stringify(text)}`);
  }
  async function press(keyCode) { web.sendInputEvent({ type: 'keyDown', keyCode }); web.sendInputEvent({ type: 'keyUp', keyCode }); await delay(40); }
  async function holdAnimationFrames() {
    await js(`(() => {
      const request = window.requestAnimationFrame, cancel = window.cancelAnimationFrame;
      const pending = new Map(); let nextId = -1;
      window.requestAnimationFrame = callback => { const id = nextId--; pending.set(id, callback); return id; };
      window.cancelAnimationFrame = id => { if (id < 0) pending.delete(id); else cancel.call(window, id); };
      window.__releaseComposerFrames = () => {
        window.requestAnimationFrame = request; window.cancelAnimationFrame = cancel;
        const callbacks = [...pending.values()]; pending.clear();
        callbacks.forEach(callback => callback(performance.now()));
        delete window.__releaseComposerFrames;
        return callbacks.length;
      };
    })()`);
  }
  const selectionOffset = () => js(`(() => {
    const editor = ${prompt}, selection = getSelection();
    if (!selection?.isCollapsed || !selection.focusNode || !editor.contains(selection.focusNode)) return null;
    const prefix = document.createRange(); prefix.selectNodeContents(editor);
    prefix.setEnd(selection.focusNode, selection.focusOffset);
    return prefix.toString().length;
  })()`);
  const hintText = () => js(`${shown}?.textContent`);
  const waitForHint = (text) => until(`${shown}?.textContent === ${JSON.stringify(text)}`);
  const hintGeometry = () => js(`(() => {
    const editor = ${prompt}, hint = ${shown};
    const walker = document.createTreeWalker(editor, NodeFilter.SHOW_TEXT);
    let lastText = null;
    while (walker.nextNode()) if (walker.currentNode.textContent.length) lastText = walker.currentNode;
    const end = document.createRange(); end.selectNodeContents(lastText); end.collapse(false);
    const caret = end.getBoundingClientRect(), box = hint.getBoundingClientRect();
    return { x: Math.abs(caret.x - box.x), y: Math.abs(caret.y - box.y), visible: getComputedStyle(hint).visibility,
      pointer: getComputedStyle(hint).pointerEvents, overflow: getComputedStyle(hint).textOverflow, truncated: hint.scrollWidth > hint.clientWidth };
  })()`);
  const hintWidthMatchesEditor = `(() => {
    const editor = ${prompt}, hint = ${shown};
    if (!hint) return false;
    const end = document.createRange(); end.selectNodeContents(editor.firstChild); end.collapse(false);
    const available = editor.clientWidth - parseFloat(getComputedStyle(editor).paddingRight)
      - (end.getBoundingClientRect().left - editor.getBoundingClientRect().left);
    return Math.abs(hint.clientWidth - Math.max(0, available)) < 1;
  })()`;
  const reviewHint = '[low|medium|high|xhigh|max|ultra] [--fix] [--comment] [<pr#>|<branch>|<path>] [--max-findings <n>|all]';
  try {
    await window.loadURL(`${url}?slash-hints`);
    await until(`Boolean(window.__composerDraftTest && ${prompt})`);
    await setDraft('/code-review ');
    await waitForHint(reviewHint);
    assert.equal(await hintText(), reviewHint);
    assert.equal(await js(`${prompt}.textContent`), '/code-review ');
    const geometry = await hintGeometry();
    assert.ok(geometry.x < 1 && geometry.y < 1, JSON.stringify(geometry));
    assert.deepEqual({ visible: geometry.visible, pointer: geometry.pointer, overflow: geometry.overflow },
      { visible: 'visible', pointer: 'none', overflow: 'ellipsis' });
    const darkHintColor = await js(`getComputedStyle(${shown}).color`);
    await writeFile(join(tmpdir(), 'lingxi-slash-argument-hint-desktop-dark.png'), (await web.capturePage()).toPNG());
    const wideInnerWidth = await js('window.innerWidth');

    // Installed font metrics determine whether this text overflows at 900px.
    // Use an actual narrow window to exercise ellipsis on every platform.
    window.setSize(360, 700);
    await until(`window.innerWidth <= 480 && ${hintWidthMatchesEditor}`);
    const narrowGeometry = await hintGeometry();
    assert.ok(narrowGeometry.x < 1 && narrowGeometry.y < 1, JSON.stringify(narrowGeometry));
    assert.deepEqual({ ...narrowGeometry, x: 0, y: 0 },
      { x: 0, y: 0, visible: 'visible', pointer: 'none', overflow: 'ellipsis', truncated: true });
    assert.equal(await hintText(), reviewHint);
    assert.equal(await js(`${prompt}.textContent`), '/code-review ');
    window.setSize(900, 700);
    await until(`window.innerWidth === ${wideInnerWidth} && ${hintWidthMatchesEditor}`);
    const restoredGeometry = await hintGeometry();
    assert.ok(restoredGeometry.x < 1 && restoredGeometry.y < 1, JSON.stringify(restoredGeometry));
    assert.deepEqual({ visible: restoredGeometry.visible, pointer: restoredGeometry.pointer, overflow: restoredGeometry.overflow },
      { visible: 'visible', pointer: 'none', overflow: 'ellipsis' });
    // Start from a committed zero cursor so a visible hint proves that the
    // interior cursor position was observed by the composer.
    await setDraft('/code-review ', 0);
    await until(`!${shown}`);
    await setDraft('/code-review ', 2);
    await waitForHint(reviewHint);
    assert.equal(await hintText(), reviewHint);
    assert.equal(await js(`Boolean(document.querySelector('#slash-command-results'))`), false);
    await setDraft('/code-review ', 0);
    await until(`!${shown}`);
    // Re-establish a nonzero committed cursor before checking invalid drafts.
    await setDraft('/code-review ');
    await waitForHint(reviewHint);
    await setDraft('/code-review  ');
    assert.equal(await hintText(), undefined);
    await setDraft('/code-review high ');
    assert.equal(await hintText(), undefined);
    await setDraft('/deploy "staging west" ');
    await waitForHint('[version] [region]');
    assert.equal(await hintText(), '[version] [region]');
    await setDraft('/deploy staging\nv1 ');
    await waitForHint('[version] [region]');
    assert.equal(await hintText(), '[version] [region]');
    await setDraft(`/deploy staging${'\nv1'.repeat(12)} `);
    await waitForHint('[version] [region]');
    await js(`${prompt}.scrollTop = ${prompt}.scrollHeight`);
    await until(`(() => {
      const editor = ${prompt}, hint = ${shown};
      return editor.scrollTop > 0 && hint?.textContent === '[version] [region]'
        && getComputedStyle(hint).visibility === 'visible';
    })()`);
    assert.equal(await hintText(), '[version] [region]');
    await setDraft('/raw ');
    await waitForHint('  <文件>\t[选项]  ');
    assert.equal(await hintText(), '  <文件>\t[选项]  ');
    await js(`${prompt}.dispatchEvent(new CompositionEvent('compositionstart', { bubbles: true }))`);
    await until(`!${shown}`);
    await js(`${prompt}.dispatchEvent(new CompositionEvent('compositionend', { bubbles: true }))`);
    await waitForHint('  <文件>\t[选项]  ');
    await setDraft('/code-rev');
    await until(`Boolean(document.querySelector('#slash-command-results'))`);
    // A backgrounded window can defer animation frames until after another
    // edit. Hold the pending frames until that edit has replaced the text.
    await holdAnimationFrames();
    await press('Tab');
    await until(`${prompt}.textContent === '/code-review '`);
    await waitForHint(reviewHint);
    assert.equal(await hintText(), reviewHint);
    // Range.insertNode used by Tab completion can leave an empty final text
    // node. The hint must still be visible at the end of the inserted command.
    await until(`${shown} && getComputedStyle(${shown}).visibility === 'visible'`);
    const completedGeometry = await hintGeometry();
    assert.ok(completedGeometry.x < 1 && completedGeometry.y < 1, JSON.stringify(completedGeometry));
    // The hint is neither selectable/copyable source text nor a submission.
    const copied = await js(`(() => {
      const editor = ${prompt}; const range = document.createRange(); range.selectNodeContents(editor);
      const selection = getSelection(); selection.removeAllRanges(); selection.addRange(range);
      const data = new DataTransfer(); editor.dispatchEvent(new ClipboardEvent('copy', { bubbles: true, cancelable: true, clipboardData: data }));
      return data.getData('text/plain');
    })()`);
    assert.equal(copied, '/code-review ');
    await setDraft('/code-review ');
    await waitForHint(reviewHint);
    await js(`window.__releaseComposerFrames()`);
    assert.equal(await selectionOffset(), 13, 'a deferred completion must not move a later edit to the start');
    await waitForHint(reviewHint);
    await press('Right');
    assert.equal(await js(`${prompt}.textContent`), '/code-review ');
    assert.equal(await js(`window.__composerDraftTest.sendPending()`), false);
    // Cursor movement after completion is also newer than a deferred focus.
    await setDraft('/code-rev');
    await until(`Boolean(document.querySelector('#slash-command-results'))`);
    await holdAnimationFrames();
    await press('Tab');
    await waitForHint(reviewHint);
    await press('Left');
    const movedOffset = await selectionOffset();
    assert.equal(movedOffset, 12);
    await js(`window.__releaseComposerFrames()`);
    assert.equal(await selectionOffset(), movedOffset, 'a deferred completion must preserve a later caret move');
    await setDraft('/code-review ');
    await waitForHint(reviewHint);
    await js(`window.__composerDraftTest.setTheme('light')`);
    window.setSize(480, 700);
    await until(`(() => {
      const hint = ${shown};
      return window.innerWidth <= 480 && hint?.textContent === ${JSON.stringify(reviewHint)}
        && getComputedStyle(hint).visibility === 'visible'
        && getComputedStyle(hint).color !== ${JSON.stringify(darkHintColor)}
        && ${hintWidthMatchesEditor};
    })()`);
    assert.equal(await js(`getComputedStyle(${shown}).visibility`), 'visible');
    await writeFile(join(tmpdir(), 'lingxi-slash-argument-hint-desktop-light-narrow.png'), (await web.capturePage()).toPNG());
    await press('Enter');
    await until(`window.__composerDraftTest.slashCommands().length === 1`);
    assert.deepEqual(await js(`window.__composerDraftTest.slashCommands()`), ['/code-review']);
    console.log(JSON.stringify({ slashArgumentHints: true }));
    window.destroy(); app.exit(0);
  } catch (error) { console.error(error); window.destroy(); app.exit(1); }
}
void main().catch((error) => { console.error(error); app.exit(1); });
