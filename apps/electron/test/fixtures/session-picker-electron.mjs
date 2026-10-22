import assert from 'node:assert/strict';
import { app, BrowserWindow } from 'electron';
import { configureFixtureWindow, waitForFixture } from './electron-test-environment.mjs';

const url = process.argv.find(value => value.startsWith('http://'));
if (!url || !process.env.LINGXI_TEST_USER_DATA) throw new Error('isolated fixture required');
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
app.commandLine.appendSwitch('disable-gpu');

async function main() {
  await app.whenReady();
  const win = new BrowserWindow({ show: false, width: 1200, height: 800,
    webPreferences: { sandbox: true, backgroundThrottling: false } });
  const web = win.webContents;
  const js = source => web.executeJavaScript(source);
  const wait = expression => waitForFixture(web, expression, 15000);
  const settle = () => js('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
  const prompt = 'document.querySelector(".beta-rich-prompt")';
  const search = 'document.querySelector(".session-picker input")';
  const modal = 'document.querySelector(".session-picker")';
  const fixture = 'window.sessionPickerFixture';
  const sessionA = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';
  const sessionB = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb';
  async function press(keyCode, modifiers = []) {
    web.sendInputEvent({ type: 'keyDown', keyCode, modifiers });
    web.sendInputEvent({ type: 'keyUp', keyCode, modifiers });
    await settle();
  }
  async function setDraft(text) {
    await js(`(() => {
      const editor = ${prompt}; editor.textContent = ${JSON.stringify(text)}; editor.focus();
      const range = document.createRange(); range.selectNodeContents(editor); range.collapse(false);
      const selection = getSelection(); selection.removeAllRanges(); selection.addRange(range);
      editor.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText' }));
    })()`);
    await settle();
  }
  try {
    await win.loadURL(url);
    await configureFixtureWindow(win);
    await wait(`Boolean(${fixture} && ${prompt}?.isContentEditable && document.querySelector('[data-session-id="${sessionB}"]'))`);
    await setDraft('/resume '); await press('Enter');
    await wait(`Boolean(${modal} && ${search} === document.activeElement && !${search}.disabled && document.querySelector('.session-picker-row'))`);

    // Native Tab traversal and the React boundary trap must keep focus inside.
    await press('Tab');
    assert.equal(await js(`document.activeElement.classList.contains('desktop-dialog-action')`), true);
    await press('Tab');
    assert.equal(await js(`document.activeElement.getAttribute('aria-label')`), 'Close chat picker');
    await press('Tab', ['shift']);
    assert.equal(await js(`document.activeElement.classList.contains('desktop-dialog-action')`), true);
    await press('Tab', ['shift']);
    assert.equal(await js(`${search} === document.activeElement`), true);

    const ime = await js(`(() => {
      const input = ${search};
      input.dispatchEvent(new CompositionEvent('compositionstart', { bubbles: true, data: '中文' }));
      const composing = new KeyboardEvent('keydown', { key: 'Escape', code: 'Escape', isComposing: true, bubbles: true, cancelable: true });
      input.dispatchEvent(composing);
      const first = { open: Boolean(${modal}), prevented: composing.defaultPrevented };
      input.dispatchEvent(new CompositionEvent('compositionend', { bubbles: true, data: '中文' }));
      const legacy = new KeyboardEvent('keydown', { key: 'Escape', code: 'Escape', keyCode: 229, bubbles: true, cancelable: true });
      input.dispatchEvent(legacy);
      return { first, legacy: { open: Boolean(${modal}), prevented: legacy.defaultPrevented, keyCode: legacy.keyCode } };
    })()`);
    assert.deepEqual(ime, { first: { open: true, prevented: false }, legacy: { open: true, prevented: false, keyCode: 229 } });
    await settle();
    assert.equal(await js(`Boolean(${modal})`), true, 'IME Escape must not schedule a close on the next React commit');
    await press('Escape');
    await wait(`!${modal} && document.activeElement === ${prompt}`);

    // beginLocalCommand really waits on the host; navigation uses the real
    // sidebar, hook and App before that original promise is released.
    await js(`${fixture}.deferNextLocalCommand()`);
    await setDraft('/resume investigation'); await press('Enter');
    await wait(`${fixture}.state().pending`);
    await js(`document.querySelector('[data-session-id="${sessionB}"]').click()`);
    await wait(`${fixture}.state().active.sessionId === '${sessionB}' && Boolean(document.querySelector('[data-session-id="${sessionB}"][aria-current="page"]')) && ${prompt}?.isContentEditable`);
    await js(`${fixture}.finishLocalCommand()`);
    await wait(`${fixture}.state().completedCommands === 2`);
    await settle();
    assert.equal(await js(`${fixture}.state().pickerEvents.length`), 1, 'the original A command must not emit a picker event after navigation to B');
    assert.equal(await js(`Boolean(${modal})`), false);
    assert.equal(await js(`${fixture}.state().listCalls.some(call => call.projectPath === '/fixture/project-B' && call.scope !== 'current')`), false);

    await setDraft('/resume '); await press('Enter');
    await wait(`Boolean(${modal} && ${search} === document.activeElement)`);
    assert.deepEqual(await js(`${fixture}.state().pickerEvents.map(event => event.projectPath)`), ['/fixture/project-A', '/fixture/project-B']);
    await press('Escape');
    await wait(`!${modal} && document.activeElement === ${prompt}`);

    // A real pending open can park on a deferred tool approval before the host
    // acknowledges the resume. Its target card must be outside picker inertness.
    await js(`${fixture}.deferNextResumeApproval()`);
    await setDraft('/resume '); await press('Enter');
    await wait(`Boolean(${modal} && document.querySelector('.session-picker-row[title$="${sessionA}.jsonl"]'))`);
    await js(`document.querySelector('.session-picker-row[title$="${sessionA}.jsonl"]').click()`);
    await wait(`Boolean(${fixture}.state().resumePending && !${modal} && document.querySelector('.permission-prompt-inline'))`);
    const permissionButton = 'document.querySelector(".permission-prompt-inline .desktop-dialog-action--primary")';
    assert.deepEqual(await js(`({
      interactive: Boolean(${permissionButton} && !${permissionButton}.closest('[inert]') && !${permissionButton}.closest('[aria-hidden="true"]')),
      oldActive: ${fixture}.state().active.sessionId,
      focusedOnApproval: Boolean(document.activeElement?.closest('.permission-prompt-inline')),
    })`), { interactive: true, oldActive: sessionB, focusedOnApproval: true });
    const openedBeforeNavigation = await js(`${fixture}.state().openCalls.length`);
    for (const selector of [
      `[data-session-id="${sessionB}"]`, '.desktop-nav-rail [aria-label="Apps"]',
      '.desktop-nav-rail [aria-label="Scheduled tasks"]', '.desktop-rail-settings',
      '.sidebar-new-chat', '[aria-label="Add project"]', '[aria-label="Project sidebar options"]',
    ]) {
      assert.equal(await js(`Boolean(document.querySelector(${JSON.stringify(selector)})?.closest('[inert]'))`), true, selector);
      await js(`document.querySelector(${JSON.stringify(selector)}).click()`); await settle();
      assert.deepEqual(await js(`({
        pendingTarget: ${fixture}.state().resumeTarget,
        openCalls: ${fixture}.state().openCalls.length,
        otherNavigation: ${fixture}.state().navigationCalls,
        approvalReachable: Boolean(${permissionButton}?.getClientRects().length && !${permissionButton}.closest('[inert]')),
        pickerHidden: !${modal},
      })`), { pendingTarget: sessionA, openCalls: openedBeforeNavigation, otherNavigation: [], approvalReachable: true, pickerHidden: true }, selector);
    }
    await js(`${fixture}.applicationMenu('settings'); ${fixture}.applicationMenu('activity');
      window.dispatchEvent(new CustomEvent('lingxi:session-picker', { detail: { query: 'must not replace pending picker' } }));`);
    await settle();
    assert.equal(await js(`Boolean(!${modal} && ${permissionButton}?.getClientRects().length && !${permissionButton}.closest('[inert]'))`), true);
    const sendButton = 'document.querySelector(".composer-send-presence button")';
    await setDraft('Blocked until resume acknowledgement');
    assert.equal(await js(`${sendButton}?.disabled`), true, 'loading permits drafting but disables submission');
    await press('Enter');
    assert.equal(await js(`${fixture}.state().promptCalls.length`), 0, 'Enter must not send a prompt while waiting for resume approval');
    assert.equal(await js(`${prompt}.textContent`), 'Blocked until resume acknowledgement');
    await js(`${permissionButton}.click()`);
    await wait(`Boolean(${fixture}.state().resumeApproved && ${modal} && !document.querySelector('.permission-prompt-inline'))`);
    assert.equal(await js(`${sendButton}?.disabled`), true, 'approving a tool must not admit a prompt before resume acknowledgement');
    assert.equal(await js(`${fixture}.state().promptCalls.length`), 0);
    assert.equal(await js(`${search}.disabled`), true, 'the same picker retains its in-flight opening state');
    assert.deepEqual(await js(`${fixture}.state().approvalCalls`), [{ sessionId: sessionA, requestId: 77, response: { type: 'allow_once' } }]);
    await js(`${fixture}.finishResume()`);
    await wait(`!${modal} && !${fixture}.state().resumePending && ${fixture}.state().active.sessionId === '${sessionA}' && ${prompt}?.isContentEditable`);
    await setDraft('Ready after resume acknowledgement'); await press('Enter');
    await wait(`${fixture}.state().promptCalls.length === 1`);
    assert.deepEqual(await js(`${fixture}.state().promptCalls`), [{ sessionId: sessionA, text: 'Ready after resume acknowledgement' }]);
    process.stdout.write(JSON.stringify({ imeEscape: true, ordinaryEscapeFocus: true, tabTrap: true, navigationFence: true, resumeApproval: true }) + '\n');
  } finally { win.destroy(); app.quit(); }
}
main().catch(error => { process.stderr.write(String(error.stack) + '\n'); app.exit(1); });
