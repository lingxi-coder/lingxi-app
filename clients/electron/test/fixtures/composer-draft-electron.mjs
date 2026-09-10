import { app, BrowserWindow } from 'electron';
import assert from 'node:assert/strict';
import { writeFile } from 'node:fs/promises';
import { join } from 'node:path';

const url = process.argv.find((argument) => argument.startsWith('http://') || argument.startsWith('https://'));
if (!url) throw new Error('fixture URL is required');

const testUserData = process.env.LINGXI_TEST_USER_DATA;
if (testUserData) app.setPath('userData', testUserData);

app.commandLine.appendSwitch('disable-gpu');
app.commandLine.appendSwitch('disable-software-rasterizer');

const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

async function waitFor(webContents, expression, timeout = 8000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await webContents.executeJavaScript(expression)) return;
    await delay(25);
  }
  const state = await webContents.executeJavaScript(`({
    prompt: document.querySelector('[aria-label="Prompt"]')?.textContent ?? null,
    fixture: Boolean(window.__composerDraftTest),
  })`);
  throw new Error(`timed out waiting for: ${expression}; state=${JSON.stringify(state)}`);
}

async function setPrompt(webContents, text) {
  await webContents.executeJavaScript(`(() => {
    const prompt = document.querySelector('[aria-label="Prompt"]');
    prompt.textContent = ${JSON.stringify(text)};
    prompt.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText', data: ${JSON.stringify(text)} }));
  })()`);
}

async function switchSession(webContents, sessionId) {
  await webContents.executeJavaScript(`window.__composerDraftTest.switchSession(${JSON.stringify(sessionId)})`);
}

