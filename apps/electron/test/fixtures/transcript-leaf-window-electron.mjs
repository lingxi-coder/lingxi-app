import { configureFixtureWindow, waitForFixture } from './electron-test-environment.mjs';
import { app, BrowserWindow } from 'electron';
import assert from 'node:assert/strict';

const url = process.argv.find(argument => argument.startsWith('http://'));
if (!url || !process.env.LINGXI_TEST_USER_DATA) throw new Error('isolated fixture URL and user data required');
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function main() {
  await app.whenReady();
  const win = new BrowserWindow({ show: false, width: 1000, height: 760, webPreferences: { sandbox: true, backgroundThrottling: false } });
  const run = code => win.webContents.executeJavaScript(code);
  const settle = () => delay(160);
  const wait = async (code, message) => {
    const deadline = Date.now() + 6000;
    while (!await run(code)) { if (Date.now() > deadline) throw new Error(message); await delay(30); }
    await settle();
  };
  const visibleLeaf = `(() => { const stage = document.querySelector('.desktop-stage').getBoundingClientRect(); const leaf = [...document.querySelectorAll('[data-transcript-leaf-id]')].find(leaf => { const r = leaf.getBoundingClientRect(); return r.bottom > stage.top + 1 && r.top < stage.bottom; }); return leaf ? { id: leaf.dataset.transcriptLeafId, offset: leaf.getBoundingClientRect().top - stage.top } : null; })()`;
  const reveal = async id => {
    await run(`window.leafFixture.reveal('uuid-${id}')`);
    await wait('window.leafFixture.located() === true', `UUID ${id} did not settle`);
    const view = await run(`(() => { const stage = document.querySelector('.desktop-stage'); const leaf = document.querySelector('[data-transcript-leaf-id="${id}"]'); const r = leaf?.getBoundingClientRect(); const s = stage.getBoundingClientRect(); return { visible: Boolean(r && r.top < s.bottom && r.bottom > s.top), leafTop: r?.top, stageTop: s.top, stageBottom: s.bottom, scrollTop: stage.scrollTop, first: document.querySelector('[data-transcript-leaf-id]')?.dataset.transcriptLeafId }; })()`);
    assert.ok(view.visible, `UUID ${id} reveals the individual leaf: ${JSON.stringify(view)}`);
  };
  let maxMounted = 0, maxSites = 0;
  const bounded = async () => {
    const count = await run('document.querySelectorAll("[data-transcript-leaf-id]").length');
    const sites = await run('window.leafFixture.sites().length');
    maxMounted = Math.max(maxMounted, count); maxSites = Math.max(maxSites, sites);
    assert.ok(count < 100, `only viewport leaves mount: ${count}`);
    assert.ok(sites < 200, `native leaf sites are bounded: ${sites}`);
  };
  try {
    await win.loadURL(url);
    await configureFixtureWindow(win);
    await waitForFixture(win.webContents, 'Boolean(window.leafFixture && document.querySelector(".desktop-stage"))');
    await settle();
    assert.equal(await run('document.querySelector(".desktop-stage-feed").dataset.windowed'), undefined, 'the large activity is below the outer threshold');
    await reveal('tool-600');
    await bounded();
    assert.equal(await run('document.querySelector("[data-leaf-windowed] [role=listitem]").getAttribute("aria-setsize")'), '1650');
    await run('document.querySelector("[data-transcript-leaf-id=tool-600] .tool-disclosure-trigger").click()');
    await settle();
    assert.equal(await run('document.querySelector("[data-transcript-leaf-id=tool-600] .tool-disclosure-trigger").getAttribute("aria-expanded")'), 'true');
    const beforePrepend = await run(visibleLeaf);
    await run('window.leafFixture.prepend()');
    await settle();
    const afterPrepend = await run(visibleLeaf);
    assert.equal(afterPrepend?.id, beforePrepend?.id, 'prepend/merged activity keeps the individual reading leaf');
    assert.ok(Math.abs(afterPrepend.offset - beforePrepend.offset) <= 2, `prepend anchor drift: ${JSON.stringify({ beforePrepend, afterPrepend })}`);
    // The native scroll and ResizeObserver queues must cooperate while paging
    // changes the aggregate ID and replaces its independently measured window.
    const point = await run(`(() => { const r = document.querySelector('.desktop-stage').getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + 180 }; })()`);
    await win.webContents.debugger.sendCommand('Input.dispatchMouseEvent', { type: 'mouseWheel', ...point, deltaX: 0, deltaY: -6 });
    await run('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
    const wheelHeld = await run(visibleLeaf);
    await run('window.leafFixture.prepend()');
    const wheelFrames = await run(`new Promise(resolve => {
      const frames = [];
      const sample = () => requestAnimationFrame(() => setTimeout(() => {
        const leaf = document.querySelector('[data-transcript-leaf-id="${wheelHeld.id}"]');
        frames.push(leaf ? leaf.getBoundingClientRect().top - document.querySelector('.desktop-stage').getBoundingClientRect().top : null);
        if (frames.length < 4) sample(); else resolve(frames);
      }, 0));
      sample();
    })`);
    const wheelPrependDrift = Math.max(...wheelFrames.map(offset => offset === null ? Infinity : Math.abs(offset - wheelHeld.offset)));
    assert.ok(wheelPrependDrift <= 1, `wheel paging preserves the merged leaf in every painted frame: ${JSON.stringify({ wheelHeld, wheelFrames })}`);
    await settle();
    const afterWheelPrepend = await run(visibleLeaf);
    await run('window.leafFixture.stream()'); await settle();
    const afterStream = await run(visibleLeaf);
    assert.equal(afterStream?.id, afterWheelPrepend?.id, 'detached streaming keeps the reading leaf');
    assert.ok(Math.abs(afterStream.offset - afterWheelPrepend.offset) <= 2, 'detached stream keeps its reading offset');
    await bounded();
    // Recycling never discards disclosure choices or the logical history.
    for (const fraction of [0, 0.2, 0.5, 0.8, 1]) {
      await run(`(() => { const s = document.querySelector('.desktop-stage'); s.scrollTop = (s.scrollHeight - s.clientHeight) * ${fraction}; })()`);
      await settle(); await bounded();
      assert.ok(await run(visibleLeaf) || fraction === 1, `scroll fraction ${fraction} reveals real leaves`);
    }
    await run('(() => { const s = document.querySelector(".desktop-stage"); s.scrollTop = (s.scrollHeight - s.clientHeight) * 0.97; })()');
    await settle();
    const beforeReasoningPrepend = await run(visibleLeaf);
    assert.ok(beforeReasoningPrepend.id.startsWith('thought-'), 'the reasoning anchor has no tool UUID');
    await run('window.leafFixture.prepend()'); await settle();
    const afterReasoningPrepend = await run(visibleLeaf);
    assert.equal(afterReasoningPrepend.id, beforeReasoningPrepend.id, 'reasoning-only leaves preserve their stable-ID anchor across merged prepends');
    const reasoningPrependDrift = Math.abs(afterReasoningPrepend.offset - beforeReasoningPrepend.offset);
    assert.ok(reasoningPrependDrift <= 2, `reasoning prepend moved ${reasoningPrependDrift}px`);
    let beforeResize;
    for (let attempt = 0; attempt < 6; attempt++) {
      beforeResize = await run('(() => { window.leafReadingRange = window.leafFixture.textAnchor(); return window.leafReadingRange?.getBoundingClientRect().top - document.querySelector(".desktop-stage").getBoundingClientRect().top; })()');
      if (Number.isFinite(beforeResize)) break;
      // A paragraph margin is not a reading character. Move into the next
      // line before testing character preservation under width reflow.
      await run('document.querySelector(".desktop-stage").scrollTop += 8'); await settle();
    }
    assert.ok(Number.isFinite(beforeResize), 'the reading position has a real text range');
    win.setSize(760, 760); await settle();
    const afterResize = await run('(() => { if (!window.leafReadingRange?.startContainer.isConnected) return null; return window.leafReadingRange.getBoundingClientRect().top - document.querySelector(".desktop-stage").getBoundingClientRect().top; })()');
    assert.ok(Number.isFinite(afterResize), 'resizing keeps the visible reasoning text mounted');
    const resizeDrift = Math.abs(afterResize - beforeResize);
    assert.ok(resizeDrift <= 2, `reasoning resize moved the reading character ${resizeDrift}px`);
    win.setSize(1000, 760); await settle();
    await reveal('tool-600');
    assert.equal(await run('document.querySelector("[data-transcript-leaf-id=tool-600] .tool-disclosure-trigger").getAttribute("aria-expanded")'), 'true');
    await run('document.querySelector(".transcript-activity-trigger").click()'); await settle();
    assert.equal(await run('document.querySelectorAll("[data-transcript-leaf-id]").length'), 0, 'collapse retires every leaf');
    assert.equal(await run('window.leafFixture.sites().length'), 0, 'collapse retires every leaf native site');
    await run('document.querySelector(".transcript-activity-trigger").click()'); await settle();
    await reveal('tool-600'); await bounded();
    assert.equal(await run('document.querySelector("[data-transcript-leaf-id=tool-600] .tool-disclosure-trigger").getAttribute("aria-expanded")'), 'true', 'collapse/reopen retains child disclosure');
    await run('document.querySelector(".desktop-stage").focus({ preventScroll: true })');
    const beforeKey = await run('document.querySelector(".desktop-stage").scrollTop');
    win.webContents.sendInputEvent({ type: 'keyDown', keyCode: 'PageUp' }); win.webContents.sendInputEvent({ type: 'keyUp', keyCode: 'PageUp' });
    await settle();
    assert.ok(await run('document.querySelector(".desktop-stage").scrollTop') < beforeKey, 'keyboard scroll uses the transcript viewport');
    await run('document.querySelector(".desktop-stage-jump").click()'); await settle();
    for (let index = 0; index < 4; index++) { await run('window.leafFixture.stream()'); await delay(40); }
    await settle();
    const tailGap = await run('(() => { const s = document.querySelector(".desktop-stage"); return s.scrollHeight - s.clientHeight - s.scrollTop; })()');
    assert.ok(tailGap <= 1, `streaming keeps the transcript tail: ${tailGap}`);
    assert.ok(await run('document.querySelector("[data-transcript-leaf-id=thought-149]") !== null'), 'the reasoning tail also uses the leaf window');
    await bounded();
    await run('window.leafFixture.loadLong()'); await settle();
    await reveal('tool-900'); await bounded();
    assert.equal(await run('document.querySelector(".desktop-stage-feed").dataset.windowed'), 'true', 'leaf and outer windows compose');
    assert.ok(await run('window.leafFixture.retired().some(request => /^tool-use:/.test(request.instance_id) && Number.isFinite(request.render_revision))'), 'leaf recycling performs revision-aware native cleanup');
    await run('window.leafFixture.directGroup()'); await settle();
    for (const fraction of [0, 0.5, 1]) {
      await run(`(() => { const s = document.querySelector('.desktop-stage'); s.scrollTop = (s.scrollHeight - s.clientHeight) * ${fraction}; })()`);
      await settle(); await bounded();
      assert.ok(await run(visibleLeaf), 'standalone ToolGroup scroll reveals actual leaves');
    }
    assert.equal(await run('document.querySelectorAll(".desktop-stage").length'), 1, 'there is one transcript scrollport');
    process.stdout.write(`${JSON.stringify({ maxMounted, maxSites, tailGap, reasoningPrependDrift, resizeDrift, wheelPrependDrift, prependDrift: Math.abs(afterPrepend.offset - beforePrepend.offset) })}\n`);
  } finally { win.destroy(); app.quit(); }
}
main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); app.exit(1); });
