import assert from 'node:assert/strict';
import { app, BrowserWindow } from 'electron';
import { configureFixtureWindow, settleFixtureAnimations, waitForFixture } from './electron-test-environment.mjs';

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
  const fixture = 'window.sessionOptimizationFixture';
  const search = 'document.querySelector(".session-picker input")';
  const list = 'document.querySelector(".session-picker-list")';
  const transcript = 'document.getElementById("restored-transcript").textContent';
  const preview = (requestId, text, mode = 'code') => js(`${fixture}.preview(${JSON.stringify(requestId)}, ${JSON.stringify(text)}, ${JSON.stringify(mode)})`);
  async function press(keyCode, modifiers = []) {
    web.sendInputEvent({ type: 'keyDown', keyCode, modifiers }); web.sendInputEvent({ type: 'keyUp', keyCode, modifiers });
    await settle();
  }
  async function query(value) {
    await js(`(() => { const input = ${search}; Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, ${JSON.stringify(value)}); input.dispatchEvent(new Event('input', { bubbles: true })); })()`);
    await settle();
  }
  async function geometry() {
    return js(`(() => {
      const input = ${search}, active = document.getElementById(input.getAttribute('aria-activedescendant'));
      const options = [...document.querySelectorAll('.session-picker-row')];
      return { mounted: options.length, activeMounted: Boolean(active), focused: input === document.activeElement,
        rowHeights: [...new Set(options.map(row => row.getBoundingClientRect().height))],
        clipped: options.some(row => row.querySelector('.session-picker-row-copy').getBoundingClientRect().height > row.getBoundingClientRect().height - 22),
        total: active?.getAttribute('aria-setsize'), position: active?.getAttribute('aria-posinset'), scrollTop: ${list}.scrollTop };
    })()`);
  }
  try {
    await win.loadURL(url); await configureFixtureWindow(win);
    await wait(`${fixture}?.state().ready`); await settle();
    const stages = [];
    for (const stage of ['empty', 'pending', 'authoritative']) {
      if (stage === 'pending') { await js(`${fixture}.submit()`); await wait(`${fixture}.state().title === 'Pending catalog title'`); }
      if (stage === 'authoritative') { await js(`${fixture}.save()`); await wait(`${fixture}.state().title === 'Authoritative saved title'`); }
      await settle();
      const before = await js(`${fixture}.state()`);
      await js(`${fixture}.stream(30)`); await settle();
      const after = await js(`${fixture}.state()`);
      assert.equal(after.catalogChanges, before.catalogChanges, stage + ' overlay projection changed while streaming');
      assert.equal(after.identityReads, before.identityReads, stage + ' repeated a catalog scan while streaming');
      assert.ok(after.renders - before.renders >= 20, stage + ' did not exercise separate rendered updates');
      stages.push(stage);
    }

    await js('document.getElementById("open-picker").click()');
    await wait(`Boolean(${search} && document.querySelector('.session-picker-row[aria-setsize="5002"]'))`);
    await settleFixtureAnimations(web, '.session-picker');
    const initial = await geometry();
    assert.equal(initial.total, '5002'); assert.equal(initial.activeMounted, true); assert.equal(initial.focused, true);
    assert.ok(initial.mounted <= 15); assert.deepEqual(initial.rowHeights, [66]);
    assert.match(await js('document.querySelector(".session-picker-count").textContent'), /5002/);

    // Narrow/zoomed layout must retain the exact height used by the window math.
    win.setSize(500, 700); web.setZoomFactor(1.5); await settle();
    const narrow = await geometry();
    assert.deepEqual(narrow.rowHeights, [66]); assert.equal(narrow.clipped, false);
    assert.equal(narrow.activeMounted, true); assert.equal(narrow.focused, true);
    web.setZoomFactor(1); win.setSize(1200, 800); await settle();

    await js(`${list}.scrollTop = 66 * 2500; ${list}.dispatchEvent(new Event('scroll', { bubbles: true }))`); await settle();
    const manual = await geometry();
    assert.ok(manual.scrollTop > 160000); assert.equal(manual.activeMounted, true); assert.equal(manual.position, '1');
    assert.ok(manual.mounted <= 15); assert.equal(manual.focused, true);
    await settle(); assert.equal((await geometry()).scrollTop, manual.scrollTop, 'mounted active descendant must not pull manual scrolling back');
    await js(`document.querySelector('.session-picker-row[aria-posinset="2501"]').dispatchEvent(new PointerEvent('pointermove', { bubbles: true }))`); await settle();
    assert.equal((await geometry()).position, '2501'); assert.equal((await geometry()).focused, true);
    for (let index = 0; index < 24; index++) await press('Down');
    const navigated = await geometry();
    assert.equal(navigated.position, '2525'); assert.equal(navigated.activeMounted, true); assert.equal(navigated.focused, true); assert.ok(navigated.mounted <= 15);

    await query('History 04999');
    await wait(`document.querySelectorAll('.session-picker-row').length === 1`);
    assert.equal((await geometry()).total, '1', 'search must include rows outside the mounted window');
    await query(''); await wait(`document.querySelector('.session-picker-row[aria-setsize="5002"]')`);
    await press('A', ['control']);
    await wait(`Boolean([...document.querySelectorAll('.session-picker-scopes button')].find(button => button.textContent === 'All' && button.getAttribute('aria-pressed') === 'true'))`);
    await wait(`document.querySelector('.session-picker-row[aria-setsize="5002"]')`);
    await js(`${list}.scrollTop = ${list}.scrollHeight; ${list}.dispatchEvent(new Event('scroll', { bubbles: true }))`); await settle();
    await wait(`Boolean([...document.querySelectorAll('.session-picker-row')].find(row => row.title === ${fixture}.state().chosenPath))`);
    await js(`([...document.querySelectorAll('.session-picker-row')].find(row => row.title === ${fixture}.state().chosenPath)).click()`);
    await wait(`${fixture}.state().opened.length === 1 && !${search}`);
    const opened = (await js(`${fixture}.state()`)).opened[0];
    assert.deepEqual(opened, { projectPath: '/fixture/session-optimization', sessionId: '00000000-0000-4000-8000-000000001770',
      selection: { scope: 'all', transcriptPath: '/fixture/Exact Case/中文 chosen copy.jsonl' } });
    assert.equal(await js('document.activeElement.id'), 'open-picker');
    await js(`${fixture}.openDeferred(1)`);
    await wait(`${fixture}.state().sessionLoading && ${fixture}.state().pending.length === 1`);
    const [firstRequest] = (await js(`${fixture}.state()`)).pending;
    await js(`${fixture}.openDeferred(2)`);
    await wait(`${fixture}.state().pending.length === 2`);
    const [, secondRequest] = (await js(`${fixture}.state()`)).pending;
    const secondId = '00000000-0000-4000-8000-000000000002';
    await preview(firstRequest, 'Stale preview from the first navigation');
    await preview(secondRequest, 'Wrong mode preview', 'chat');
    await settle();
    assert.doesNotMatch(await js(transcript), /Stale preview|Wrong mode/);
    await js(`${fixture}.requestApproval(${JSON.stringify(secondId)}, 41)`);
    await wait(`${fixture}.state().pendingPermission === 41`);
    await preview(secondRequest, 'Read-only preview for the active navigation');
    await wait(`${transcript}.includes('Read-only preview for the active navigation')`);
    const previewState = await js(`${fixture}.state()`);
    assert.equal(previewState.ready, false);
    assert.equal(previewState.sessionLoading, true);
    assert.equal(previewState.authoritativeItems, 0);
    assert.equal(previewState.pendingPermission, 41);
    await js(`${fixture}.submit()`);
    assert.deepEqual((await js(`${fixture}.state()`)).sent, previewState.sent);
    await js(`${fixture}.approve(41)`);
    assert.deepEqual((await js(`${fixture}.state()`)).approved, [{ sessionId: secondId, requestId: 41 }]);
    await js(`${fixture}.resume(${JSON.stringify(secondId)}, 'Authoritative replay after resume')`);
    await wait(`${fixture}.state().authoritativeItems === 1`);
    assert.doesNotMatch(await js(transcript), /Authoritative replay/);
    await js(`${fixture}.completeOpen(${JSON.stringify(secondRequest)})`);
    await wait(`!${fixture}.state().sessionLoading && ${fixture}.state().activeSessionId === ${JSON.stringify(secondId)}`);
    const finalTranscript = await js(transcript);
    assert.doesNotMatch(finalTranscript, /Read-only preview/);
    assert.equal(finalTranscript.match(/Authoritative replay after resume/g)?.length, 1);
    await js(`${fixture}.completeOpen(${JSON.stringify(firstRequest)})`); await settle();
    assert.equal((await js(`${fixture}.state()`)).activeSessionId, secondId);

    await js(`${fixture}.openDuplicate(false)`);
    await wait(`${fixture}.state().pending.length === 1`);
    const [olderCopy] = (await js(`${fixture}.state()`)).pending;
    await js(`${fixture}.openDuplicate(true)`);
    await wait(`${fixture}.state().pending.length === 2`);
    const [, selectedCopy] = (await js(`${fixture}.state()`)).pending;
    await preview(olderCopy, 'Another copy of the same session');
    await settle();
    assert.doesNotMatch(await js(transcript), /Another copy|Authoritative replay/);
    await preview(selectedCopy, 'Exactly the selected copy');
    await wait(`${transcript}.includes('Exactly the selected copy')`);
    await js(`${fixture}.completeOpen(${JSON.stringify(selectedCopy)})`);
    await js(`${fixture}.completeOpen(${JSON.stringify(olderCopy)})`);
    await wait(`!${fixture}.state().sessionLoading`);
    assert.doesNotMatch(await js(transcript), /Exactly the selected copy|Another copy/);
    process.stdout.write(JSON.stringify({ stages, streamedUpdates: 90, fullResults: 5002, maximumMounted: Math.max(initial.mounted, manual.mounted, navigated.mounted),
      activeDescendant: true, manualScroll: true, keyboard: true, narrowZoom: true, exactSource: true, overlappingNavigation: true, progressiveResume: true }) + '\n');
  } finally { win.destroy(); app.quit(); }
}
main().catch(error => { process.stderr.write(String(error.stack) + '\n'); app.exit(1); });
