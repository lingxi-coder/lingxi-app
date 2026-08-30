/**
 * Electron driver for the microphone-permission round trip.
 *
 * Runs the PRODUCTION main-process reader (`src/main/microphoneAccess.ts`,
 * transpiled by the test into a temp directory and passed as argv) inside a
 * real Electron main process, next to a direct read of
 * `systemPreferences.getMediaAccessStatus('microphone')` — so the test can
 * assert the production path agrees with the OS rather than assuming it.
 *
 * The window carries this app's own session handlers, copied verbatim from
 * `src/main/index.ts`'s `secureSession`, because they are what makes
 * `navigator.permissions.query({name:'microphone'})` answer `granted` for
 * the app's own renderer no matter what macOS thinks — the defect under test.
 */
import { app, BrowserWindow, ipcMain, session, systemPreferences } from 'electron';
import { pathToFileURL } from 'node:url';
import { join } from 'node:path';

const url = process.argv.find((argument) => argument.startsWith('http://'));
const modulePath = process.argv.find((argument) => argument.endsWith('main/microphoneAccess.js'));
if (!url) throw new Error('fixture URL is required');
if (!modulePath) throw new Error('the transpiled production microphoneAccess module path is required');

const { readMicrophoneAccess } = await import(pathToFileURL(modulePath).toString());

const testUserData = process.env.LINGXI_TEST_USER_DATA;
if (testUserData) app.setPath('userData', testUserData);
app.commandLine.appendSwitch('disable-gpu');
app.commandLine.appendSwitch('disable-software-rasterizer');

const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

async function waitFor(webContents, expression, timeout = 6000) {
  const deadline = Date.now() + timeout;
  let last;
  while (Date.now() < deadline) {
    last = await webContents.executeJavaScript(expression);
    if (last) return last;
    await delay(25);
  }
  throw new Error(`timed out waiting for: ${expression} (last value ${JSON.stringify(last)})`);
}

// Verbatim from src/main/index.ts's isLocalRendererUrl/secureSession.
function isLocalRendererUrl(raw) {
  try {
    const parsed = new URL(raw);
    return parsed.protocol === 'file:'
      || ((parsed.protocol === 'http:' || parsed.protocol === 'https:')
        && (parsed.hostname === 'localhost' || parsed.hostname === '127.0.0.1' || parsed.hostname === '::1'));
  } catch {
    return false;
  }
}

async function main() {
  await app.whenReady();
  session.defaultSession.setPermissionCheckHandler((_webContents, permission, requestingOrigin) => (
    permission === 'media' && isLocalRendererUrl(requestingOrigin)
  ));
  session.defaultSession.setPermissionRequestHandler((webContents, permission, callback) => callback(
    permission === 'media' && isLocalRendererUrl(webContents.getURL()),
  ));

  // The production channel and the production reader — the same pair
  // `HostController.registerIpc` installs.
  ipcMain.handle('lingxi:microphone-access:get', () => readMicrophoneAccess());

  const window = new BrowserWindow({
    show: false,
    width: 900,
    height: 700,
    webPreferences: {
      preload: join(import.meta.dirname, 'microphone-permission-preload.cjs'),
      sandbox: true,
      contextIsolation: true,
      nodeIntegration: false,
    },
  });
  try {
    await window.loadURL(url);
    const { webContents } = window;
    await waitFor(webContents, 'Boolean(window.__microphonePermissionTest)');

    const mediaAccessStatus = systemPreferences.getMediaAccessStatus('microphone');
    const mainProcessRead = readMicrophoneAccess();
    const rendererProbe = await webContents.executeJavaScript('window.__microphonePermissionTest.probe()');
    const permissionsApi = await webContents.executeJavaScript('window.__microphonePermissionTest.permissionsApi()');

    // The mounted page, once its probe has resolved.
    await waitFor(webContents, '!window.__microphonePermissionTest.rowText().includes("正在检测设备的语音能力")');
    const rowTextFromOs = await webContents.executeJavaScript('window.__microphonePermissionTest.rowText()');

    // Second pass: the OS answer is swapped for a denial while the renderer's
    // OWN page permission stays granted. A probe that still reports granted is
    // reading the page permission, on any machine, whatever its real TCC state.
    ipcMain.removeHandler('lingxi:microphone-access:get');
    ipcMain.handle('lingxi:microphone-access:get', () => 'denied');
    const rendererProbeWhenOsDenies = await webContents.executeJavaScript('window.__microphonePermissionTest.probe()');

    // The page was mounted BEFORE the grant changed and is not told about it —
    // exactly the situation of a user flipping the switch in System Settings
    // while the window sits open. Coming back to the window is the only signal.
    const rowTextBeforeRefocus = await webContents.executeJavaScript('window.__microphonePermissionTest.rowText()');
    await webContents.executeJavaScript('window.__microphonePermissionTest.refocus()');
    await waitFor(webContents, 'window.__microphonePermissionTest.rowText().includes("未授权") || null', 4000)
      .catch(() => undefined);
    const rowTextAfterRefocus = await webContents.executeJavaScript('window.__microphonePermissionTest.rowText()');

    // The remedy the denied state exists to offer.
    const clickedOpenSystemSettings = await webContents.executeJavaScript('window.__microphonePermissionTest.clickOpenSystemSettings()');
    await delay(50);
    const openedSystemSettingsPanes = await webContents.executeJavaScript('window.__microphonePermissionTest.openedSystemSettingsPanes()');

    // Third pass: the user grants the permission and comes back. The row has
    // to follow the OS in BOTH directions — which also makes the refresh proof
    // independent of whatever this machine's real grant happens to be.
    ipcMain.removeHandler('lingxi:microphone-access:get');
    ipcMain.handle('lingxi:microphone-access:get', () => 'granted');
    await webContents.executeJavaScript('window.__microphonePermissionTest.refocus()');
    await waitFor(webContents, 'window.__microphonePermissionTest.rowText().includes("已授权") || null', 4000)
      .catch(() => undefined);
    const rowTextAfterGrant = await webContents.executeJavaScript('window.__microphonePermissionTest.rowText()');
    const openSystemSettingsAfterGrant = await webContents.executeJavaScript('window.__microphonePermissionTest.clickOpenSystemSettings()');

    process.stdout.write(`${JSON.stringify({
      mediaAccessStatus,
      mainProcessRead,
      rendererProbe,
      permissionsApi,
      rowTextFromOs,
      rendererProbeWhenOsDenies,
      rowTextBeforeRefocus,
      rowTextAfterRefocus,
      clickedOpenSystemSettings,
      openedSystemSettingsPanes,
      rowTextAfterGrant,
      openSystemSettingsAfterGrant,
    })}\n`);
    window.destroy();
    app.exit(0);
  } catch (error) {
    process.stderr.write(`${error?.stack ?? String(error)}\n`);
    window.destroy();
    app.exit(1);
  }
}

void main();
