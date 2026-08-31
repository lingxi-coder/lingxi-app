import { app, BrowserWindow } from 'electron';

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
    webPreferences: { sandbox: true },
  });

  try {
    await window.loadURL(url);
    const { webContents } = window;
    await waitFor(webContents, `Boolean(window.__composerDraftTest && document.querySelector('[aria-label="Prompt"]'))`);

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
      attachEnabled: document.querySelector('[aria-label="Attach image"]')?.disabled === false,
      goalEnabled: document.querySelector('[aria-label="Toggle goal mode"]')?.disabled === false,
      stopEnabled: document.querySelector('[aria-label="Stop current turn"]')?.disabled === false,
    })`);
    await webContents.executeJavaScript(`document.querySelector('[aria-label="Send pending message"]').click()`);
    await waitFor(webContents, `window.__composerDraftTest.sendPending()`);
    const sentPending = await webContents.executeJavaScript(`window.__composerDraftTest.lastSentPrompt()`);
    await webContents.executeJavaScript(`window.__composerDraftTest.resolveSend()`);
    await waitFor(webContents, `document.querySelector('[aria-label="Prompt"]')?.textContent === ''`);

    process.stdout.write(`${JSON.stringify({ restoredA, restoredB, survivingDraft, richDraft, runningInteraction, sentPending })}\n`);
  } finally {
    if (!window.isDestroyed()) window.destroy();
    if (app.isReady()) await app.quit();
  }
}

main().catch((error) => {
  process.stderr.write(`${error instanceof Error ? error.stack : String(error)}\n`);
  app.exit(1);
});
