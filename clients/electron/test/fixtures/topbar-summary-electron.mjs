import { app, BrowserWindow } from 'electron';
import { writeFile } from 'node:fs/promises';

const url = process.argv.find((argument) => argument.startsWith('http://') || argument.startsWith('https://'));
if (!url) throw new Error('fixture URL is required');

const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

async function waitFor(webContents, expression, timeout = 8_000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await webContents.executeJavaScript(expression)) return;
    await delay(25);
  }
  throw new Error(`timed out waiting for: ${expression}`);
}

async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 1120, height: 720, webPreferences: { sandbox: true } });
  try {
    await window.loadURL(url);
    const { webContents } = window;
    await waitFor(webContents, `Boolean(document.querySelector('[aria-label="Open context summaries"]'))`);

    const initial = await webContents.executeJavaScript(`(() => {
      const trigger = document.querySelector('[aria-label="Open context summaries"]');
      return {
        commandVisible: document.body.textContent.includes('Commands'),
        engineVisible: document.body.textContent.includes('Engine ready'),
        background: getComputedStyle(trigger).backgroundColor,
      };
    })()`);

    await webContents.executeJavaScript(`document.querySelector('[aria-label="Open context summaries"]').click()`);
    await waitFor(webContents, `Boolean(document.querySelector('#context-summary-panel'))`);
    await delay(180);
    const activeBackground = await webContents.executeJavaScript(`getComputedStyle(document.querySelector('[aria-label="Open context summaries"]')).backgroundColor`);
    const selectedText = await webContents.executeJavaScript(`(() => {
      const options = [...document.querySelectorAll('[role="option"]')];
      options[1].click();
      return options[1].textContent;
    })()`);
    await waitFor(webContents, `document.querySelector('.context-summary-markdown')?.textContent.includes('Provider routing')`);
    await delay(180);
    const firstItem = await webContents.executeJavaScript(`({
      optionCount: document.querySelectorAll('[role="option"]').length,
      selectedText: ${JSON.stringify(selectedText)},
      detail: document.querySelector('.context-summary-markdown')?.textContent,
    })`);

    const screenshotPath = process.env.LINGXI_TOPBAR_SCREENSHOT;
    if (screenshotPath) {
      const image = await webContents.capturePage();
      await writeFile(screenshotPath, image.toPNG());
    }

    await webContents.executeJavaScript(`document.querySelector('[aria-label="Close context summaries"]').click()`);
    await waitFor(webContents, `!document.querySelector('#context-summary-panel')`);

    process.stdout.write(`${JSON.stringify({ initial, activeBackground, firstItem })}\n`);
  } finally {
    if (!window.isDestroyed()) window.destroy();
    app.quit();
  }
}

main().catch((error) => {
  process.stderr.write(`${error instanceof Error ? error.stack : String(error)}\n`);
  app.exit(1);
});
