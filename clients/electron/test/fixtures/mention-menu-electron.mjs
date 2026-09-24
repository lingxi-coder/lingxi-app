import { app, BrowserWindow } from 'electron';
import assert from 'node:assert/strict';
import { writeFile } from 'node:fs/promises';

app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
app.commandLine.appendSwitch('disable-gpu');
const url = process.argv.find((value) => value.startsWith('http://'));
const delay = (ms) => new Promise((done) => setTimeout(done, ms));
async function main() {
await app.whenReady();
const window = new BrowserWindow({ show: false, width: 1000, height: 780, webPreferences: { sandbox: true } });
const page = window.webContents;
const evaluate = async (expression) => { try { return await page.executeJavaScript(expression); } catch (cause) { throw new Error(`Renderer expression failed: ${expression}`, { cause }); } };
async function wait(expression) {
  for (let i = 0; i < 160; i += 1) { if (await evaluate(expression)) return; await delay(25); }
  throw new Error(`Timed out: ${expression}\n${await evaluate('document.body.innerText')}`);
}
async function prompt(text) {
  await evaluate(`(() => { const editor = document.querySelector('[aria-label="Prompt"]'); editor.textContent = ${JSON.stringify(text)}; editor.focus(); const range = document.createRange(); if (editor.firstChild) range.setStart(editor.firstChild, editor.firstChild.textContent.length); else range.selectNodeContents(editor); range.collapse(false); getSelection().removeAllRanges(); getSelection().addRange(range); editor.dispatchEvent(new InputEvent('input', { bubbles: true })); })()`);
}
async function key(key, composing = false) {
  await evaluate(`(() => { const editor = document.activeElement; editor.dispatchEvent(new KeyboardEvent('keydown', { key: ${JSON.stringify(key)}, bubbles: true, isComposing: ${composing} })); editor.dispatchEvent(new KeyboardEvent('keyup', { key: ${JSON.stringify(key)}, bubbles: true, isComposing: ${composing} })); })()`);
}
const selected = () => evaluate('document.querySelector("#mention-results [aria-selected=true]")?.textContent');
const menuMetrics = async () => { await delay(170); return evaluate(`(() => { const menu = document.querySelector('.slash-command-menu'); const style = getComputedStyle(menu); const box = menu.getBoundingClientRect(); const row = getComputedStyle(menu.querySelector('[role="option"]')); return { x: Math.round(box.x), width: Math.round(box.width), bottom: style.bottom, radius: style.borderRadius, rowHeight: row.minHeight, rowColumns: row.gridTemplateColumns.split(' ').filter((_, index) => index !== 1) }; })()`); };
try {
  await window.loadURL(url);
  await wait('Boolean(window.__composerDraftTest)');
  await prompt('/');
  await wait('Boolean(document.querySelector("#slash-command-results"))');
  const slash = await menuMetrics();
  await prompt('@');
  await wait('document.querySelectorAll("#mention-results [role=option]").length === 7');
  assert.deepEqual(await menuMetrics(), slash);
  assert.match(await selected(), /Files and folders/);
  await key('ArrowDown');
  assert.match(await selected(), /Attach files/);
  await key('ArrowUp');
  assert.match(await selected(), /Files and folders/);
  await key('Enter', true);
  assert.equal(await evaluate('Boolean(document.querySelector("[aria-label=\\\"File search query\\\"]"))'), false);
  await key('Escape');
  await wait('!document.querySelector("#mention-results")');
  assert.equal(await evaluate('document.querySelector("[aria-label=Prompt]").textContent'), '@');

  // Hidden Electron windows throttle animation frames; freeze motion for
  // reproducible visual captures after exercising the normal menu above.
  await evaluate(`(() => { const style = document.createElement('style'); style.textContent = '*, .beta-composer { animation: none !important; transition: none !important; }'; document.head.append(style); })()`);
  for (const theme of ['light', 'dark']) {
    await evaluate(`window.__composerDraftTest.setTheme('${theme}')`);
    await prompt('@');
    await wait('Boolean(document.querySelector("#mention-results"))');
    await delay(170);
    if (process.env.LINGXI_MENTION_SCREENSHOT) await writeFile(`${process.env.LINGXI_MENTION_SCREENSHOT}-${theme}.png`, (await page.capturePage()).toPNG());
  }
  window.setContentSize(540, 740);
  await delay(170);
  assert.equal(await evaluate(`(() => { const menu = document.querySelector('.mention-command-menu').getBoundingClientRect(); const composer = document.querySelector('.beta-composer').getBoundingClientRect(); return menu.x >= 0 && menu.right <= innerWidth && menu.bottom <= composer.top - 8; })()`), true);
  window.setContentSize(1000, 752);
  await delay(170);
  await key('Enter');
  await wait('Boolean(document.querySelector("[aria-label=\\\"File search query\\\"]"))');
  await wait('document.querySelectorAll("#mention-results [role=option]").length === 4');
  await key('Escape');
  await prompt('inspect @src/main');
  await wait('document.querySelectorAll("#mention-results [role=option]").length === 1');
  await key('Enter');
  await wait('Boolean(document.querySelector("[data-file-mention]"))');
  assert.equal(await evaluate('document.querySelector("[data-file-mention]").dataset.fileMention'), 'src/main.ts');
  assert.equal(await evaluate('document.querySelector("[aria-label=Prompt]").textContent.includes("@src")'), false);
  assert.match(await evaluate('document.querySelector("[data-file-mention]").getAttribute("href")'), /^lingxi-mention:\/\/file/);
  await evaluate('document.querySelector("[data-file-mention]").click()');
  await wait('document.querySelector("[aria-label=\\\"Reference preview\\\"]")?.textContent.includes("export const value = 42;")');
  await key('Escape');

  // Inserting a second @ directly after an atomic token must work, and the
  // original file query must never be duplicated in the serialized message.
  await page.insertText('@Browser');
  await wait('document.querySelector("#mention-results")?.textContent.includes("Control the in-app browser")');
  await key('Tab');
  await wait('Boolean(document.querySelector("[data-context-mention]"))');
  await evaluate(`(() => { const link = document.querySelector('[data-context-mention]'); link.focus(); link.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })); })()`);
  await wait('window.__composerDraftTest.openedSettings().length === 1');
  assert.deepEqual(await evaluate('window.__composerDraftTest.openedSettings()'), ['plugins']);
  await evaluate('window.__composerDraftTest.switchSession("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb")');
  await wait('document.querySelector("[aria-label=Prompt]").textContent === ""');
  await evaluate('window.__composerDraftTest.switchSession("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")');
  await wait('document.querySelectorAll("[data-context-mention], [data-file-mention]").length === 2');
  const copied = await evaluate(`(() => { const editor = document.querySelector('[aria-label="Prompt"]'); const range = document.createRange(); range.selectNodeContents(editor); getSelection().removeAllRanges(); getSelection().addRange(range); const clipboardData = new DataTransfer(); editor.dispatchEvent(new ClipboardEvent('copy', { bubbles: true, clipboardData })); return clipboardData.getData('text/plain'); })()`);
  await evaluate('document.querySelector("[aria-label=\\\"Send prompt\\\"]").click()');
  await wait('window.__composerDraftTest.sendPending()');
  const sent = await evaluate('window.__composerDraftTest.lastSentPrompt()');
  assert.equal(copied, sent);
  assert(sent.startsWith('@src/main.ts\n\ninspect '));
  assert.match(sent, /\[@Browser\]\(lingxi-mention:\/\/plugin\?name=Browser&target=browser%40local "browser@local"\)/);
  await evaluate('window.__composerDraftTest.resolveSend()');
  await wait('document.querySelector("[aria-label=Prompt]").textContent === ""');

  await prompt('@review');
  await wait('document.querySelectorAll("#mention-results [role=option]").length === 1');
  await key('Enter');
  await wait('Boolean(document.querySelector("[data-context-mention]"))');
  await evaluate('document.querySelector("[data-context-mention]").click()');
  assert.deepEqual(await evaluate('window.__composerDraftTest.openedSettings()'), ['plugins', 'skills']);

  await prompt('@src/');
  await wait('document.querySelectorAll("#mention-results [role=option]").length === 2');
  await key('ArrowDown');
  await key('Enter');
  await wait('Boolean(document.querySelector("[data-file-mention]"))');
  assert.equal(await evaluate('document.querySelector("[data-file-mention]").dataset.fileMention'), 'src/');
  await evaluate('document.querySelector("[data-file-mention]").click()');
  await wait(`document.querySelector('[aria-label="Reference preview"]')?.textContent.includes('src/main.ts')`);
  await key('Escape');

  await prompt('@"My Files/read');
  await wait('document.querySelectorAll("#mention-results [role=option]").length === 1');
  await key('Enter');
  await wait('Boolean(document.querySelector("[data-file-mention]"))');
  assert.equal(await evaluate('document.querySelector("[data-file-mention]").dataset.fileMention'), 'My Files/read me.md');

  await prompt('@plan');
  await wait('document.querySelectorAll("#mention-results [role=option]").length === 1');
  await key('Enter');
  await wait('window.__composerDraftTest.permissionChanges().length === 1');
  assert.deepEqual(await evaluate('window.__composerDraftTest.permissionChanges()'), ['plan']);
  assert.equal(await evaluate('document.querySelector("[aria-label=Prompt]").textContent'), '');
  await prompt('finish the review @goal');
  await wait('document.querySelectorAll("#mention-results [role=option]").length === 1');
  await key('Enter');
  await wait('document.querySelector("[aria-label=Prompt]").textContent === "/goal finish the review "');
  await prompt('email@example.com');
  assert.equal(await evaluate('Boolean(document.querySelector("#mention-results"))'), false);
  await prompt('@slow');
  await delay(110);
  await prompt('@src/main');
  await wait('document.querySelectorAll("#mention-results [role=option]").length === 1');
  await delay(300);
  assert.match(await selected(), /main.ts/);
  await evaluate('document.body.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }))');
  await wait('!document.querySelector("#mention-results")');
  process.stdout.write('mention interactions passed\n');
} finally {
  window.destroy();
}
}
main().then(() => app.quit()).catch((cause) => { process.stderr.write(`${cause.stack}\n`); app.exit(1); });
