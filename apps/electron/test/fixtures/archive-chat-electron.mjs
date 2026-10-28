import { app, BrowserWindow } from 'electron';
const url = process.argv.find((argument) => argument.startsWith('http://'));
if (!url || !process.env.LINGXI_TEST_USER_DATA) throw new Error('fixture URL and isolated user data required');
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 1100, height: 800, webPreferences: { sandbox: true } });
  try {
    await window.loadURL(url);
    window.showInactive();
    window.webContents.focus();
    const run = (script) => window.webContents.executeJavaScript(script);
    const pressKey = async (keyCode, modifiers = []) => {
      if (!await run('document.hasFocus()')) {
        await run('window.__focusBeforeActivation = document.activeElement');
        window.focus();
        window.webContents.focus();
        await wait('document.hasFocus()');
        await run('window.__focusBeforeActivation?.focus()');
      }
      window.webContents.sendInputEvent({ type: 'keyDown', keyCode, modifiers });
      if (keyCode === 'ENTER') window.webContents.sendInputEvent({ type: 'char', keyCode: '\r', modifiers });
      window.webContents.sendInputEvent({ type: 'keyUp', keyCode, modifiers });
    };
    const wait = async (expression) => {
      const deadline = Date.now() + 6000;
      while (Date.now() < deadline) { if (await run(expression)) return; await delay(20); }
      throw new Error(`Timed out: ${expression}\n${JSON.stringify(await run('({ active: document.activeElement?.outerHTML?.slice(0, 180), key: window.__sidebarKey, menuKey: window.__menuKey, row: window.__keyTarget?.outerHTML?.slice(0, 220), visible: window.__keyTarget?.getClientRects().length, docFocused: document.hasFocus(), fixture: window.archiveChatFixture?.state(), dialog: !!document.querySelector("[role=dialog]") })'))}\n${await run('document.body.innerText')}`);
    };
    const dialog = 'document.querySelector("[role=dialog]")';
    const confirm = 'document.querySelector(".desktop-dialog-action--primary")';
    const cancel = 'document.querySelector(".desktop-dialog-action--cancel")';
    const open = async () => {
      await run('document.querySelector("#opener").focus(); document.querySelector("#opener").click()');
      await wait(`Boolean(${dialog})`);
      await delay(30);
    };
    const configure = async (settings) => {
      await run(`window.archiveChatFixture.configure(${JSON.stringify(settings)})`);
      await delay(30);
    };
    const state = () => run('window.archiveChatFixture.state()');
    const openSidebarArchiveWithKeyboard = async (sessionId) => {
      const row = `document.querySelector('button[data-session-id="${sessionId}"]')`;
      await run(`window.__keyTarget = ${row}; ${row}.focus(); window.__sidebarKey = null; ${row}.addEventListener('keydown', event => {
        window.__sidebarKey = { key: event.key, shiftKey: event.shiftKey, trusted: event.isTrusted };
      }, { once: true })`);
      await pressKey('F10', ['shift']);
      await wait(`document.activeElement === document.querySelector('.session-actions-menu [role="menuitem"]:first-child')`);
      await pressKey('TAB');
      await wait(`document.activeElement === document.querySelector('.session-actions-menu [role="menuitem"]:nth-child(2)')`);
      await pressKey('TAB');
      await wait(`document.activeElement === document.querySelector('.session-actions-menu [role="menuitem"]:nth-child(3)')`);
      await run(`window.__menuKey = null; document.activeElement.addEventListener('keydown', event => { window.__menuKey = { key: event.key, trusted: event.isTrusted }; }, { once: true })`);
      await run(`window.__menuEnterReleased = false; document.addEventListener('keyup', event => {
        if (event.key === 'Enter' && event.isTrusted) window.__menuEnterReleased = true;
      }, { once: true })`);
      await pressKey('ENTER');
      await wait(`Boolean(${dialog}) && !${confirm}.disabled`);
      await wait(`window.__menuEnterReleased === true`);
    };
    await wait('Boolean(window.archiveChatFixture)');
    await configure({ loading: true });
    await open();
    const result = { loadingDisabled: await run(`${confirm}.disabled`) };
    await run(`${confirm}.click(); ${cancel}.click()`);
    await wait(`!${dialog}`);
    result.afterCancel = await state();
    await configure({});
    await open();
    result.content = await run(`${dialog}.textContent`);
    await run(`${confirm}.focus(); ${confirm}.dispatchEvent(new KeyboardEvent('keydown', {key:'Tab', bubbles:true, cancelable:true}))`);
    result.forwardTrap = await run(`document.activeElement === ${dialog}.querySelector('button')`);
    await run(`document.activeElement.dispatchEvent(new KeyboardEvent('keydown', {key:'Tab', shiftKey:true, bubbles:true, cancelable:true}))`);
    result.backwardTrap = await run(`document.activeElement === ${confirm}`);
    await run(`document.activeElement.dispatchEvent(new KeyboardEvent('keydown', {key:'Escape', bubbles:true, cancelable:true}))`);
    await wait(`!${dialog}`);
    result.escapeRestored = await run('document.activeElement === document.querySelector("#opener")');
    await configure({ busy: true });
    await open();
    result.initialBusyFocused = await run(`document.activeElement === ${dialog}`);
    await configure({ busy: false });
    await run(`${dialog}.dispatchEvent(new KeyboardEvent('keydown', {key:'Escape', bubbles:true, cancelable:true}))`);
    await wait(`!${dialog}`);
    result.initialBusyRestored = await run('document.activeElement === document.querySelector("#opener")');
    await open();
    // Observe the actual disabled-attribute commit, before a later passive
    // React effect or test-side delay can move focus back into the dialog.
    await run(`(() => {
      const target = ${dialog};
      window.__busyFocusAtDisable = [];
      window.__busyFocusObserver = new MutationObserver(() => {
        const buttons = [...target.querySelectorAll('button')];
        if (buttons.length && buttons.every(button => button.disabled)) {
          window.__busyFocusAtDisable.push({
            contained: document.activeElement === target,
            active: document.activeElement?.outerHTML?.slice(0, 180) ?? null,
          });
        }
      });
      window.__busyFocusObserver.observe(target, { subtree: true, attributes: true, attributeFilter: ['disabled'] });
    })()`);
    await configure({ busy: true });
    result.busyFocusAtDisable = await run(`window.__busyFocusAtDisable`);
    await run(`window.__busyFocusObserver.disconnect()`);
    result.busyFocusContained = await run(`document.activeElement === ${dialog}`);
    result.busyTabPrevented = await run(`!${dialog}.dispatchEvent(new KeyboardEvent('keydown', {key:'Tab', bubbles:true, cancelable:true}))`);
    result.busyAllDisabled = await run(`[...${dialog}.querySelectorAll('button')].every(button => button.disabled)`);
    await run(`${cancel}.click(); ${confirm}.click(); ${dialog}.dispatchEvent(new KeyboardEvent('keydown', {key:'Escape', bubbles:true, cancelable:true}))`);
    await delay(30);
    result.busyRemainsOpen = (await state()).open;
    await configure({ error: 'Unable to check scheduled tasks' });
    result.errorDisabled = await run(`${confirm}.disabled`);
    await configure({});
    const point = await run(`(() => { const rect = ${confirm}.getBoundingClientRect(); return { x: Math.round(rect.x + rect.width / 2), y: Math.round(rect.y + rect.height / 2) }; })()`);
    window.webContents.sendInputEvent({ type: 'mouseDown', ...point, button: 'left', clickCount: 1 });
    window.webContents.sendInputEvent({ type: 'mouseUp', ...point, button: 'left', clickCount: 1 });
    await wait(`!${dialog}`);
    result.afterConfirm = await state();

    await openSidebarArchiveWithKeyboard('fixture-session');
    await pressKey('ESCAPE');
    await wait(`!${dialog}`);
    result.sidebarCancelRestored = await run(`document.activeElement === document.querySelector('button[data-session-id="fixture-session"]')`);

    await openSidebarArchiveWithKeyboard('fixture-session');
    await run(`document.querySelector('button[data-session-id="fixture-neighbor"]').focus()`);
    await pressKey('ENTER');
    await wait(`!${dialog} && window.archiveChatFixture.state().openedSession === 'fixture-neighbor'`);
    result.switchDidNotRestoreOldRow = await run(`document.activeElement !== document.querySelector('button[data-session-id="fixture-session"]')`);

    await openSidebarArchiveWithKeyboard('fixture-session');
    await delay(250);
    result.sidebarHitTarget = await run(`(() => { const rect = ${confirm}.getBoundingClientRect(); return document.elementFromPoint(rect.x + rect.width / 2, rect.y + rect.height / 2)?.closest('button') === ${confirm}; })()`);
    const sidebarPoint = await run(`(() => { const rect = ${confirm}.getBoundingClientRect(); return { x: Math.round(rect.x + rect.width / 2), y: Math.round(rect.y + rect.height / 2) }; })()`);
    window.webContents.sendInputEvent({ type: 'mouseDown', ...sidebarPoint, button: 'left', clickCount: 1 });
    window.webContents.sendInputEvent({ type: 'mouseUp', ...sidebarPoint, button: 'left', clickCount: 1 });
    await wait(`Boolean(${dialog}?.querySelector('[role=alert]'))`);
    result.retryEnabled = await run(`!${confirm}.disabled && ${confirm}.textContent === 'Try again'`);
    await run(`${confirm}.click()`);
    await wait(`${confirm}.textContent === 'Archive chat' && !${confirm}.disabled`);
    await run(`${confirm}.click()`);
    await wait(`!${dialog}`);
    result.sidebarCalls = (await state()).sidebarCalls;
    result.sidebarArchivedRowRemoved = await run(`!document.querySelector('button[data-session-id="fixture-session"]')`);
    result.sidebarArchiveFocusedNeighbor = await run(`document.activeElement === document.querySelector('button[data-session-id="fixture-neighbor"]')`);

    await openSidebarArchiveWithKeyboard('fixture-neighbor');
    await wait(`document.activeElement === ${dialog}.querySelector('button')`);
    await pressKey('TAB');
    await wait(`document.activeElement === ${cancel}`);
    await pressKey('TAB');
    await wait(`document.activeElement === ${confirm}`);
    await pressKey('ENTER');
    await wait(`!${dialog} && !document.querySelector('button[data-session-id="fixture-neighbor"]')`);
    result.sidebarArchiveFocusedList = await run(`document.activeElement === document.querySelector('.desktop-sidebar-list')`);
    process.stdout.write(`${JSON.stringify(result)}\n`);
  } finally { window.destroy(); app.quit(); }
}
main().catch((error) => { process.stderr.write(`${error.stack ?? error}\n`); app.exit(1); });
