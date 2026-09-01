import { app, BrowserWindow } from 'electron';

// Electron may insert its own switches into argv before app arguments. Pick
// the fixture URL by shape so the driver remains stable across Electron CLI
// versions and launch modes.
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
    active: document.activeElement?.outerHTML?.slice(0, 240) ?? null,
    dialog: Boolean(document.querySelector('[role="dialog"]')),
    labels: [...document.querySelectorAll('button')].map((button) => button.getAttribute('aria-label')),
  })`);
  throw new Error(`timed out waiting for: ${expression}; state=${JSON.stringify(state)}`);
}

async function main() {
  await app.whenReady();
  const window = new BrowserWindow({
    show: false,
    width: 900,
    height: 700,
    webPreferences: {
      sandbox: true,
    },
  });

  try {
    await window.loadURL(url);
    const { webContents } = window;
    webContents.focus();
    await waitFor(webContents, `Boolean(document.querySelector('[role="dialog"]'))`);
    await waitFor(webContents, `document.activeElement?.textContent?.trim() === 'Allow once'`);

    const beforeTab = await webContents.executeJavaScript(`({
      promptOpen: Boolean(document.querySelector('[role="dialog"]')),
      activeLabel: document.activeElement?.textContent?.trim() ?? null,
      sidebarExists: Boolean(document.querySelector('button[aria-label="Add project"]')),
    })`);

    // This is a native Chromium key event, not a synthetic DOM KeyboardEvent.
    // The old document-level Tab trap prevented this default focus traversal.
    webContents.sendInputEvent({ type: 'keyDown', keyCode: 'TAB' });
    webContents.sendInputEvent({ type: 'keyUp', keyCode: 'TAB' });
    await waitFor(webContents, `document.activeElement?.classList.contains('sidebar-primary-action')`);

    const afterTab = await webContents.executeJavaScript(`({
      activeText: document.activeElement?.textContent?.trim() ?? null,
      activeIsSidebar: document.activeElement?.classList.contains('sidebar-primary-action') === true,
    })`);

    await webContents.executeJavaScript('window.__promptFocusTest.dismissPrompt()');
    await waitFor(webContents, `!document.querySelector('[role="dialog"]')`);
    const afterDismiss = await webContents.executeJavaScript(`({
      promptOpen: Boolean(document.querySelector('[role="dialog"]')),
      activeText: document.activeElement?.textContent?.trim() ?? null,
      activeIsSidebar: document.activeElement?.classList.contains('sidebar-primary-action') === true,
    })`);

    process.stdout.write(`${JSON.stringify({ beforeTab, afterTab, afterDismiss })}\n`);
  } finally {
    if (!window.isDestroyed()) window.destroy();
    if (app.isReady()) await app.quit();
  }
}

main().catch((error) => {
  process.stderr.write(`${error instanceof Error ? error.stack : String(error)}\n`);
  app.exit(1);
});
