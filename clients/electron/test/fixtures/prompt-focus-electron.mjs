import { writeFile } from 'node:fs/promises';
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
    resizeHandle: document.querySelector('[aria-label="Resize sidebar"]')?.getBoundingClientRect().toJSON() ?? null,
    sidebar: document.querySelector('[aria-label="Resize sidebar"]')?.closest('aside')?.getBoundingClientRect().toJSON() ?? null,
    resizing: document.querySelector('[aria-label="Resize sidebar"]')?.closest('aside')?.getAttribute('data-resizing') ?? null,
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

    const coverage = await webContents.executeJavaScript(`(() => {
      const overlay = document.querySelector('.desktop-dialog-overlay');
      const rect = overlay.getBoundingClientRect();
      return {
        fillsWindow: rect.x === 0 && rect.y === 0 && rect.width === innerWidth && rect.height === innerHeight,
        coversEdges: [[1, 1], [innerWidth - 1, 1], [1, innerHeight - 1], [innerWidth - 1, innerHeight - 1]]
          .every(([x, y]) => overlay.contains(document.elementFromPoint(x, y))),
      };
    })()`);
    if (process.env.LINGXI_PERMISSION_SCREENSHOT) {
      window.showInactive();
      await delay(300);
      await webContents.executeJavaScript(`document.getAnimations().forEach((animation) => animation.finish())`);
      await writeFile(process.env.LINGXI_PERMISSION_SCREENSHOT, (await webContents.capturePage()).toPNG());
    }

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

    if (process.env.LINGXI_SIDEBAR_SCREENSHOT) {
      const bounds = await webContents.executeJavaScript(`(() => {
        const rect = document.querySelector('.desktop-sidebar-footer').getBoundingClientRect();
        return { x: Math.floor(rect.x), y: Math.floor(rect.y), width: Math.ceil(rect.width), height: Math.ceil(rect.height) };
      })()`);
      await writeFile(process.env.LINGXI_SIDEBAR_SCREENSHOT, (await webContents.capturePage(bounds)).toPNG());
    }

    const resizeGeometry = await webContents.executeJavaScript(`(() => {
      const handle = document.querySelector('[aria-label="Resize sidebar"]');
      const sidebar = handle?.closest('aside');
      const handleRect = handle?.getBoundingClientRect();
      return {
        before: sidebar?.getBoundingClientRect().width ?? null,
        handleX: handleRect ? handleRect.x + handleRect.width / 2 : null,
        handleY: handleRect ? handleRect.y + Math.min(180, handleRect.height / 2) : null,
      };
    })()`);
    if (resizeGeometry.handleX === null || resizeGeometry.handleY === null) {
      throw new Error(`sidebar resize handle has no geometry: ${JSON.stringify(resizeGeometry)}`);
    }
    webContents.sendInputEvent({ type: 'mouseMove', x: Math.round(resizeGeometry.handleX), y: Math.round(resizeGeometry.handleY) });
    await delay(50);
    webContents.sendInputEvent({ type: 'mouseDown', x: Math.round(resizeGeometry.handleX), y: Math.round(resizeGeometry.handleY), button: 'left', clickCount: 1 });
    await delay(50);
    // Electron 43 derives pointermove.buttons from modifiers, not button alone.
    webContents.sendInputEvent({ type: 'mouseMove', x: 360, y: Math.round(resizeGeometry.handleY), button: 'left', modifiers: ['leftButtonDown'] });
    await waitFor(webContents, `Math.round(document.querySelector('[aria-label="Resize sidebar"]')?.closest('aside')?.getBoundingClientRect().width ?? 0) === 360`);
    webContents.sendInputEvent({ type: 'mouseUp', x: 360, y: Math.round(resizeGeometry.handleY), button: 'left', clickCount: 1 });
    await delay(50);
    const afterResize = await webContents.executeJavaScript(`Math.round(document.querySelector('[aria-label="Resize sidebar"]').closest('aside').getBoundingClientRect().width)`);

    process.stdout.write(`${JSON.stringify({ coverage, beforeTab, afterTab, afterDismiss, resize: { before: Math.round(resizeGeometry.before), after: afterResize } })}\n`);
  } finally {
    if (!window.isDestroyed()) window.destroy();
    if (app.isReady()) await app.quit();
  }
}

main().catch((error) => {
  process.stderr.write(`${error instanceof Error ? error.stack : String(error)}\n`);
  app.exit(1);
});
