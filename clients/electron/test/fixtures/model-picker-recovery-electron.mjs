/**
 * What the picker does when the model catalog has not arrived.
 *
 * `desktop.models` is filled by one `list_models` reply per connection. A reply
 * lost to a mid-connection engine restart left it empty, and the picker used to
 * read that as "nothing to pick" and disable itself — permanently, because
 * nothing else asks for the catalog. This drives the empty case in a real
 * window: the trigger must still open, opening must ask for the catalog again,
 * and the arriving catalog must land in the list.
 */
import { app, BrowserWindow } from 'electron';

const url = process.argv.find((argument) => argument.startsWith('http://') || argument.startsWith('https://'));
if (!url) throw new Error('fixture URL is required');

const testUserData = process.env.LINGXI_TEST_USER_DATA;
if (testUserData) app.setPath('userData', testUserData);

app.commandLine.appendSwitch('disable-gpu');
app.commandLine.appendSwitch('disable-software-rasterizer');

const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

/** Let React commit and the browser paint, so a read sees the settled DOM. */
const settle = (webContents) => webContents.executeJavaScript(
  `new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve(true))))`,
);

// Generous on purpose: this drives a whole Electron app alongside the rest of
// the suite, and every wait here is for a state change that has already been
// asked for — a slow machine is not a failure.
async function waitFor(webContents, expression, timeout = 20000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await webContents.executeJavaScript(expression)) return;
    await delay(25);
  }
  const state = await webContents.executeJavaScript(`({
    fixture: Boolean(window.__composerDraftTest),
    trigger: document.querySelector('button[aria-label^="Model: "]')?.disabled ?? null,
    menus: [...document.querySelectorAll('[role="menu"]')].map((menu) => menu.getAttribute('aria-label')),
  })`);
  throw new Error(`timed out waiting for: ${expression}; state=${JSON.stringify(state)}`);
}

async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 1400, height: 700, webPreferences: { sandbox: true } });

  try {
    await window.loadURL(url);
    const { webContents } = window;
    await waitFor(webContents, `Boolean(window.__composerDraftTest)`);

    // The catalog never arrived: model identity is known, the list is not.
    await webContents.executeJavaScript(`window.__composerDraftTest.setModels([])`);
    await waitFor(webContents, `window.__composerDraftTest.modelRefreshCount() === 0`);
    const empty = await webContents.executeJavaScript(`(() => {
      const trigger = document.querySelector('button[aria-label^="Model: "]');
      const rect = trigger.getBoundingClientRect();
      const hit = document.elementFromPoint(Math.round(rect.left + rect.width / 2), Math.round(rect.top + rect.height / 2));
      return {
        disabled: trigger.disabled,
        aria: trigger.getAttribute('aria-label'),
        reachable: Boolean(hit && trigger.contains(hit)),
        refreshes: window.__composerDraftTest.modelRefreshCount(),
      };
    })()`);

    // A disabled pill can never be opened, so stop here and let the test say
    // that in one line rather than time out on a menu that cannot appear.
    if (empty.disabled) {
      process.stdout.write(`${JSON.stringify({ empty, opened: null, listed: null })}\n`);
      return;
    }

    // Opening it is what asks for the catalog again.
    await webContents.executeJavaScript(`document.querySelector('button[aria-label^="Model: "]').click()`);
    await waitFor(webContents, `Boolean(document.querySelector('[aria-label="Model settings"]'))`);
    const opened = { refreshes: await webContents.executeJavaScript(`window.__composerDraftTest.modelRefreshCount()`) };

    // The reply the reopened picker asked for.
    await webContents.executeJavaScript(`window.__composerDraftTest.setModels('all')`);
    await webContents.executeJavaScript(`[...document.querySelectorAll('[aria-label="Model settings"] button')]
      .find((button) => button.textContent.trim().startsWith('Model')).click()`);
    await waitFor(webContents, `Boolean(document.querySelector('[aria-label="Available models"]'))`);
    const listed = await webContents.executeJavaScript(`(() => {
      const submenu = document.querySelector('[aria-label="Available models"]');
      return {
        rows: submenu.querySelectorAll('[role="menuitemradio"]').length,
        firstRowLabel: submenu.querySelector('[role="menuitemradio"]')?.textContent.trim() ?? null,
      };
    })()`);

    await webContents.executeJavaScript(`document.querySelector('button[aria-label^="Model: "]').click()`);
    await waitFor(webContents, `!document.querySelector('[aria-label="Model settings"]')`);

    // `ready` false is a composer that cannot SEND — no provider connected, no
    // live connection. The model control is how you fix that, so it stays live.
    await webContents.executeJavaScript(`window.__composerDraftTest.setReady(false)`);
    await settle(webContents);
    const notReady = await webContents.executeJavaScript(`(() => ({
      trigger: document.querySelector('button[aria-label^="Model: "]').disabled,
      attach: document.querySelector('button[aria-label="Attach files"]').disabled,
    }))()`);
    // Only reachable if it is live; otherwise report that and let the test say
    // so in one line rather than time out on a menu that cannot appear.
    let openedWhileNotReady = false;
    if (!notReady.trigger) {
      await webContents.executeJavaScript(`document.querySelector('button[aria-label^="Model: "]').click()`);
      await settle(webContents);
      openedWhileNotReady = await webContents.executeJavaScript(`Boolean(document.querySelector('[aria-label="Model settings"]'))`);
    }

    // No engine to take the change is the one refusal that is not a lie.
    await webContents.executeJavaScript(`window.__composerDraftTest.setConnected(false)`);
    await settle(webContents);
    const disconnected = await webContents.executeJavaScript(`(() => ({
      trigger: document.querySelector('button[aria-label^="Model: "]').disabled,
      menu: Boolean(document.querySelector('[aria-label="Model settings"]')),
    }))()`);

    // And it comes straight back, without the app being restarted.
    await webContents.executeJavaScript(`window.__composerDraftTest.setConnected(true)`);
    await settle(webContents);
    const reconnected = await webContents.executeJavaScript(
      `document.querySelector('button[aria-label^="Model: "]').disabled`,
    );

    process.stdout.write(`${JSON.stringify({ empty, opened, listed, notReady, openedWhileNotReady, disconnected, reconnected })}\n`);
  } finally {
    if (!window.isDestroyed()) window.destroy();
    if (app.isReady()) await app.quit();
  }
}

main().catch((error) => {
  process.stderr.write(`${error instanceof Error ? error.stack : String(error)}\n`);
  app.exit(1);
});
