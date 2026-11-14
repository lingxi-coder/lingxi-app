import { configureFixtureWindow, waitForFixture } from './electron-test-environment.mjs';
import { app, BrowserWindow } from 'electron';
import assert from 'node:assert/strict';

const url = process.argv.find(argument => argument.startsWith('http://'));
if (!url || !process.env.LINGXI_TEST_USER_DATA) throw new Error('isolated fixture URL and profile required');
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));

async function main() {
  await app.whenReady();
  const win = new BrowserWindow({ show: false, width: 1000, height: 800, webPreferences: { sandbox: true, backgroundThrottling: false } });
  const run = source => win.webContents.executeJavaScript(source);
  try {
    await win.loadURL(url);
    await configureFixtureWindow(win);
    await waitForFixture(win.webContents, 'Boolean(window.stageScrollFixture && document.querySelector(".desktop-stage"))');
    await run('window.stageScrollFixture.setAncestorHeight(700)');
    await run(`document.querySelector('.desktop-stage').addEventListener('wheel', event => {
      if (event.isTrusted) window.historyWheelEvents++;
    }, { passive: true })`);
    const wheel = async (deltaY = -6) => {
      const point = await run(`(() => { const r = document.querySelector('.desktop-stage').getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + 200 }; })()`);
      await win.webContents.debugger.sendCommand('Input.dispatchMouseEvent', { type: 'mouseWheel', ...point, deltaX: 0, deltaY });
      await run('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
    };
    const results = [];
    for (const count of [1, 2, 40, 140, 180]) {
      await run(`window.stageScrollFixture.historySetup(${count})`);
      await delay(120);
      await run(`(() => {
        const stage = document.querySelector('.desktop-stage');
        stage.scrollTop = 140;
        stage.dispatchEvent(new Event('scroll'));
        window.historyWheelEvents = 0;
      })()`);
      await wheel();
      await waitForFixture(win.webContents, 'window.stageScrollFixture.historyRequests() === 1');
      await delay(100);
      if (count > 2) {
        await run(`(() => { const stage = document.querySelector('.desktop-stage'); stage.scrollTop = 500; stage.dispatchEvent(new Event('scroll')); })()`);
        await run('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
        assert.ok(await run(`document.querySelector('[data-message-uuid="history-current-0"]').getBoundingClientRect().bottom <= document.querySelector('.desktop-stage').getBoundingClientRect().top`), 'the growing message is above the reading position');
      }
      const held = await run(`(() => {
        const stage = document.querySelector('.desktop-stage').getBoundingClientRect();
        const row = [...document.querySelectorAll('[data-message-uuid]')].find(row => {
          const rect = row.getBoundingClientRect();
          return rect.top > stage.top + 40 && rect.bottom < stage.bottom - 30;
        });
        return row?.dataset.messageUuid;
      })()`);
      assert.ok(held, 'a visible saved message is available as a reading anchor');
      const read = `(() => {
        const stage = document.querySelector('.desktop-stage');
        const row = [...stage.querySelectorAll('[data-message-uuid]')].find(row => row.dataset.messageUuid === ${JSON.stringify(held)});
        return { at: performance.now(), offset: row ? row.getBoundingClientRect().top - stage.getBoundingClientRect().top : null, scrollTop: stage.scrollTop };
      })()`;
      const collect = () => run(`new Promise(resolve => {
        const frames = [];
        const sample = () => requestAnimationFrame(() => setTimeout(() => {
          frames.push(${read});
          if (frames.length < 4) sample(); else resolve(frames);
        }, 0));
        sample();
      })`);
      const drift = (before, frames) => Math.max(...frames.map(frame => frame.offset === null || before.offset === null ? Number.MAX_SAFE_INTEGER : Math.abs(frame.offset - before.offset)));
      const phase = async (name, mutation) => {
        await wheel();
        const before = await run(read);
        const mutationResult = await run(mutation);
        const enteringOffset = name === 'prepend' && count === 1 ? await run(`(() => {
          const row = document.querySelector('[data-message-uuid="older-59"]');
          const body = row?.querySelector('.transcript-run-item');
          return body && row ? body.getBoundingClientRect().top - row.getBoundingClientRect().top : null;
        })()`) : undefined;
        const frames = await collect();
        return { phase: name, before, mutationResult, enteringOffset, frames, maxDrift: drift(before, frames) };
      };
      const phases = count > 2 ? [await phase('markdown-growth', `window.stageScrollFixture.growHistory('history-current-0')`)] : [];
      const prepend = await phase('prepend', 'window.stageScrollFixture.completeHistory()');
      phases.push(prepend);
      if (count === 1) {
        assert.ok(Number.isFinite(prepend.enteringOffset), 'the newly mounted historical message is measured');
        await delay(350);
        await run('window.stageScrollFixture.repaint()');
        const frames = await run(`new Promise(resolve => {
          const frames = [];
          const sample = () => requestAnimationFrame(() => {
            const row = document.querySelector('[data-message-uuid="older-59"]');
            const body = row?.querySelector('.transcript-run-item');
            frames.push({ at: performance.now(), offset: body && row ? body.getBoundingClientRect().top - row.getBoundingClientRect().top : null });
            if (frames.length < 4) sample(); else resolve(frames);
          });
          sample();
        })`);
        const before = { offset: prepend.enteringOffset };
        phases.push({ phase: 'historical-text-after-entry-animation', before, frames, maxDrift: drift(before, frames) });
      }
      phases.push(await phase('late-child-resize', `(() => {
        const stage = document.querySelector('.desktop-stage');
        const top = stage.getBoundingClientRect().top;
        const reading = [...stage.querySelectorAll('.desktop-stage-feed > .transcript-row')].find(row => row.getBoundingClientRect().bottom > top);
        const row = reading?.previousElementSibling;
        if (!row?.classList.contains('transcript-row')) return false;
        const child = document.createElement('div');
        child.style.height = '90px';
        row.prepend(child);
        return true;
      })()`));
      const settledBefore = await run(read);
      await delay(220);
      const settledFrames = await collect();
      phases.push({ phase: 'gesture-settled', before: settledBefore, frames: settledFrames, maxDrift: drift(settledBefore, settledFrames) });
      const state = await run(`(() => {
        const ids = [...document.querySelectorAll('[data-message-uuids]')].flatMap(row => row.dataset.messageUuids.split(' '));
        return { wheelEvents: window.historyWheelEvents, historyRequests: window.stageScrollFixture.historyRequests(),
          messageCount: ids.length, uniqueMessages: new Set(ids).size };
      })()`);
      if (count === 1) {
        await wheel(6);
        await delay(220);
      } else if (count === 2) {
        await run(`document.querySelector('.desktop-stage').focus({ preventScroll: true })`);
        win.webContents.sendInputEvent({ type: 'keyDown', keyCode: 'End' });
        win.webContents.sendInputEvent({ type: 'keyUp', keyCode: 'End' });
      } else await run(`document.querySelector('.desktop-stage-jump')?.click()`);
      await run('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
      const tail = await run(`(() => { const stage = document.querySelector('.desktop-stage'); return {
        reserved: document.querySelector('.transcript-history-tail-space').offsetHeight,
        gap: stage.scrollHeight - stage.scrollTop - stage.clientHeight,
      }; })()`);
      results.push({ count, ...state, phases, tail });
    }
    process.stdout.write(JSON.stringify(results) + '\n');
  } finally {
    win.destroy();
    app.quit();
  }
}
main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); app.exit(1); });