async function main() {
  await app.whenReady();
  const window = new BrowserWindow({
    show: false,
    width: 900,
    height: 700,
    webPreferences: { sandbox: true, ...(process.env.LINGXI_TEST_PRELOAD ? { preload: process.env.LINGXI_TEST_PRELOAD } : {}) },
  });

  try {
    await window.loadURL(url);
    const { webContents } = window;
    await waitFor(webContents, `Boolean(window.__composerDraftTest && document.querySelector('[aria-label="Prompt"]'))`);

    const emptyState = await webContents.executeJavaScript(`(() => {
      const prompt = document.querySelector('[aria-label="Prompt"]');
      prompt.focus();
      const send = document.querySelector('[aria-label="Send prompt"]');
      return {
        placeholderPosition: getComputedStyle(prompt, '::before').position,
        sendVisibility: getComputedStyle(send.parentElement).visibility,
        disabled: send.disabled,
        tabIndex: send.tabIndex,
        offset: window.getSelection().anchorOffset,
      };
    })()`);
    assert.deepEqual(emptyState, { placeholderPosition: 'absolute', sendVisibility: 'hidden', disabled: true, tabIndex: -1, offset: 0 });
    await webContents.insertText('Hello');
    await waitFor(webContents, `getComputedStyle(document.querySelector('.composer-send-presence')).transform === 'matrix(1, 0, 0, 1, 0, 0)'`);
    const typedState = await webContents.executeJavaScript(`(() => {
      const prompt = document.querySelector('[aria-label="Prompt"]');
      const range = document.createRange();
      range.setStart(prompt.firstChild, 0);
      range.setEnd(prompt.firstChild, 1);
      return { firstCharacterX: range.getBoundingClientRect().x - prompt.getBoundingClientRect().x,
        enabled: !document.querySelector('[aria-label="Send prompt"]').disabled };
    })()`);
    assert.equal(typedState.firstCharacterX, 18);
    assert.equal(typedState.enabled, true);
    if (process.env.LINGXI_COMPOSER_SCREENSHOT) {
      await writeFile(process.env.LINGXI_COMPOSER_SCREENSHOT + '.filled.png', (await webContents.capturePage()).toPNG());
    }
    const sendBounds = await webContents.executeJavaScript(`document.querySelector('[aria-label="Send prompt"]').getBoundingClientRect().toJSON()`);
    await setPrompt(webContents, '');
    await waitFor(webContents, `getComputedStyle(document.querySelector('.composer-send-presence')).visibility === 'hidden'`);
    await webContents.executeJavaScript(`window.__composerDraftTest.setRunning(true)`);
    await waitFor(webContents, `getComputedStyle(document.querySelector('.composer-stop-presence')).transform === 'matrix(1, 0, 0, 1, 0, 0)'`);
    const stopBounds = await webContents.executeJavaScript(`document.querySelector('[aria-label="Stop current turn"]').getBoundingClientRect().toJSON()`);
    assert.equal(stopBounds.x, sendBounds.x);
    assert.equal(stopBounds.y, sendBounds.y);
    assert.equal(stopBounds.width, sendBounds.width);
    if (process.env.LINGXI_COMPOSER_SCREENSHOT) {
      await writeFile(process.env.LINGXI_COMPOSER_SCREENSHOT + '.stop.png', (await webContents.capturePage()).toPNG());
    }
    await setPrompt(webContents, 'pending');
    await waitFor(webContents, `getComputedStyle(document.querySelector('.composer-send-presence')).transform === 'matrix(1, 0, 0, 1, 0, 0)'`);
    const pendingBounds = await webContents.executeJavaScript(`document.querySelector('[aria-label="Send pending message"]').getBoundingClientRect().toJSON()`);
    assert.equal(pendingBounds.x, sendBounds.x);
    await waitFor(webContents, `document.querySelector('.composer-submit-actions').getBoundingClientRect().width === 84`);
    const pendingStopBounds = await webContents.executeJavaScript(`document.querySelector('[aria-label="Stop current turn"]').getBoundingClientRect().toJSON()`);
    assert.equal(pendingBounds.x - pendingStopBounds.x, 44);
    if (process.env.LINGXI_COMPOSER_SCREENSHOT) {
      await writeFile(process.env.LINGXI_COMPOSER_SCREENSHOT + '.pending.png', (await webContents.capturePage()).toPNG());
    }
    // Reverse an in-flight transition and verify it settles at the requested state.
    await setPrompt(webContents, '');
    await setPrompt(webContents, 'quick correction');
    await setPrompt(webContents, '');
    await waitFor(webContents, `document.querySelector('.composer-submit-actions').getBoundingClientRect().width === 40`);
    await waitFor(webContents, `getComputedStyle(document.querySelector('.composer-send-presence')).visibility === 'hidden'`);
    webContents.debugger.attach('1.3');
    await webContents.debugger.sendCommand('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-reduced-motion', value: 'reduce' }] });
    assert.equal(await webContents.executeJavaScript(`getComputedStyle(document.querySelector('.composer-submit-actions')).transitionDuration`), '0s');
    await webContents.debugger.sendCommand('Emulation.setEmulatedMedia', { features: [] });
    webContents.debugger.detach();
    await setPrompt(webContents, '');
    await webContents.executeJavaScript(`window.__composerDraftTest.setRunning(false)`);

    await setPrompt(webContents, '   ');
    assert.equal(await webContents.executeJavaScript(`document.querySelector('[aria-label="Send prompt"]').disabled`), true);
    await setPrompt(webContents, '');

    const composerScreenshotPath = process.env.LINGXI_COMPOSER_SCREENSHOT;
    if (composerScreenshotPath) {
      const bounds = await webContents.executeJavaScript(`(() => {
        const composer = document.querySelector('[aria-label="Prompt"]')?.closest('[data-composer-surface]')
          ?? document.querySelector('[aria-label="Prompt"]')?.parentElement?.parentElement;
        const rect = composer.getBoundingClientRect();
        return { x: Math.floor(rect.x), y: Math.floor(rect.y), width: Math.ceil(rect.width), height: Math.ceil(rect.height) };
      })()`);
      const image = await webContents.capturePage(bounds);
      await writeFile(composerScreenshotPath, image.toPNG());
    }

    if (process.env.LINGXI_TEST_PRELOAD && testUserData) {
      const filePath = join(testUserData, 'notes 中文.txt');
      await writeFile(filePath, 'File attachment regression test');
      webContents.debugger.attach('1.3');
      const { root } = await webContents.debugger.sendCommand('DOM.getDocument');
      const { nodeId } = await webContents.debugger.sendCommand('DOM.querySelector', { nodeId: root.nodeId, selector: 'input[type="file"]' });
      await webContents.executeJavaScript(`document.querySelector('input[type="file"]').addEventListener('change', (event) => { window.__nativeAttachment = event.target.files[0]; }, { capture: true })`);
      await webContents.debugger.sendCommand('DOM.setFileInputFiles', { nodeId, files: [filePath] });
      await waitFor(webContents, `Boolean(document.querySelector('[data-file-mention]'))`);
      assert.equal(await webContents.executeJavaScript(`document.querySelector('[data-file-mention]').dataset.fileMention`), filePath);
      await waitFor(webContents, `getComputedStyle(document.querySelector('.composer-send-presence')).transform === 'matrix(1, 0, 0, 1, 0, 0)'`);
      if (process.env.LINGXI_COMPOSER_SCREENSHOT) {
        await writeFile(process.env.LINGXI_COMPOSER_SCREENSHOT + '.file.png', (await webContents.capturePage()).toPNG());
      }
      // Duplicate attachments are deduplicated; session switching preserves the token.
      await webContents.debugger.sendCommand('DOM.setFileInputFiles', { nodeId, files: [filePath] });
      assert.equal(await webContents.executeJavaScript(`document.querySelectorAll('[data-file-mention]').length`), 1);
      for (const eventType of ['paste', 'drop']) {
        await setPrompt(webContents, '');
        await webContents.executeJavaScript(`(() => {
          const transfer = new DataTransfer();
          transfer.items.add(window.__nativeAttachment);
          const event = ${JSON.stringify(eventType)} === 'paste'
            ? new ClipboardEvent('paste', { clipboardData: transfer, bubbles: true, cancelable: true })
            : new DragEvent('drop', { dataTransfer: transfer, bubbles: true, cancelable: true });
          document.querySelector('[aria-label="Prompt"]').dispatchEvent(event);
        })()`);
        await waitFor(webContents, `Boolean(document.querySelector('[data-file-mention]'))`);
        assert.equal(await webContents.executeJavaScript(`document.querySelector('[data-file-mention]').dataset.fileMention`), filePath);
      }
      await switchSession(webContents, 'ffffffff-ffff-4fff-8fff-ffffffffffff');
      await waitFor(webContents, `!document.querySelector('[data-file-mention]')`);
      await switchSession(webContents, 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa');
      await waitFor(webContents, `Boolean(document.querySelector('[data-file-mention]'))`);
      await webContents.executeJavaScript(`document.querySelector('[aria-label="Send prompt"]').click()`);
      await waitFor(webContents, `window.__composerDraftTest.sendPending()`);
      assert.equal(await webContents.executeJavaScript(`window.__composerDraftTest.lastSentPrompt()`), '@"' + filePath + '"');
      await webContents.executeJavaScript(`window.__composerDraftTest.resolveSend()`);
      await waitFor(webContents, `!document.querySelector('[data-file-mention]')`);
      webContents.debugger.detach();
      process.stdout.write(JSON.stringify({ nativeFileAttachments: true }) + '\n');
      return;
    }

    async function exerciseComposerControls(running) {
      await setPrompt(webContents, '');
      await webContents.executeJavaScript(`window.__composerDraftTest.setRunning(${running})`);
      await webContents.executeJavaScript(`window.__composerDraftTest.clearAudioRequests()`);
      await webContents.executeJavaScript(`document.querySelector('[aria-label^="Permission mode:"]').click()`);
      await waitFor(webContents, `Boolean(document.querySelector('[aria-label="Permission modes"]'))`);
      await webContents.executeJavaScript(`document.querySelector('[aria-label="Permission modes"] [role="menuitemradio"][aria-checked="false"]').click()`);
      await waitFor(webContents, `!document.querySelector('[aria-label="Permission modes"]')`);
      await webContents.executeJavaScript(`document.querySelector('[aria-label="Start ordinary recording"]').click()`);
      await waitFor(webContents, `document.querySelector('[aria-label="Stop dictation"]')?.disabled === false`);
      if (running) {
        await waitFor(webContents, `document.querySelector('[aria-label="Stop current turn"]')?.disabled === false`);
        const stopState = await webContents.executeJavaScript(`(() => {
          const stop = document.querySelector('[aria-label="Stop current turn"]');
          const rect = stop.getBoundingClientRect();
          return { visible: getComputedStyle(stop.parentElement).visibility, width: rect.width, tabIndex: stop.tabIndex,
            hitTarget: stop.contains(document.elementFromPoint(rect.x + rect.width / 2, rect.y + rect.height / 2)) };
        })()`);
        assert.deepEqual(stopState, { visible: 'visible', width: 40, tabIndex: 0, hitTarget: true });
        const cancellations = await webContents.executeJavaScript(`window.__composerDraftTest.cancelCount()`);
        await webContents.executeJavaScript(`document.querySelector('[aria-label="Stop current turn"]').click()`);
        assert.equal(await webContents.executeJavaScript(`window.__composerDraftTest.cancelCount()`), cancellations + 1);
        assert.deepEqual(await webContents.executeJavaScript(`window.__composerDraftTest.audioRequestTypes()`), ['request_authorization', 'start_listening']);
        assert.equal(await webContents.executeJavaScript(`document.querySelector('[aria-label="Stop dictation"]')?.disabled`), false);
      }
      const dictationStartRequests = await webContents.executeJavaScript(`window.__composerDraftTest.audioRequestTypes()`);
      await webContents.executeJavaScript(`document.querySelector('[aria-label="Stop dictation"]').click()`);
      await waitFor(webContents, `document.querySelector('[aria-label="Prompt"]')?.textContent === 'dictated text'`);
      const dictatedText = await webContents.executeJavaScript(`document.querySelector('[aria-label="Prompt"]')?.textContent`);

      await webContents.executeJavaScript(`window.__composerDraftTest.clearAudioRequests()`);
      await webContents.executeJavaScript(`document.querySelector('[aria-label="开启心流模式"]').click()`);
      await waitFor(webContents, `Boolean(document.querySelector('[role="group"][aria-label="心流模式"]'))`);
      await waitFor(webContents, `window.__composerDraftTest.audioRequestTypes().includes('start_listening')`);
      const flowStartRequests = await webContents.executeJavaScript(`window.__composerDraftTest.audioRequestTypes()`);
      const audioInteraction = { dictationStartRequests, dictatedText, flowStartRequests };
      await webContents.executeJavaScript(`document.querySelector('[aria-label="关闭心流模式"]').click()`);
      await waitFor(webContents, `!document.querySelector('[role="group"][aria-label="心流模式"]')`);

      await webContents.executeJavaScript(`document.querySelector('button[aria-label^="Model:"]').click()`);
      await waitFor(webContents, `Boolean(document.querySelector('[aria-label="Model settings"]'))`);
      const modelSettings = await webContents.executeJavaScript(`({
        resetToDefaultVisible: document.querySelector('[aria-label="Model settings"]')?.textContent?.includes('Reset to default') ?? false,
      })`);
      await webContents.executeJavaScript(`document.querySelector('[aria-label="Model settings"] button').click()`);
      await waitFor(webContents, `document.querySelector('[aria-label="Search models"]') === document.activeElement`);
      const initialPicker = await webContents.executeJavaScript(`(() => ({
        sections: [...document.querySelectorAll('[aria-label="Available models"] div')]
          .map((element) => element.textContent?.trim())
          .filter((text) => text === 'Paid' || text === 'Free'),
        models: [...document.querySelectorAll('[aria-label="Available models"] [role="menuitemradio"]')]
          .map((element) => element.textContent?.trim()),
      }))()`);

      const screenshotPath = process.env.LINGXI_MODEL_PICKER_SCREENSHOT;
      if (screenshotPath) {
        const bounds = await webContents.executeJavaScript(`(() => {
          const rect = document.querySelector('[aria-label="Available models"]').getBoundingClientRect();
          return { x: Math.floor(rect.x), y: Math.floor(rect.y), width: Math.ceil(rect.width), height: Math.ceil(rect.height) };
        })()`);
        const image = await webContents.capturePage(bounds);
        await writeFile(screenshotPath, image.toPNG());
      }

      await webContents.executeJavaScript(`(() => {
        const input = document.querySelector('[aria-label="Search models"]');
        const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
        setter.call(input, 'flash fin');
        input.dispatchEvent(new Event('input', { bubbles: true }));
      })()`);
      await waitFor(webContents, `document.querySelectorAll('[aria-label="Available models"] [role="menuitemradio"]').length === 1`);
      const filteredPicker = await webContents.executeJavaScript(`({
        focused: document.activeElement?.getAttribute('aria-label'),
        models: [...document.querySelectorAll('[aria-label="Available models"] [role="menuitemradio"]')]
          .map((element) => element.textContent?.trim()),
        clearVisible: Boolean(document.querySelector('[aria-label="Clear model search"]')),
      })`);
      await webContents.executeJavaScript(`document.querySelector('[aria-label="Clear model search"]').click()`);
      await waitFor(webContents, `document.querySelectorAll('[aria-label="Available models"] [role="menuitemradio"]').length === 4`);
      await webContents.executeJavaScript(`document.querySelector('button[aria-label^="Model:"]').click()`);

      return { audioInteraction, modelSettings, initialPicker, filteredPicker };
    }
    const idleControls = await exerciseComposerControls(false);
    const runningControls = await exerciseComposerControls(true);
    assert.deepEqual(runningControls, idleControls, 'running retains all idle audio and model picker behavior');
    const { audioInteraction, modelSettings, initialPicker, filteredPicker } = idleControls;
    await webContents.executeJavaScript(`window.__composerDraftTest.setRunning(false)`);

    const sessionA = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';
    const sessionB = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb';
    await setPrompt(webContents, 'draft for A');
    await switchSession(webContents, sessionB);
    await waitFor(webContents, `document.querySelector('[aria-label="Prompt"]')?.textContent === ''`);
    await setPrompt(webContents, 'draft for B');
    await switchSession(webContents, sessionA);
    await waitFor(webContents, `document.querySelector('[aria-label="Prompt"]')?.textContent === 'draft for A'`);
    const restoredA = await webContents.executeJavaScript(`document.querySelector('[aria-label="Prompt"]')?.textContent`);
    await switchSession(webContents, sessionB);
    await waitFor(webContents, `document.querySelector('[aria-label="Prompt"]')?.textContent === 'draft for B'`);
    const restoredB = await webContents.executeJavaScript(`document.querySelector('[aria-label="Prompt"]')?.textContent`);

    await setPrompt(webContents, 'sending from B');
    await waitFor(webContents, `document.querySelector('[aria-label="Send prompt"]')?.disabled === false`);
    await webContents.executeJavaScript(`document.querySelector('[aria-label="Send prompt"]').click()`);
    await waitFor(webContents, `window.__composerDraftTest.sendPending()`);
    const sessionC = 'cccccccc-cccc-4ccc-8ccc-cccccccccccc';
    await switchSession(webContents, sessionC);
    await waitFor(webContents, `document.querySelector('[aria-label="Prompt"]')?.textContent === ''`);
    await setPrompt(webContents, 'draft that must survive');
    await webContents.executeJavaScript(`window.__composerDraftTest.resolveSend()`);
    await delay(50);
    const survivingDraft = await webContents.executeJavaScript(`document.querySelector('[aria-label="Prompt"]')?.textContent`);

    const sessionD = 'dddddddd-dddd-4ddd-8ddd-dddddddddddd';
    await webContents.executeJavaScript(`(() => {
      const prompt = document.querySelector('[aria-label="Prompt"]');
      prompt.innerHTML = '<span data-file-mention="src/app.ts" contenteditable="false">app.ts</span> inspect this';
      prompt.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText', data: ' inspect this' }));
      const input = document.querySelector('input[type="file"]');
      const bytes = new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10]);
      const file = new File([bytes], 'tiny.png', { type: 'image/png' });
      const transfer = new DataTransfer();
      transfer.items.add(file);
      input.files = transfer.files;
      input.dispatchEvent(new Event('change', { bubbles: true }));
    })()`);
    await waitFor(webContents, `Boolean(document.querySelector('img[alt="tiny.png"]'))`);
    await switchSession(webContents, sessionD);
    await waitFor(webContents, `document.querySelector('[aria-label="Prompt"]')?.textContent === ''`);
    await switchSession(webContents, sessionC);
    await waitFor(webContents, `Boolean(document.querySelector('[data-file-mention="src/app.ts"]') && document.querySelector('img[alt="tiny.png"]'))`);
    const richDraft = await webContents.executeJavaScript(`({
      text: document.querySelector('[aria-label="Prompt"]')?.textContent,
      mention: document.querySelector('[data-file-mention="src/app.ts"]')?.getAttribute('data-file-mention'),
      image: document.querySelector('img[alt="tiny.png"]')?.getAttribute('alt'),
    })`);

    const sessionE = 'eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee';
    await switchSession(webContents, sessionE);
    await waitFor(webContents, `document.querySelector('[aria-label="Prompt"]')?.textContent === ''`);
    await webContents.executeJavaScript(`window.__composerDraftTest.setRunning(true)`);
    await waitFor(webContents, `document.querySelector('[aria-label="Prompt"]')?.isContentEditable === true`);
    await setPrompt(webContents, 'pending follow-up');
    await waitFor(webContents, `document.querySelector('[aria-label="Send pending message"]')?.disabled === false`);
    const runningInteraction = await webContents.executeJavaScript(`({
      editable: document.querySelector('[aria-label="Prompt"]')?.isContentEditable,
      attachEnabled: document.querySelector('[aria-label="Attach files"]')?.disabled === false,
      goalEnabled: document.querySelector('[aria-label="Toggle goal mode"]')?.disabled === false,
      stopEnabled: document.querySelector('[aria-label="Stop current turn"]')?.disabled === false,
    })`);
    await webContents.executeJavaScript(`document.querySelector('[aria-label="Send pending message"]').click()`);
    await waitFor(webContents, `window.__composerDraftTest.sendPending()`);
    const sentPending = await webContents.executeJavaScript(`window.__composerDraftTest.lastSentPrompt()`);
    await webContents.executeJavaScript(`window.__composerDraftTest.resolveSend()`);
    await waitFor(webContents, `document.querySelector('[aria-label="Prompt"]')?.textContent === ''`);

    process.stdout.write(`${JSON.stringify({ audioInteraction, modelSettings, initialPicker, filteredPicker, restoredA, restoredB, survivingDraft, richDraft, runningInteraction, sentPending })}\n`);
  } finally {
    if (!window.isDestroyed()) window.destroy();
    if (app.isReady()) await app.quit();
  }
}

main().catch((error) => {
  process.stderr.write(`${error instanceof Error ? error.stack : String(error)}\n`);
  app.exit(1);
});
