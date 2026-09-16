import { app, BrowserWindow } from 'electron';
import { writeFile } from 'node:fs/promises';

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
    fixture: Boolean(window.__composerDraftTest),
    innerWidth,
    trigger: document.querySelector('button[aria-label^="Model: "]')?.getAttribute('aria-expanded') ?? null,
    menus: [...document.querySelectorAll('[role="menu"]')].map((menu) => menu.getAttribute('aria-label')),
  })`);
  throw new Error(`timed out waiting for: ${expression}; state=${JSON.stringify(state)}`);
}

/** Open the picker and drill into its model list. */
async function openModelList(webContents) {
  await webContents.executeJavaScript(`document.querySelector('button[aria-label^="Model: "]').click()`);
  await waitFor(webContents, `Boolean(document.querySelector('[aria-label="Model settings"]'))`);
  await webContents.executeJavaScript(`[...document.querySelectorAll('[aria-label="Model settings"] button')]
    .find((button) => button.textContent.trim().startsWith('Model')).click()`);
  await waitFor(webContents, `Boolean(document.querySelector('[aria-label="Available models"]'))`);
}

async function closePicker(webContents) {
  await webContents.executeJavaScript(`document.querySelector('button[aria-label^="Model: "]').click()`);
  await waitFor(webContents, `!document.querySelector('[aria-label="Model settings"]')`);
}

/**
 * The rectangles that decide whether the list is readable, plus a hit test on
 * the first model row. `getBoundingClientRect` reports the LAID OUT box even
 * where an ancestor's `overflow: hidden` has cut it away, so the rectangles
 * alone cannot tell a placed panel from a clipped one — `elementFromPoint`
 * over the row's label can.
 */
async function measure(webContents) {
  return webContents.executeJavaScript(`(() => {
    const box = (element) => {
      const rect = element.getBoundingClientRect();
      return { left: rect.left, right: rect.right, width: rect.width, top: rect.top, bottom: rect.bottom };
    };
    const submenu = document.querySelector('[aria-label="Available models"]');
    const menu = document.querySelector('[aria-label="Model settings"]');
    const dock = document.querySelector('.desktop-composer-dock');
    const row = submenu.querySelector('[role="menuitemradio"]');
    const rowRect = row.getBoundingClientRect();
    const probeX = Math.round(rowRect.left + 12);
    const probeY = Math.round(rowRect.top + rowRect.height / 2);
    const hit = document.elementFromPoint(probeX, probeY);
    return {
      submenu: box(submenu),
      menu: box(menu),
      dock: box(dock),
      innerWidth,
      firstRowLabel: row.textContent.trim(),
      firstRowReachable: Boolean(hit && submenu.contains(hit)),
    };
  })()`);
}

/** Paint the window and save what the picker actually looks like. */
async function screenshot(window, suffix) {
  const target = process.env.LINGXI_MODEL_PICKER_SCREENSHOT;
  if (!target) return;
  window.showInactive();
  await delay(300);
  await window.webContents.executeJavaScript(`document.getAnimations().forEach((animation) => animation.finish())`);
  await writeFile(`${target}.${suffix}.png`, (await window.webContents.capturePage()).toPNG());
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
    await waitFor(webContents, `Boolean(window.__composerDraftTest)`);

    // A sidebar the width of the app's default one, so the composer sits in the
    // same narrow workspace the picker has to fit inside.
    await webContents.executeJavaScript(`window.__composerDraftTest.setSidebarWidth(260)`);
    await waitFor(webContents, `document.querySelector('.desktop-composer-dock')?.getBoundingClientRect().left === 260`);
    await openModelList(webContents);
    const narrow = await measure(webContents);
    await screenshot(window, 'narrow');

    // Drilled down over the menu, the heading is the way back to it.
    await webContents.executeJavaScript(`document.querySelector('[aria-label="Back to model settings from Model"]').click()`);
    await waitFor(webContents, `!document.querySelector('[aria-label="Available models"]')`);
    const afterBack = await webContents.executeJavaScript(`(() => {
      const menu = document.querySelector('[aria-label="Model settings"]');
      const row = menu?.querySelector('button');
      const rect = row?.getBoundingClientRect();
      const hit = rect && document.elementFromPoint(Math.round(rect.left + 12), Math.round(rect.top + rect.height / 2));
      return { rowLabel: row?.textContent.trim() ?? null, rowReachable: Boolean(hit && menu.contains(hit)) };
    })()`);
    await closePicker(webContents);

    // Wide enough for the flyout: the list should go back beside the menu
    // rather than stay parked on top of it.
    await webContents.executeJavaScript(`window.__composerDraftTest.setSidebarWidth(0)`);
    window.setContentSize(1400, 700);
    await waitFor(webContents, `innerWidth === 1400 && document.querySelector('.desktop-composer-dock')?.getBoundingClientRect().left === 0`);
    await openModelList(webContents);
    const wide = await measure(webContents);
    await screenshot(window, 'wide');

    process.stdout.write(`${JSON.stringify({ narrow, afterBack, wide })}\n`);
  } finally {
    if (!window.isDestroyed()) window.destroy();
    if (app.isReady()) await app.quit();
  }
}

main().catch((error) => {
  process.stderr.write(`${error instanceof Error ? error.stack : String(error)}\n`);
  app.exit(1);
});
