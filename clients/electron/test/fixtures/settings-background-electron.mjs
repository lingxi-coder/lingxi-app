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
  throw new Error(`timed out waiting for: ${expression}`);
}

async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 900, height: 700, webPreferences: { sandbox: true } });
  try {
    await window.loadURL(url);
    const { webContents } = window;
    webContents.focus();
    await waitFor(webContents, 'Boolean(window.__settingsBackgroundTest)');
    await webContents.executeJavaScript('window.__settingsBackgroundTest.setOpen(true)');
    await waitFor(webContents, 'document.querySelector(\'.ask-user-question-inline\') && document.querySelector(\'[aria-hidden="true"]\')');
    const open = await webContents.executeJavaScript(`(() => {
      const button = document.querySelector('#background-button');
      const background = button.parentElement;
      const input = document.querySelector('.ask-user-question-inline input[type="radio"]');
      button.focus();
      const backgroundFocusable = document.activeElement === button;
      input.focus();
      return {
        inert: background?.hasAttribute('inert') === true,
        ariaHidden: background?.getAttribute('aria-hidden') ?? null,
        backgroundFocusable,
        promptInsideInert: Boolean(input.closest('[inert]')),
        promptFocusable: document.activeElement === input,
      };
    })()`);
    await webContents.executeJavaScript('window.__settingsBackgroundTest.setOpen(false)');
    await waitFor(webContents, '!document.querySelector(\'[inert]\') && !document.querySelector(\'#settings-view\')');
    const closed = await webContents.executeJavaScript(`(() => {
      const button = document.querySelector('#background-button');
      const input = document.querySelector('.ask-user-question-inline input[type="radio"]');
      button.focus();
      const backgroundFocusable = document.activeElement === button;
      input.focus();
      return {
        inert: Boolean(document.querySelector('[inert]')),
        ariaHidden: button.parentElement.getAttribute('aria-hidden'),
        backgroundFocusable,
        promptFocusable: document.activeElement === input,
      };
    })()`);
    process.stdout.write(`${JSON.stringify({ open, closed })}\n`);
  } finally {
    if (!window.isDestroyed()) window.destroy();
    if (app.isReady()) await app.quit();
  }
}

main().catch((error) => { process.stderr.write(`${error instanceof Error ? error.stack : String(error)}\n`); app.exit(1); });
