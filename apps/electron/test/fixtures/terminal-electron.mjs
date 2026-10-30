import { app, BrowserWindow } from 'electron';
import { mkdirSync, writeFileSync } from 'node:fs';
import { configureFixtureWindow, settleFixtureAnimations } from './electron-test-environment.mjs';
const url = process.argv.find(arg => arg.startsWith('http://'));
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const trace = message => { if (process.env.LINGXI_TERMINAL_TEST_TRACE === '1') process.stderr.write(`[terminal fixture] ${message}\n`); };
async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 1200, height: 820, webPreferences: { sandbox: true, backgroundThrottling: false } });
  try {
    await window.loadURL(url);
    await configureFixtureWindow(window);
    const run = script => window.webContents.executeJavaScript(script);
    const wait = async expr => { trace(`waiting: ${expr}`); for (let n = 0; n < 300; n++) { if (await run(expr)) { trace(`ready: ${expr}`); return; } await delay(20); } throw new Error(`Timeout: ${expr}\n${await run('document.body.innerText')}\n${JSON.stringify(await run('window.terminalFixture?.state()'))}`); };
    const click = selector => run(`document.querySelector(${JSON.stringify(selector)}).click()`);
    const state = () => run('window.terminalFixture.state()');
    await wait('Boolean(window.terminalFixture)');
    await click('#toggle');
    await wait('document.querySelector(".xterm-helper-textarea") && window.terminalFixture.state().acks.length > 0');
    const first = (await state()).selected;
    await run(`window.terminalFixture.output('${first}', '\\r\\n\\x1b[32m中文 shell ready\\x1b[0m\\r\\n')`);
    await wait('window.terminalFixture.state().acks.some(item => item.sequence === 1)');
    const result = { outputAcknowledged: true };
    const ackCount = (await state()).acks.length;
    await run(`window.terminalFixture.duplicate('${first}')`); await delay(70);
    result.duplicateIgnored = (await state()).acks.length === ackCount + 1;
    await run(`window.terminalFixture.output('${first}', 'reset shell % ', true)`);
    await wait('window.terminalFixture.state().acks.some(item => item.sequence === 2)'); result.resetAcknowledged = true;
    await run('document.querySelector(".xterm-helper-textarea").focus()');
    window.webContents.sendInputEvent({ type: 'keyDown', keyCode: 'A' }); window.webContents.sendInputEvent({ type: 'char', keyCode: 'a' }); window.webContents.sendInputEvent({ type: 'keyUp', keyCode: 'A' });
    await wait('window.terminalFixture.state().inputs.length > 0'); result.inputWorks = true;
    await run(`(() => { const textarea = document.querySelector('.xterm-helper-textarea'); textarea.value = ''; textarea.dispatchEvent(new CompositionEvent('compositionstart', { bubbles: true })); textarea.value = '你好'; textarea.dispatchEvent(new CompositionEvent('compositionupdate', { data: '你好', bubbles: true })); textarea.dispatchEvent(new CompositionEvent('compositionend', { data: '你好', bubbles: true })); })()`);
    // xterm commits a composition on a later task. Do not send the next key
    // before those actual UTF-8 characters have reached the terminal bridge.
    await wait(`window.terminalFixture.state().inputs.map(item => item.data).join('').includes('你好')`);
    window.webContents.sendInputEvent({ type: 'keyDown', keyCode: 'C', modifiers: ['control'] });
    window.webContents.sendInputEvent({ type: 'keyUp', keyCode: 'C', modifiers: ['control'] });
    window.webContents.sendInputEvent({ type: 'keyDown', keyCode: 'Tab' });
    window.webContents.sendInputEvent({ type: 'keyUp', keyCode: 'Tab' });
    await wait(`window.terminalFixture.state().inputs.map(item => item.data).join('').includes('你好') && window.terminalFixture.state().inputs.some(item => item.data === '\u0003') && window.terminalFixture.state().inputs.some(item => item.data === '\t')`);
    result.nativeKeysWork = true;
    await click('[aria-label="Hide terminal panel"]');
    await run(`window.terminalFixture.output('${first}', '\\r\\nhidden output\\r\\n')`);
    await wait('window.terminalFixture.state().acks.some(item => item.sequence === 3)');
    result.hiddenShellRetained = (await state()).count === 1 && await run('document.querySelector("#desktop-terminal").inert');
    await run('window.dispatchEvent(new KeyboardEvent("keydown", {ctrlKey:true, code:"Backquote", bubbles:true, cancelable:true}))');
    await wait('!document.querySelector("#desktop-terminal").inert'); result.shortcutWorks = true;
    await click('[aria-label="New terminal"]'); await wait(`window.terminalFixture.state().count === 2 && window.terminalFixture.state().selected !== '${first}'`);
    const second = (await state()).selected;
    await run('document.querySelector("#composer").focus(); window.terminalFixture.session("session-b")'); await delay(80);
    result.switchFocusPreserved = await run('document.activeElement.id === "composer"');
    await click('#toggle'); await wait('window.terminalFixture.state().count === 3');
    await run('document.querySelector("#composer").focus(); window.terminalFixture.session("session-a")'); await delay(80);
    result.restoredSelection = (await state()).selected === second && (await state()).open && await run('document.activeElement.id === "composer"');
    await run('document.querySelector(".terminal-resizer").dispatchEvent(new KeyboardEvent("keydown", { key:"ArrowUp", bubbles:true, cancelable:true }))');
    await wait('localStorage.getItem("lingxi.terminal.height.v1") === "264"'); result.heightPersisted = true;
    result.resizeWorks = (await state()).sizes.length > 0;
    // The panel's height transition may still be in flight in a hidden CI
    // window even after a fixed delay. Measure only the settled 264 px panel.
    await settleFixtureAnimations(window.webContents, '#desktop-terminal');
    await wait(`Math.abs(document.querySelector('#desktop-terminal').getBoundingClientRect().height - 264) < 1`);
    const handle = await run(`(() => { const r = document.querySelector('.terminal-resizer').getBoundingClientRect(); return { x: Math.round(r.x + r.width / 2), y: Math.floor(r.bottom - 1) }; })()`);
    window.webContents.sendInputEvent({ type: 'mouseMove', ...handle });
    window.webContents.sendInputEvent({ type: 'mouseDown', ...handle, button: 'left', clickCount: 1 });
    // Wait for pointer capture and the move listener before moving outside
    // the 7 px handle; otherwise a busy renderer can miss the move entirely.
    await wait(`document.querySelector('#desktop-terminal').dataset.resizing === 'true'`);
    window.webContents.sendInputEvent({ type: 'mouseMove', x: handle.x, y: handle.y - 40, modifiers: ['leftButtonDown'] });
    await wait(`Math.abs(document.querySelector('#desktop-terminal').getBoundingClientRect().height - 304) < 1`);
    result.dragMetrics = await run(`(() => { const panel = document.querySelector('#desktop-terminal'); const r = document.querySelector('.terminal-resizer').getBoundingClientRect(); return { resizing: panel.dataset.resizing, transition: getComputedStyle(panel).transitionDuration, height: panel.getBoundingClientRect().height, expectedHeight: panel.style.height, handle: {top:r.top, bottom:r.bottom}, active: document.activeElement?.className }; })()`);
    result.directResize = await run(`(() => { const panel = document.querySelector('#desktop-terminal'); return panel.dataset.resizing === 'true' && getComputedStyle(panel).transitionDuration === '0s' && Math.abs(panel.getBoundingClientRect().height - 304) < 1; })()`);
    window.webContents.sendInputEvent({ type: 'mouseUp', x: handle.x, y: handle.y - 40, button: 'left', clickCount: 1 });
    await wait(`!document.querySelector('#desktop-terminal').hasAttribute('data-resizing')`);
    result.resizeReleased = await run(`!document.querySelector('#desktop-terminal').hasAttribute('data-resizing')`);
    const screenshots = process.env.LINGXI_TERMINAL_SCREENSHOTS || '/tmp/lingxi-terminal-ui'; mkdirSync(screenshots, { recursive: true });
    await run(`window.terminalFixture.output('${second}', '\\r\\n\\x1b[32m✓ Terminal ready\\x1b[0m\\r\\n中文输入 · ANSI colors · session isolation\\r\\n(base) user@MacBook LingXi-Next % ')`); await delay(150);
    writeFileSync(`${screenshots}/light.png`, (await window.webContents.capturePage()).toPNG());
    await run('window.terminalFixture.theme(true)'); await delay(100); writeFileSync(`${screenshots}/dark.png`, (await window.webContents.capturePage()).toPNG());
    await run(`window.terminalFixture.exit('${second}')`); await wait('document.body.innerText.includes("Shell exited (7)")');
    await run('window.terminalFixture.failClose(true)');
    await click('.terminal-exit-status button'); await delay(50);
    result.failedClosePreservesShell = (await state()).count === 3 && (await state()).selected === second;
    await run('window.terminalFixture.failClose(false)');
    await click('.terminal-exit-status button'); await wait(`window.terminalFixture.state().selected !== '${second}' && window.terminalFixture.state().closed.includes('${second}')`);
    result.restartWorks = (await state()).count === 3;
    await click('.terminal-tab .terminal-icon-button'); await wait('window.terminalFixture.state().count === 2'); result.closedOneOnly = true;
    const remaining = (await state()).selected;
    await run(`window.terminalFixture.migrate('${remaining}', 'session-migrated'); document.querySelector('#composer').focus(); window.terminalFixture.session('session-migrated')`);
    await delay(80);
    result.migrationPreservesShell = (await state()).selected === remaining && (await state()).open && await run('document.activeElement.id === "composer"');
    // Hold an old running snapshot while the unknown event and late output
    // arrive, then ensure the stale snapshot cannot restore input admission.
    await run('window.terminalFixture.session("session-a")'); await delay(80);
    await run('window.terminalFixture.holdList(); window.terminalFixture.session("session-migrated")');
    await wait('window.terminalFixture.state().listHeld');
    await run(`window.terminalFixture.unknown('${remaining}')`);
    await wait('document.body.innerText.includes("Terminal status could not be confirmed")');
    await run(`window.terminalFixture.output('${remaining}', '\\r\\nunknown tail 中文\\r\\n')`);
    await wait(`document.querySelector('.terminal-surface:not([hidden]) .xterm-rows')?.innerText.includes('unknown tail 中文')`);
    await run('window.terminalFixture.releaseList()'); await delay(80);
    result.unknownSurvivesStaleSnapshot = await run('document.body.innerText.includes("Terminal status could not be confirmed") && !document.body.innerText.includes("Shell exited")');
    const inputCount = (await state()).inputs.length;
    await run('document.querySelector(".terminal-surface:not([hidden]) .xterm-helper-textarea").focus()');
    window.webContents.sendInputEvent({ type: 'keyDown', keyCode: 'A' }); window.webContents.sendInputEvent({ type: 'char', keyCode: 'a' }); window.webContents.sendInputEvent({ type: 'keyUp', keyCode: 'A' });
    await delay(80);
    result.unknownDisablesInput = (await state()).inputs.length === inputCount;
    result.unknownTailRendered = await run(`document.querySelector('.terminal-surface:not([hidden]) .xterm-rows')?.innerText.includes('unknown tail 中文')`);
    await click('.terminal-tab .terminal-icon-button');
    await wait(`window.terminalFixture.state().closed.includes('${remaining}')`);
    result.unknownCloseWorks = true;
    // The broker event can predate window watch registration, so a create
    // snapshot itself must protect unknown against later stale list results.
    await run('window.terminalFixture.createUnknownNext()');
    await click('[aria-label="New terminal"]');
    await wait('document.body.innerText.includes("Terminal status could not be confirmed")');
    const snapshotOnly = (await state()).selected;
    await run('window.terminalFixture.session("session-a")'); await delay(80);
    await run('window.terminalFixture.staleNextList(); window.terminalFixture.session("session-migrated")'); await delay(80);
    result.unknownSnapshotSurvivesStaleList = (await state()).selected === snapshotOnly && await run('document.body.innerText.includes("Terminal status could not be confirmed")');
    await click('.terminal-tab .terminal-icon-button'); await wait(`window.terminalFixture.state().closed.includes('${snapshotOnly}')`);
    trace(`result: ${JSON.stringify(result)}`);
    await new Promise(resolve => process.stdout.write(`${JSON.stringify(result)}\n`, resolve));
  } finally { trace('destroying window'); window.destroy(); }
}
main().then(() => app.exit(0)).catch(error => { process.stderr.write(`${error.stack ?? error}\n`); app.exit(1); });
