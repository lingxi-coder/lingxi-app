import { configureFixtureWindow, waitForFixture } from './electron-test-environment.mjs';
import { app, BrowserWindow } from 'electron';
import assert from 'node:assert/strict';

const url = process.argv.find(argument => argument.startsWith('http://'));
if (!url || !process.env.LINGXI_TEST_USER_DATA) throw new Error('isolated fixture URL and user data required');
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 1000, height: 760, webPreferences: { sandbox: true, backgroundThrottling: false } });
  try {
    await window.loadURL(url);
    await configureFixtureWindow(window);
    await waitForFixture(window.webContents, 'Boolean(window.transcriptFixture && document.querySelector(".desktop-stage"))');
    const run = script => window.webContents.executeJavaScript(script);
    const settle = () => delay(160);
    const wait = async (expression, message) => {
      const deadline = Date.now() + 6000;
      while (!await run(expression)) {
        if (Date.now() > deadline) throw new Error(message);
        await delay(30);
      }
      await settle();
    };
    const click = async selector => {
      await run(`document.querySelector(${JSON.stringify(selector)}).scrollIntoView({ block: 'center' })`);
      await settle();
      const point = await run(`(() => { const rect = document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect(); return { x: Math.round(rect.left + rect.width / 2), y: Math.round(rect.top + rect.height / 2) }; })()`);
      window.webContents.sendInputEvent({ type: 'mouseMove', ...point });
      window.webContents.sendInputEvent({ type: 'mouseDown', ...point, button: 'left', clickCount: 1 });
      window.webContents.sendInputEvent({ type: 'mouseUp', ...point, button: 'left', clickCount: 1 });
      await settle();
    };
    await settle();
    const isTargetVisible = uuid => `(() => { const row = document.querySelector('[data-message-uuid="${uuid}"]'); if (!row) return false; const stage = document.querySelector('.desktop-stage').getBoundingClientRect(); const rect = row.getBoundingClientRect(); return rect.bottom > stage.top && rect.top < stage.bottom; })()`;
    await wait('window.transcriptFixture.located() === true', 'initial offscreen UUID target did not materialize');
    const initialTargetRevealed = await run(isTargetVisible('uuid-25'));
    assert.equal(initialTargetRevealed, true, 'initial target must remain visible after location completes');
    await run(`window.transcriptFixture.load(500, 'switch-target', 'uuid-row-120-b')`);
    await wait('window.transcriptFixture.located() === true', 'same-render session-switch target did not materialize');
    const switchedTargetRevealed = await run(isTargetVisible('uuid-row-120-b'));
    assert.equal(switchedTargetRevealed, true, 'same-render session target must remain visible after location completes');
    await run(`window.transcriptFixture.load(500, 'switch-folded-target', 'uuid-row-120-b', true)`);
    await wait('window.transcriptFixture.located() === true', 'session-switch target inside a closed fold did not materialize');
    const foldedTargetRevealed = await run(isTargetVisible('uuid-row-120-b'));
    assert.equal(foldedTargetRevealed, true, 'fold expansion must retain target scroll ownership until location');
    await run(`window.transcriptFixture.load(500, 'switch-collapsed-target', 'uuid-400')`);
    await wait('window.transcriptFixture.located() === true', 'session-switch target inside a collapsed message did not materialize');
    const collapsedTargetRevealed = await run(isTargetVisible('uuid-400'));
    assert.equal(collapsedTargetRevealed, true, 'message disclosure expansion must retain target scroll ownership');
    const atBottom = `(() => { const stage = document.querySelector('.desktop-stage'); return stage.scrollHeight - stage.clientHeight - stage.scrollTop <= 1; })()`;
    await run('window.transcriptFixture.sendPrompt()');
    await settle();
    const tailAfterTarget = await run(atBottom);
    assert.equal(tailAfterTarget, true, 'a retained successful target must not prevent a later prompt following the tail');
    await run(`window.transcriptFixture.reveal('missing-uuid')`);
    await wait('window.transcriptFixture.located() === false', 'missing target did not settle');
    await run('window.transcriptFixture.sendPrompt()');
    await settle();
    const tailAfterMissingTarget = await run(atBottom);
    assert.equal(tailAfterMissingTarget, true, 'a missing target must release pending reveal ownership');
    await run(`window.transcriptFixture.load(500, 'no-target')`);
    await settle();
    const sessionWithoutTargetAtTail = await run(atBottom);
    assert.equal(sessionWithoutTargetAtTail, true, 'a session without a target still opens at the tail');
    await run(`window.transcriptFixture.load(12, 'short')`);
    await settle();
    const groupCount = `window.transcriptFixture.renders().filter(entry => entry.sessionId === 'short' && entry.instance.startsWith('tool-group:')).length`;
    const before = await run(groupCount);
    for (const phase of ['preparing', 'summarizing', 'restoring']) {
      await run(`window.transcriptFixture.phase(${JSON.stringify(phase)})`);
      await settle();
    }
    await run('window.transcriptFixture.settleCompact()');
    await settle();
    const nativeHistoricalRenders = await run(groupCount) - before;
    await run('window.transcriptFixture.updateTool()');
    await settle();
    const toolUpdated = await run(`window.transcriptFixture.renders().some(entry => entry.instance === 'tool-group:tool-group:row-10-a' && entry.props.calls?.some(call => call.isErrored))`);
    await click('[data-tool-group-id="tool-group:row-10-a"] .tool-group-trigger');
    assert.equal(await run(`document.querySelector('[data-tool-group-id="tool-group:row-10-a"] .tool-group-trigger').getAttribute('aria-expanded')`), 'true');
    await run(`window.transcriptFixture.plan('# Initial plan')`);
    await settle();
    await run(`window.transcriptFixture.plan('# Updated plan content')`);
    await settle();
    const planUpdated = await run(`document.querySelector('.desktop-stage').textContent.includes('Updated plan content')`);

    await run(`window.transcriptFixture.load(160, 'threshold')`);
    await settle();
    await run(`document.querySelector('.desktop-stage').scrollTop = 2400`);
    await settle();
    const thresholdProbe = `(() => { const top = document.querySelector('.desktop-stage').getBoundingClientRect().top; const row = [...document.querySelectorAll('[data-message-uuid]')].find(row => row.getBoundingClientRect().bottom > top); return { uuid: row.dataset.messageUuid, offset: row.getBoundingClientRect().top - top }; })()`;
    const thresholdBefore = await run(thresholdProbe);
    await run(`window.transcriptFixture.append()`);
    await settle();
    const thresholdAfter = await run(thresholdProbe);
    assert.equal(thresholdAfter.uuid, thresholdBefore.uuid, 'enabling windowing while backscrolling preserves the historical row being read');
    assert.ok(Math.abs(thresholdAfter.offset - thresholdBefore.offset) <= 1, 'enabling windowing preserves the exact reading offset');

    await run('window.transcriptFixture.load(2000)');
    await settle();
    await run('window.originalLogicalHistory = window.transcriptFixture.history()');
    const mounted = `document.querySelectorAll('[data-transcript-id]').length`;
    let maxMounted = await run(mounted);
    assert.ok(await run(`document.querySelector('[role="list"]') && document.querySelector('[role="listitem"]').getAttribute('aria-setsize') === '2000'`), 'virtual history exposes its logical size to assistive technology');
    for (const fraction of [0, 0.25, 0.5, 0.75, 1, 0.4]) {
      await run(`(() => { const stage = document.querySelector('.desktop-stage'); stage.scrollTop = (stage.scrollHeight - stage.clientHeight) * ${fraction}; })()`);
      await settle();
      maxMounted = Math.max(maxMounted, await run(mounted));
      const view = await run(`(() => { const stage = document.querySelector('.desktop-stage'); const bounds = stage.getBoundingClientRect(); const rows = [...document.querySelectorAll('[data-transcript-id]')]; return { visible: rows.some(node => { const rect = node.getBoundingClientRect(); return rect.bottom > bounds.top && rect.top < bounds.bottom; }), top: stage.scrollTop, height: stage.scrollHeight, first: rows[0]?.dataset.transcriptIndex, last: rows.at(-1)?.dataset.transcriptIndex, firstTop: rows[0]?.getBoundingClientRect().top, lastBottom: rows.at(-1)?.getBoundingClientRect().bottom }; })()`);
      assert.ok(view.visible, `scrolling must reveal actual history, not a blank spacer (${fraction}): ${JSON.stringify(view)}`);
    }
    await run(`window.transcriptFixture.reveal('uuid-399')`);
    await wait('window.transcriptFixture.located() === true', 'historical UUID did not materialize');
    const disclosure = '[data-transcript-id="row-400"] .narration-disclosure-trigger';
    assert.equal(await run(`document.querySelector(${JSON.stringify(disclosure)}).getAttribute('aria-expanded')`), 'false');
    await click(disclosure);
    assert.equal(await run(`document.querySelector(${JSON.stringify(disclosure)}).getAttribute('aria-expanded')`), 'true');
    await run(`(() => { const stage = document.querySelector('.desktop-stage'); stage.scrollTop = 0; })()`);
    await settle();
    assert.equal(await run(`Boolean(document.querySelector('[data-transcript-id="row-0"]'))`), true, 'oldest message remains available');
    await run(`window.transcriptFixture.reveal('uuid-600')`);
    await wait('window.transcriptFixture.located() === true', 'middle UUID did not materialize');
    await run(`window.transcriptFixture.reveal('uuid-399')`);
    await wait('window.transcriptFixture.located() === true', 'repeated historical reveal failed');
    const disclosureRetained = await run(`document.querySelector(${JSON.stringify(disclosure)}).getAttribute('aria-expanded') === 'true'`);
    const historyRetained = await run('window.originalLogicalHistory === window.transcriptFixture.history()');

    await run(`window.transcriptFixture.reveal('uuid-600')`);
    await wait('window.transcriptFixture.located() === true', 'detached reading target missing');
    const anchor = `(() => { const stage = document.querySelector('.desktop-stage'); const top = stage.getBoundingClientRect().top; const row = [...document.querySelectorAll('[data-transcript-id]')].find(row => row.getBoundingClientRect().bottom > top); return { id: row.dataset.transcriptId, offset: row.getBoundingClientRect().top - top }; })()`;
    const anchorBefore = await run(anchor);
    await run('window.transcriptFixture.append(); window.transcriptFixture.delta()');
    await settle();
    const anchorAfter = await run(anchor);
    assert.equal(anchorAfter.id, anchorBefore.id, 'append/delta preserves the row being read');
    const detachedDrift = Math.abs(anchorAfter.offset - anchorBefore.offset);
    window.setSize(760, 760);
    await settle();
    const resized = await run(anchor);
    assert.equal(resized.id, anchorAfter.id, 'resizing a long transcript preserves the row being read');
    const resizeDrift = Math.abs(resized.offset - anchorAfter.offset);
    window.setSize(1000, 760);
    await settle();
    await run(`document.querySelector('.desktop-stage').focus({ preventScroll: true })`);
    const topBeforeKey = await run('document.querySelector(".desktop-stage").scrollTop');
    window.webContents.sendInputEvent({ type: 'keyDown', keyCode: 'PageUp' });
    window.webContents.sendInputEvent({ type: 'keyUp', keyCode: 'PageUp' });
    await settle();
    const keyboardScrolled = await run('document.querySelector(".desktop-stage").scrollTop') < topBeforeKey;
    await click('.desktop-stage-jump');
    for (let index = 0; index < 8; index++) { await run('window.transcriptFixture.delta()'); await delay(30); }
    await settle();
    const tailGap = await run(`(() => { const stage = document.querySelector('.desktop-stage'); return stage.scrollHeight - stage.clientHeight - stage.scrollTop; })()`);
    maxMounted = Math.max(maxMounted, await run(mounted));
    await run(`window.transcriptFixture.reveal('uuid-25')`);
    await wait('window.transcriptFixture.located() === true', 'offscreen reveal after streaming failed');
    const revealed = await run(`(() => { const row = document.querySelector('[data-message-uuid="uuid-25"]'); const bounds = document.querySelector('.desktop-stage').getBoundingClientRect(); const rect = row.getBoundingClientRect(); return rect.bottom > bounds.top && rect.top < bounds.bottom; })()`);
    await run(`window.transcriptFixture.reveal('uuid-row-120-b')`);
    await wait('window.transcriptFixture.located() === true', 'grouped tool UUID alias did not materialize');
    assert.ok(await run(`Boolean(document.querySelector('[data-message-uuid="uuid-row-120-b"] [data-tool-group-id="tool-group:row-120-a"]'))`), 'a grouped tool result reveals its containing history row');

    await run(`window.transcriptFixture.load(500, 'second')`);
    await settle();
    await run(`window.transcriptFixture.reveal('uuid-399')`);
    await wait('window.transcriptFixture.located() === true', 'session-switch reveal failed');
    const sessionClean = await run(`document.querySelector('[data-transcript-id="row-400"] .narration-disclosure-trigger').getAttribute('aria-expanded') === 'false' && document.querySelector('.desktop-stage').textContent.includes('second row 399')`);
    const draft = await run('document.querySelector("textarea").value');
    assert.ok(await run(`window.transcriptFixture.unmounts().some(request => request.type === 'draw_unmount' && Number.isFinite(request.render_revision))`), 'virtualized native surfaces use revision-aware cleanup');
    process.stdout.write(`${JSON.stringify({ maxMounted, nativeHistoricalRenders, toolUpdated, planUpdated, disclosureRetained, historyRetained, detachedDrift, resizeDrift, keyboardScrolled, tailGap, revealed, sessionClean, initialTargetRevealed, switchedTargetRevealed, foldedTargetRevealed, collapsedTargetRevealed, tailAfterTarget, tailAfterMissingTarget, sessionWithoutTargetAtTail, draft })}\n`);
  } finally { window.destroy(); app.quit(); }
}
main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); app.exit(1); });
