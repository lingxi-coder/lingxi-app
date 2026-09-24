import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { join, resolve } from 'node:path';
import { test } from 'node:test';

import react from '@vitejs/plugin-react';
import { createServer } from 'vite';

const electronRoot = resolve(fileURLToPath(new URL('..', import.meta.url)));
const fixtureRoot = join(electronRoot, 'test', 'fixtures');
const electronBinary = resolve(electronRoot, 'node_modules/electron/cli.js');
const electronDriver = join(fixtureRoot, 'stage-scroll-electron.mjs');

test('Stage keeps the actual bottom stable during streaming and respects manual scrolling', async () => {
  const viteCacheDir = mkdtempSync(join(tmpdir(), 'lingxi-stage-scroll-vite-'));
  const temporaryUserData = mkdtempSync(join(tmpdir(), 'lingxi-stage-scroll-electron-'));
  const vite = await createServer({
    root: fixtureRoot,
    cacheDir: viteCacheDir,
    configFile: false,
    logLevel: 'error',
    server: { host: '127.0.0.1', port: 0, strictPort: false, hmr: false },
    plugins: [react()],
    resolve: {
      alias: { '@renderer': resolve(electronRoot, 'src/renderer') },
      dedupe: ['react', 'react-dom'],
    },
  });
  let child;

  try {
    await vite.listen();
    const address = vite.httpServer?.address();
    assert.ok(address && typeof address === 'object' && address.port);
    const fixtureUrl = `http://127.0.0.1:${address.port}/stage-scroll-fixture.html`;
    const output = [];
    const errors = [];
    child = spawn(process.execPath, [electronBinary, electronDriver, fixtureUrl], {
      cwd: electronRoot,
      env: {
        ...process.env,
        ELECTRON_ENABLE_LOGGING: '0',
        ELECTRON_IS_DEV: '0',
        LINGXI_TEST_USER_DATA: temporaryUserData,
      },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    child.stdout.on('data', (chunk) => output.push(String(chunk)));
    child.stderr.on('data', (chunk) => errors.push(String(chunk)));

    const result = await new Promise((resolveResult, rejectResult) => {
      const timeout = setTimeout(() => {
        child.kill('SIGTERM');
        rejectResult(new Error(`Electron Stage scroll fixture timed out\n${errors.join('')}`));
      }, 20_000);
      child.once('error', (error) => { clearTimeout(timeout); rejectResult(error); });
      child.once('exit', (code, signal) => {
        clearTimeout(timeout);
        const line = output.join('').trim().split('\n').at(-1);
        if (code !== 0 || !line) {
          rejectResult(new Error(`Electron Stage scroll fixture exited ${code ?? signal}\n${errors.join('')}`));
          return;
        }
        try { resolveResult(JSON.parse(line)); }
        catch (error) { rejectResult(new Error(`Invalid Electron Stage scroll fixture output: ${line}`, { cause: error })); }
      });
    });

    assert.ok(result.initialGap <= 1, `initial bottom gap: ${result.initialGap}`);
    assert.equal(result.ancestorScroll, 0, 'following the tail must not scroll an ancestor');
    assert.ok(Math.abs(result.dragAfter - result.awayBefore) <= 1, 'updates during a pointer drag must not snap');
    assert.ok(Math.abs(result.awayAfter - result.awayBefore) <= 1, 'updates must preserve manual position 40px above the bottom');
    assert.ok(result.immediateStreamingGaps.every((gap) => gap <= 1), `pre-paint streaming bottom gaps: ${result.immediateStreamingGaps}`);
    assert.ok(result.streamingGaps.every((gap) => gap <= 1), `streaming bottom gaps: ${result.streamingGaps}`);
    assert.ok(result.resizeGap <= 1, `async child resize bottom gap: ${result.resizeGap}`);
    assert.ok(result.shrinkGap <= 1, `async child shrink bottom gap: ${result.shrinkGap}`);
    assert.equal(result.finalAncestorScroll, 0);
    for (const layout of result.thinkingLayouts) {
      assert.deepEqual(layout, result.thinkingLayouts[0], 'thinking visibility must preserve tail height and scroll position');
    }

    for (const offset of [0, 4]) {
      const baseline = result.deliveryLayouts[offset];
      for (const layout of result.deliveryLayouts.slice(offset, offset + 4)) assert.deepEqual(layout, baseline, 'delivery changes must not resize or move transcript messages');
    }

    // Hover-revealed clock + copy button under a user message: the row reserves
    // its box, so revealing it may not resize the message.
    assert.equal(result.messageAway.opacity, '0', 'the affordance starts hidden');
    assert.equal(result.messageHover.opacity, '1', 'hovering the message reveals it');
    assert.notEqual(result.messageAway.clock, '', 'the clock renders when the prompt has a send time');
    assert.ok(result.messageAway.actions > 0, 'the hidden affordance still occupies its space');
    assert.equal(result.messageHover.row, result.messageAway.row, 'revealing must not resize the message row');
    assert.equal(result.messageHover.bubble, result.messageAway.bubble, 'revealing must not resize the bubble');
    assert.equal(result.messageHover.actions, result.messageAway.actions, 'the reserved box height is stable');

    // The revealed button, clicked for real. Two prompts are on screen and the
    // click lands on the second, so this pins ROW SELECTION — not merely that
    // some button copied some text.
    assert.equal(result.copyPoints.length, 2, 'both prompts expose their own control');
    assert.equal(result.copyBeforeClick.state, 'idle', 'the control starts idle');
    assert.equal(result.copyBeforeClick.copied, null, 'nothing is copied before the click');
    assert.equal(result.copyBeforeClick.status, '', 'and it says nothing while idle');
    assert.equal(result.copyBeforeClick.name, 'Copy message', 'the control carries one name');
    assert.equal(result.copyClick.copied, 'ship the release notes', 'the click copies the row it was on');
    assert.equal(result.copyClick.state, 'copied', 'the control reports the copy');
    assert.equal(result.copyClick.status, 'Copied', 'the outcome is announced');
    // A name that changed to "Copied" was announced twice over the live region
    // and re-announced when it reverted; the outcome lives in the region alone.
    assert.equal(result.copyClick.name, 'Copy message', 'the name does not change with the outcome');
    assert.equal(result.copyClick.neighbourState, 'idle', 'the other message is left alone');
    assert.equal(result.copyClick.neighbourStatus, '', 'and stays silent');
    assert.equal(result.copyClickReset.state, 'idle', 'the copied state is transient');
    assert.equal(result.copyClickReset.status, '', 'and the announcement is retracted');

    // A failing clipboard: the control must report the failure and must not
    // claim a copy happened.
    assert.equal(result.copyFailure.copied, null, 'a failed copy records nothing');
    assert.equal(result.copyFailure.state, 'error', 'the control reports the failure');
    assert.equal(result.copyFailure.status, 'Copy failed', 'and announces it');
    assert.equal(result.copyFailure.name, 'Copy message', 'the name still does not change');
    assert.equal(result.copyFailure.neighbourState, 'idle', 'the other message is still left alone');
    assert.equal(result.copyFailureReset.state, 'idle', 'the failed state is transient too');
    assert.equal(result.copyFailureReset.status, '', 'and its announcement is retracted');

    // A control that appears only once the reader leaves the tail, tracks the
    // viewport, and re-engages the follow when pressed.
    assert.equal(result.controlParked, false, 'the control is hidden while parked at the tail');
    assert.equal(result.controlAway.present, true, 'the control appears once the reader leaves the tail');
    assert.equal(result.controlAway.onScreen, true, 'the control is on screen, not merely inside the scrollport box');
    assert.ok(
      result.controlAway.pinnedToBottom >= 0 && result.controlAway.pinnedToBottom <= 16,
      `the control is pinned to the scrollport's bottom edge, not the content end: ${result.controlAway.pinnedToBottom}`,
    );
    assert.ok(result.jumpAfterClick <= 1, `the control returns to the true bottom: ${result.jumpAfterClick}`);
    assert.equal(result.controlAfterJump, false, 'the control retires once the reader is back at the tail');
    assert.ok(result.jumpFollowGap <= 1, `pressing it re-engages the follow: ${result.jumpFollowGap}`);

    // Parking is a band, not an exact fit. A streaming turn moves the bottom
    // every frame, so an exact test could never be satisfied by hand again.
    assert.ok(result.bandFollowGap <= 1, `inside the band the tail is still followed: ${result.bandFollowGap}`);

    // The reader's own prompt is one of the explicit re-engagements; nothing
    // idle-driven may do it, because an idle reader is reading.
    assert.ok(result.sendBeforeGap > 24, `the reader starts detached: ${result.sendBeforeGap}`);
    assert.ok(result.sendAfterGap <= 1, `sending re-engages the tail: ${result.sendAfterGap}`);

    // A resize must move the scroll offset, not the content under the reader.
    assert.ok(result.anchorBeforeResize && result.anchorAfterResize, 'the resize probe found a visible row');
    assert.equal(result.anchorAfterResize.text, result.anchorBeforeResize.text, 'the same row stays under the reader across a resize');
    assert.ok(
      Math.abs(result.anchorAfterResize.offset - result.anchorBeforeResize.offset) <= 1,
      `the reading offset survives a resize: ${result.anchorBeforeResize.offset} -> ${result.anchorAfterResize.offset}`,
    );

    for (const offset of result.textResizeOffsets) {
      assert.ok(Math.abs(offset - result.textResizeOffsets[0]) <= 1, `reading text moved during inspector resize: ${result.textResizeOffsets}`);
    }

    // A closed `/loop` fold hides a run of rows that can include the reader's own
    // prompt. Reading "the newest visible prompt" would then walk back to an
    // OLDER prompt id, look like a prompt the reader just sent, and drag them to
    // the bottom with no intent behind it.
    assert.equal(result.foldControlBefore, true, 'the reader is detached before the fold');
    assert.ok(
      Math.abs(result.foldAfter.top - result.foldBefore.top) <= 1,
      `hiding a folded run must not move a detached reader: ${result.foldBefore.top} -> ${result.foldAfter.top}`,
    );
    assert.equal(result.foldControlAfter, true, 'the control is still offered after the fold');
  } finally {
    if (child && child.exitCode === null) child.kill('SIGTERM');
    await vite.close();
    rmSync(viteCacheDir, { recursive: true, force: true });
    rmSync(temporaryUserData, { recursive: true, force: true });
  }
});
