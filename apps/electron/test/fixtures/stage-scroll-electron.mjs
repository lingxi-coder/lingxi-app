import { configureFixtureWindow, waitForFixture } from './electron-test-environment.mjs';
import { app, BrowserWindow } from 'electron';
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
const url = process.argv.find((argument) => argument.startsWith('http://'));
if (!url || !process.env.LINGXI_TEST_USER_DATA) throw new Error('fixture URL and isolated user data required');
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 1000, height: 800, webPreferences: { sandbox: true, backgroundThrottling: false } });
  try {
    await window.loadURL(url);
    await configureFixtureWindow(window);
    const run = (script) => window.webContents.executeJavaScript(script);
    const settle = () => delay(100);
    const deadline = Date.now() + 6000;
    while (!await run('Boolean(window.stageScrollFixture && document.querySelector(".transcript-thinking"))')) {
      if (Date.now() > deadline) throw new Error('Stage fixture did not render');
      await delay(20);
    }
    await settle();
    const metrics = `(() => { const node = document.querySelector('.desktop-stage'); return { top: node.scrollTop, gap: node.scrollHeight - node.clientHeight - node.scrollTop, ancestor: document.querySelector('#scroll-ancestor').scrollTop }; })()`;
    const initial = await run(metrics);
    const result = { initialGap: initial.gap, ancestorScroll: initial.ancestor };
    await run(`(() => { const node = document.querySelector('.desktop-stage'); node.dispatchEvent(new PointerEvent('pointerdown', {bubbles:true})); node.scrollTop = node.scrollHeight - node.clientHeight - 40; })()`);
    await settle();
    result.awayBefore = (await run(metrics)).top;
    await run('window.stageScrollFixture.insert()');
    await settle();
    result.dragAfter = (await run(metrics)).top;
    await run("window.dispatchEvent(new PointerEvent('pointerup', {bubbles:true}))");
    await run('window.stageScrollFixture.insert()');
    await settle();
    result.awayAfter = (await run(metrics)).top;
    await run(`(() => { const node = document.querySelector('.desktop-stage'); node.dispatchEvent(new PointerEvent('pointerdown', {bubbles:true})); node.scrollTop = node.scrollHeight; })()`);
    await settle();
    await run("window.dispatchEvent(new PointerEvent('pointerup', {bubbles:true}))");
    result.streamingGaps = [];
    result.immediateStreamingGaps = [];
    for (let i = 0; i < 5; i++) {
      result.immediateStreamingGaps.push(await run(`(() => { window.stageScrollFixture.insert(); return ${metrics}.gap; })()`));
      await settle();
      result.streamingGaps.push((await run(metrics)).gap);
    }
    result.thinkingLayouts = [];
    for (const visible of [true, false, true, false, true]) {
      await run(`window.stageScrollFixture.thinking(${visible})`);
      await settle();
      result.thinkingLayouts.push(await run(`(() => {
        const stage = document.querySelector('.desktop-stage');
        const slot = document.querySelector('[data-thinking-slot]');
        return { scrollHeight: stage.scrollHeight, scrollTop: stage.scrollTop, slotHeight: slot.getBoundingClientRect().height };
      })()`));
    }
    // Local child layout changes do not update Stage props (e.g. late markdown/media).
    await run(`document.querySelector('.transcript-thinking').style.height = '150px'`);
    await waitForFixture(window.webContents, `${metrics}.gap <= 1`);
    result.resizeGap = (await run(metrics)).gap;
    await run(`document.querySelector('.transcript-thinking').style.height = '20px'`);
    await waitForFixture(window.webContents, `${metrics}.gap <= 1`);
    result.shrinkGap = (await run(metrics)).gap;
    result.finalAncestorScroll = (await run(metrics)).ancestor;
    await run(`(() => { const style = document.createElement('style'); style.textContent = '* { animation: none !important; transition: none !important; }'; document.head.append(style); })()`);
    result.deliveryLayouts = [];
    for (const long of [false, true]) {
      for (const status of ['pending', undefined, 'failed', undefined]) {
        await run(`window.stageScrollFixture.delivery(${JSON.stringify(status)}, ${long})`);
        await settle();
        const alignment = await run(`(() => {
          const bubble = document.querySelector('.user-message-bubble').getBoundingClientRect();
          const row = document.querySelector('.transcript-user-message').getBoundingClientRect();
          return { rightGap: row.right - bubble.right, width: bubble.width, rowWidth: row.width };
        })()`);
        assert.ok(Math.abs(alignment.rightGap) <= 1, 'user bubble shares the right edge of its row');
        assert.ok(alignment.width < alignment.rowWidth, 'user bubble remains narrower than its row');
        result.deliveryLayouts.push(await run(`(() => {
          const bubble = document.querySelector('.user-message-bubble');
          const rect = bubble.getBoundingClientRect();
          return { width: rect.width, height: rect.height, top: rect.top, nextTop: document.getElementById('narration-content-after-delivery').getBoundingClientRect().top, scrollHeight: document.querySelector('.desktop-stage').scrollHeight };
        })()`));
        if (!long && status === 'pending' && process.env.LINGXI_DELIVERY_SCREENSHOTS) {
          await mkdir(process.env.LINGXI_DELIVERY_SCREENSHOTS, { recursive: true });
          await writeFile(process.env.LINGXI_DELIVERY_SCREENSHOTS + '/pending.png', (await window.webContents.capturePage()).toPNG());
        }
      }
    }
    // The clock/copy affordance under a user message reveals on hover. Its box
    // is reserved, so revealing it must not change the message's height — the
    // transcript would otherwise shift under the pointer mid-read.
    const messageBox = `(() => {
      const row = document.querySelector('.transcript-user-message');
      const bubble = row.querySelector('.user-message-bubble');
      const actions = row.querySelector('.user-message-actions');
      return {
        row: row.getBoundingClientRect().height,
        bubble: bubble.getBoundingClientRect().height,
        actions: actions.getBoundingClientRect().height,
        opacity: getComputedStyle(actions).opacity,
        clock: row.querySelector('.user-message-clock')?.textContent ?? '',
      };
    })()`;
    const messageCenter = await run(`(() => { const r = document.querySelector('.user-message-bubble').getBoundingClientRect(); return { x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2) }; })()`);
    window.focus();
    window.webContents.sendInputEvent({ type: 'mouseMove', x: 6, y: 6 });
    await settle();
    result.messageAway = await run(messageBox);
    window.webContents.sendInputEvent({ type: 'mouseMove', x: messageCenter.x, y: messageCenter.y });
    await settle();
    result.messageHover = await run(messageBox);
    if (process.env.LINGXI_DELIVERY_SCREENSHOTS) {
      await mkdir(process.env.LINGXI_DELIVERY_SCREENSHOTS, { recursive: true });
      await writeFile(process.env.LINGXI_DELIVERY_SCREENSHOTS + '/message-hover.png', (await window.webContents.capturePage()).toPNG());
    }
    // Click revealed buttons for real. TWO prompts are put on screen and the
    // click lands on the SECOND one: with a single row, "copied the right
    // message" and "copied the only message" are indistinguishable.
    await run('window.stageScrollFixture.twoPrompts()');
    await settle();
    await run(`document.querySelectorAll('.user-message-copy')[1].scrollIntoView({ block: 'center' })`);
    await settle();
    result.copyPoints = await run(`(() => [...document.querySelectorAll('.user-message-copy')].map((button) => { const r = button.getBoundingClientRect(); return { x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2) }; }))()`);
    const copyState = `(() => {
      const buttons = [...document.querySelectorAll('.user-message-copy')];
      const status = (button) => button.parentElement.querySelector('.user-message-actions-status')?.textContent ?? '';
      return {
        copied: window.copiedMessage ?? null,
        state: buttons[1].dataset.state,
        name: buttons[1].getAttribute('aria-label'), status: status(buttons[1]),
        neighbourState: buttons[0].dataset.state, neighbourStatus: status(buttons[0]),
      };
    })()`;
    const clickSecondCopy = async () => {
      const point = result.copyPoints[1];
      window.webContents.sendInputEvent({ type: 'mouseMove', x: point.x, y: point.y });
      // The move has to land (and re-enable `pointer-events` on the revealed
      // footer) before the press is hit-tested, or the click reaches the row.
      await settle();
      window.webContents.sendInputEvent({ type: 'mouseDown', x: point.x, y: point.y, button: 'left', clickCount: 1 });
      window.webContents.sendInputEvent({ type: 'mouseUp', x: point.x, y: point.y, button: 'left', clickCount: 1 });
      await settle();
    };
    // Poll for the reset rather than sleeping a fixed 1.5s: the reset IS the
    // assertion, so a fixed wait both assumes the timer fires on time and cannot
    // report how long it actually took.
    const waitForIdle = async (timeoutMs) => {
      const deadline = Date.now() + timeoutMs;
      let snapshot = await run(copyState);
      while (snapshot.state !== 'idle' && Date.now() < deadline) {
        await delay(50);
        snapshot = await run(copyState);
      }
      return snapshot;
    };

    result.copyBeforeClick = await run(copyState);
    await clickSecondCopy();
    result.copyClick = await run(copyState);
    result.copyClickReset = await waitForIdle(5_000);

    // The rejection branch. `copiedMessage` is cleared first, so "a failed copy
    // records nothing" is a real assertion rather than a repeat of the text the
    // success above already stored.
    await run('delete window.copiedMessage; window.stageScrollFixture.failCopies(true)');
    await clickSecondCopy();
    result.copyFailure = await run(copyState);
    result.copyFailureReset = await waitForIdle(5_000);
    await run('window.stageScrollFixture.failCopies(false)');

    // ── The jump-to-bottom control, and the band that decides it ──────────
    await run('window.stageScrollFixture.longList()');
    // Show the whole scrollport: the control is pinned to its bottom edge, so
    // it must be really on screen for the click below to be a real click.
    await run('window.stageScrollFixture.setAncestorHeight(700)');
    await settle();
    // Park at the true bottom, then leave it by hand.
    await run(`(() => { const node = document.querySelector('.desktop-stage'); node.scrollTop = node.scrollHeight; })()`);
    await settle();
    result.controlParked = await run('Boolean(document.querySelector(".desktop-stage-jump"))');
    await run(`(() => { const node = document.querySelector('.desktop-stage'); node.scrollTop = 0; })()`);
    await settle();
    result.controlAway = await run(`(() => {
      const stage = document.querySelector('.desktop-stage');
      const control = document.querySelector('.desktop-stage-jump');
      if (!control) return { present: false };
      const rect = control.getBoundingClientRect();
      const bounds = stage.getBoundingClientRect();
      return {
        present: true,
        // Really on screen, not merely inside the scrollport's box.
        onScreen: rect.top >= bounds.top - 1 && rect.bottom <= bounds.bottom + 1
          && rect.bottom <= window.innerHeight + 1 && rect.top >= -1,
        // Distance from the scrollport's bottom edge. Content-anchored would be
        // hundreds of pixels away here: the transcript is scrolled to its top.
        pinnedToBottom: Math.round(bounds.bottom - rect.bottom),
      };
    })()`);

    const jumpPoint = await run(`(() => {
      const control = document.querySelector('.desktop-stage-jump');
      const rect = control.getBoundingClientRect();
      return { x: Math.round(rect.left + rect.width / 2), y: Math.round(rect.top + rect.height / 2) };
    })()`);
    window.webContents.sendInputEvent({ type: 'mouseMove', x: jumpPoint.x, y: jumpPoint.y });
    await settle();
    window.webContents.sendInputEvent({ type: 'mouseDown', x: jumpPoint.x, y: jumpPoint.y, button: 'left', clickCount: 1 });
    window.webContents.sendInputEvent({ type: 'mouseUp', x: jumpPoint.x, y: jumpPoint.y, button: 'left', clickCount: 1 });
    await settle();
    result.jumpAfterClick = (await run(metrics)).gap;
    result.controlAfterJump = await run('Boolean(document.querySelector(".desktop-stage-jump"))');
    // The click re-engaged the tail, so the next arrival must be followed.
    await run('window.stageScrollFixture.insert()');
    await settle();
    result.jumpFollowGap = (await run(metrics)).gap;

    // ── The band: an exact fit is not required to keep following ──────────
    await run(`(() => { const node = document.querySelector('.desktop-stage'); node.scrollTop = node.scrollHeight - node.clientHeight - 20; })()`);
    await settle();
    await run('window.stageScrollFixture.insert()');
    await settle();
    result.bandFollowGap = (await run(metrics)).gap;

    // ── Sending re-engages the tail, without a timer ──────────────────────
    await run(`(() => { const node = document.querySelector('.desktop-stage'); node.scrollTop = 0; })()`);
    await settle();
    result.sendBeforeGap = (await run(metrics)).gap;
    await run('window.stageScrollFixture.sendPrompt()');
    await settle();
    result.sendAfterGap = (await run(metrics)).gap;

    // ── A resize keeps the row the reader is on exactly where it was ──────
    await run('window.stageScrollFixture.longList()');
    await settle();
    await run(`(() => { const node = document.querySelector('.desktop-stage'); node.scrollTop = 600; })()`);
    await settle();
    // The first row crossing the fold, named by its text so "the same row is
    // still under the reader" is asserted, not merely "some offset matched".
    const anchorProbe = `(() => {
      const node = document.querySelector('.desktop-stage');
      const rows = document.querySelector('.desktop-stage-feed').children;
      const top = node.getBoundingClientRect().top;
      for (const row of rows) {
        const rect = row.getBoundingClientRect();
        if (rect.bottom > top) return { text: row.textContent.slice(0, 40), offset: Math.round((rect.top - top) * 100) / 100 };
      }
      return null;
    })()`;
    result.anchorBeforeResize = await run(anchorProbe);
    // Narrowing the frame rewraps every row above the fold, so the reading
    // position survives only if the offset is compensated.
    await run('window.stageScrollFixture.setViewport(420, 560)');
    await settle();
    result.anchorAfterResize = await run(anchorProbe);

    await run('window.stageScrollFixture.readingWithInspector()');
    window.setSize(1400, 800);
    await settle();
    await run(`(() => {
      const stage = document.querySelector('.desktop-stage');
      const walker = document.createTreeWalker(document.querySelector('.desktop-stage-feed'), NodeFilter.SHOW_TEXT);
      let text;
      while ((text = walker.nextNode()) && !text.textContent.startsWith('Reading segment')) {}
      const range = document.createRange();
      range.setStart(text, 3500); range.setEnd(text, 3501);
      const lineTop = range.getBoundingClientRect().top;
      let start = 3500;
      while (start > 0) {
        range.setStart(text, start - 1); range.setEnd(text, start);
        if (range.getBoundingClientRect().top < lineTop) break;
        start -= 1;
      }
      range.setStart(text, start); range.setEnd(text, start + 1);
      stage.scrollTop += range.getBoundingClientRect().top - stage.getBoundingClientRect().top + 4;
      window.readingRange = range;
    })()`);
    await settle();
    const readingOffset = `window.readingRange.getBoundingClientRect().top - document.querySelector('.desktop-stage').getBoundingClientRect().top`;
    result.textResizeOffsets = [await run(readingOffset)];
    for (const width of [1200, 1101, 1099, 900, 1400]) {
      window.setSize(width, 800);
      await settle();
      result.textResizeOffsets.push(await run(readingOffset));
    }

    // ── A closed `/loop` fold must not read as a new prompt ───────────────
    await run('window.stageScrollFixture.foldSetup()');
    await settle();
    await run(`(() => { const node = document.querySelector('.desktop-stage'); node.scrollTop = 0; })()`);
    await settle();
    result.foldBefore = await run(metrics);
    result.foldControlBefore = await run('Boolean(document.querySelector(".desktop-stage-jump"))');
    // Hides the reader's own prompt, which is below the fold and untouched by
    // what they are reading.
    await run('window.stageScrollFixture.foldNewestPrompt()');
    await settle();
    result.foldAfter = await run(metrics);
    result.foldControlAfter = await run('Boolean(document.querySelector(".desktop-stage-jump"))');

    process.stdout.write(`${JSON.stringify(result)}\n`);
  } finally { window.destroy(); app.quit(); }
}
main().catch((error) => { process.stderr.write(`${error.stack ?? error}\n`); app.exit(1); });
