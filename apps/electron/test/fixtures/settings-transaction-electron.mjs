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
  await waitFor(webContents, `Boolean(document.querySelector('[data-testid^="provider-account-test-"]'))`);
  const connectionTestButtonState = await webContents.executeJavaScript('window.__settingsTransactionTest.connectionTestButtonState()');
  if (!connectionTestButtonState.exists || connectionTestButtonState.disabled) {
    throw new Error(`Anthropic connection-test prerequisites are not ready: ${JSON.stringify(connectionTestButtonState)}`);
  }
  await webContents.executeJavaScript('window.__settingsTransactionTest.startConnectionTest()');
  await waitFor(webContents, 'window.__settingsTransactionTest.connectionTestErrorVisible()');
  const connectionTestCalls = await webContents.executeJavaScript('window.__settingsTransactionTest.connectionTestCalls()');
  if (connectionTestCalls !== 1) throw new Error(`expected one rejected Anthropic connection test, observed ${connectionTestCalls}`);
  const connectionTestErrorBeforeDelete = await webContents.executeJavaScript('window.__settingsTransactionTest.connectionTestErrorVisible()');
  await webContents.executeJavaScript('window.__settingsTransactionTest.clearStoredCredential()');
  await waitFor(webContents, 'Boolean(document.querySelector("dialog[open] .pa-danger-button"))');
  await webContents.executeJavaScript('window.__settingsTransactionTest.confirmCredentialRemoval()');
  await waitFor(webContents, '!window.__settingsTransactionTest.connectionTestErrorVisible()');
  const connectionTestErrorAfterDelete = await webContents.executeJavaScript('window.__settingsTransactionTest.connectionTestErrorVisible()');
  await waitFor(webContents, '!document.querySelector("dialog[open] .pa-danger-button")');
  await webContents.executeJavaScript('window.__settingsTransactionTest.openAddAccount()');
  await waitFor(webContents, 'window.__settingsTransactionTest.connectionTestButtonState().providerCredentialField');
  await webContents.executeJavaScript('window.__settingsTransactionTest.startSave()');
  await waitFor(webContents, 'window.__settingsTransactionTest.state().persistencePending');
  const busyState = await webContents.executeJavaScript(`({
    closeDisabled: document.querySelector('button[aria-label="Back to app"]')?.disabled ?? true,
    state: window.__settingsTransactionTest.state(),
  })`);
  // The account form is modal while saving. Simulate the owning settings surface
  // being torn down by navigation/window lifecycle, not an impossible click
  // through that modal.
  await webContents.executeJavaScript('window.__settingsTransactionTest.closeOwner()');
  await waitFor(webContents, '!document.querySelector(\'[role="dialog"]\')');
  const afterClose = await webContents.executeJavaScript('window.__settingsTransactionTest.state()');
  await webContents.executeJavaScript('window.__settingsTransactionTest.switchSessionAndStartWork()');
  await waitFor(webContents, 'document.querySelector("[data-testid=\\"settings-transaction-owner\\"]")?.dataset.sessionId === "session-b"');
  await webContents.executeJavaScript('window.__settingsTransactionTest.resolvePersistence()');
  await waitFor(webContents, '!window.__settingsTransactionTest.state().persistencePending');
  const afterLateResult = await webContents.executeJavaScript('window.__settingsTransactionTest.state()');
  return { connectionTestErrorBeforeDelete, connectionTestErrorAfterDelete, busyState, afterClose, afterLateResult };
}

async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 900, height: 700, webPreferences: { sandbox: true } });
  try {
    await window.loadURL(url);
    const { webContents } = window;
    webContents.focus();
    await waitFor(webContents, 'Boolean(window.__settingsTransactionTest && document.querySelector(\'[role="dialog"]\'))');
    const result = await runCloseScenario(webContents);
    process.stdout.write(`${JSON.stringify(result)}\n`);
  } finally {
    if (!window.isDestroyed()) window.destroy();
    if (app.isReady()) await app.quit();
  }
}

main().catch((error) => { process.stderr.write(`${error instanceof Error ? error.stack : String(error)}\n`); app.exit(1); });
