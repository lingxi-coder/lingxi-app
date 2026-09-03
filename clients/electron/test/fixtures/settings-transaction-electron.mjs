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

async function runCloseScenario(webContents) {
  await webContents.executeJavaScript('window.__settingsTransactionTest.startConnectionTest()');
  await waitFor(webContents, 'window.__settingsTransactionTest.connectionTestErrorVisible()');
  const connectionTestErrorBeforeDelete = await webContents.executeJavaScript('window.__settingsTransactionTest.connectionTestErrorVisible()');
  await webContents.executeJavaScript('window.__settingsTransactionTest.clearStoredCredential()');
  await waitFor(webContents, '!window.__settingsTransactionTest.connectionTestErrorVisible()');
  const connectionTestErrorAfterDelete = await webContents.executeJavaScript('window.__settingsTransactionTest.connectionTestErrorVisible()');
  await webContents.executeJavaScript('window.__settingsTransactionTest.startSave()');
  await waitFor(webContents, 'window.__settingsTransactionTest.state().persistencePending');
  const busyState = await webContents.executeJavaScript(`({
    closeDisabled: document.querySelector('button[aria-label="Close settings"]')?.disabled ?? true,
    state: window.__settingsTransactionTest.state(),
  })`);
  webContents.sendInputEvent({ type: 'keyDown', keyCode: 'ESC' });
  webContents.sendInputEvent({ type: 'keyUp', keyCode: 'ESC' });
  await waitFor(webContents, '!document.querySelector(\'[role="dialog"]\')');
  const afterClose = await webContents.executeJavaScript('window.__settingsTransactionTest.state()');
  await webContents.executeJavaScript('window.__settingsTransactionTest.switchSessionAndStartWork()');
  await webContents.executeJavaScript('window.__settingsTransactionTest.resolvePersistence()');
  await waitFor(webContents, '!window.__settingsTransactionTest.state().persistencePending');
  const afterLateResult = await webContents.executeJavaScript('window.__settingsTransactionTest.state()');
  return { connectionTestErrorBeforeDelete, connectionTestErrorAfterDelete, busyState, afterClose, afterLateResult };
}

async function runRecoveryScenario(webContents) {
  await webContents.executeJavaScript('window.__settingsTransactionTest.startSave()');
  await waitFor(webContents, 'window.__settingsTransactionTest.state().persistencePending');
  await webContents.executeJavaScript('window.__settingsTransactionTest.setSessionState("session-b", true)');
  await webContents.executeJavaScript('window.__settingsTransactionTest.resolvePersistence()');
  await waitFor(
    webContents,
    'window.__settingsTransactionTest.state().restartErrors === 1 && Boolean([...document.querySelectorAll("button")].find((button) => button.textContent?.trim() === "重试引擎连接"))',
  );
  const afterFirstFailure = await webContents.executeJavaScript('window.__settingsTransactionTest.state()');
  await webContents.executeJavaScript('window.__settingsTransactionTest.setSessionState("session-b", false)');
  await webContents.executeJavaScript('window.__settingsTransactionTest.clickRetry()');
  await waitFor(webContents, 'window.__settingsTransactionTest.state().restartErrors === 2');
  const afterWrongSessionRetry = await webContents.executeJavaScript('window.__settingsTransactionTest.state()');
  await webContents.executeJavaScript('window.__settingsTransactionTest.setSessionState("session-a", false)');
  await webContents.executeJavaScript('window.__settingsTransactionTest.clickRetry()');
  await waitFor(webContents, 'window.__settingsTransactionTest.state().restartCalls === 1');
  const afterRecoveredRetry = await webContents.executeJavaScript('window.__settingsTransactionTest.state()');
  return { afterFirstFailure, afterWrongSessionRetry, afterRecoveredRetry };
}

async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 900, height: 700, webPreferences: { sandbox: true } });
  try {
    await window.loadURL(url);
    const { webContents } = window;
    webContents.focus();
    await waitFor(webContents, 'Boolean(window.__settingsTransactionTest && document.querySelector(\'[role="dialog"]\'))');
    const scenario = process.env.LINGXI_SETTINGS_SCENARIO ?? 'close';
    const result = scenario === 'recovery'
      ? await runRecoveryScenario(webContents)
      : await runCloseScenario(webContents);
    process.stdout.write(`${JSON.stringify(result)}\n`);
  } finally {
    if (!window.isDestroyed()) window.destroy();
    if (app.isReady()) await app.quit();
  }
}

main().catch((error) => { process.stderr.write(`${error instanceof Error ? error.stack : String(error)}\n`); app.exit(1); });
