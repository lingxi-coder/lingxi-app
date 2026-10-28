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
  const reviewHint = '[low|medium|high|xhigh|max|ultra] [--fix] [--comment] [<pr#>|<branch>|<path>] [--max-findings <n>|all]';
  let darkHintColor = null;
  async function diagnosticSnapshot() {
    const renderer = await js(`(() => {
      const editor = ${prompt}, hint = ${shown};
      const rect = (element) => {
        if (!element) return null;
        const box = element.getBoundingClientRect();
        return { x: box.x, y: box.y, width: box.width, height: box.height };
      };
      const endOf = (node) => {
        if (!node) return null;
        const range = document.createRange();
        range.selectNodeContents(node); range.collapse(false);
        const box = range.getBoundingClientRect();
        return { x: box.x, y: box.y, height: box.height };
      };
      let last = editor;
      while (last?.lastChild) last = last.lastChild;
      const firstEnd = endOf(editor?.firstChild), lastEnd = endOf(last);
      const editorRect = rect(editor), hintRect = rect(hint);
      const editorStyle = editor && getComputedStyle(editor), hintStyle = hint && getComputedStyle(hint);
      const available = firstEnd && editorRect
        ? Math.max(0, editor.clientWidth - parseFloat(editorStyle.paddingRight) - (firstEnd.x - editorRect.x)) : null;
      const selection = getSelection();
      return {
        viewport: { innerWidth, innerHeight, devicePixelRatio },
        lightNarrowChecks: {
          narrow: innerWidth <= 480,
          expectedText: hint?.textContent === ${JSON.stringify(reviewHint)},
          visible: hintStyle?.visibility === 'visible',
          changedColor: Boolean(hintStyle && ${JSON.stringify(darkHintColor)} !== null && hintStyle.color !== ${JSON.stringify(darkHintColor)}),
          widthMatches: Boolean(hint && available !== null && Math.abs(hint.clientWidth - available) < 1),
        },
        editor: editor && { text: editor.textContent, rect: editorRect, clientWidth: editor.clientWidth,
          paddingRight: editorStyle.paddingRight, scrollTop: editor.scrollTop, scrollHeight: editor.scrollHeight,
          children: Array.from(editor.childNodes, (node) => ({ name: node.nodeName, length: node.textContent?.length })),
          firstEnd, lastEnd, expectedHintWidth: available },
        hint: hint && { text: hint.textContent, rect: hintRect, clientWidth: hint.clientWidth,
          scrollWidth: hint.scrollWidth, visibility: hintStyle.visibility, color: hintStyle.color,
          pointerEvents: hintStyle.pointerEvents, textOverflow: hintStyle.textOverflow,
          widthDifference: available === null ? null : hint.clientWidth - available },
        selection: selection && { collapsed: selection.isCollapsed, focusOffset: selection.focusOffset,
          focusInEditor: Boolean(editor && selection.focusNode && editor.contains(selection.focusNode)) },
      };
    })()`);
    return { windowSize: window.getSize(), contentSize: window.getContentSize(), darkHintColor, renderer };
  }
  async function until(source) {
    for (let attempt = 0; attempt < 160; attempt++) { if (await js(source)) return; await delay(25); }
    let state;
    try { state = await diagnosticSnapshot(); }
    catch (error) { state = { snapshotError: String(error) }; }
    throw new Error(`Timeout: ${source}\nstate=${JSON.stringify(state)}`);
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
  const hintText = () => js(`${shown}?.textContent`);
  const waitForHint = (text) => until(`${shown}?.textContent === ${JSON.stringify(text)}`);
  const hintGeometry = () => js(`(() => {
    const editor = ${prompt}, hint = ${shown};
    const end = document.createRange(); end.selectNodeContents(editor.firstChild); end.collapse(false);
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
    darkHintColor = await js(`getComputedStyle(${shown}).color`);
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
    await press('Tab');
    await until(`${prompt}.textContent === '/code-review '`);
    await waitForHint(reviewHint);
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
    await waitForHint(reviewHint);
    await press('Right');
    assert.equal(await js(`${prompt}.textContent`), '/code-review ');
    assert.equal(await js(`window.__composerDraftTest.sendPending()`), false);
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
