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
  const run = expression => webContents.executeJavaScript(expression);
  const escape = () => {
    webContents.sendInputEvent({ type: 'keyDown', keyCode: 'ESC' });
    webContents.sendInputEvent({ type: 'keyUp', keyCode: 'ESC' });
  };
  await waitFor(webContents, `Boolean(document.querySelector('[data-testid^="provider-account-test-"]'))`);
  const connectionTestButtonState = await run('window.__settingsTransactionTest.connectionTestButtonState()');
  if (!connectionTestButtonState.exists || connectionTestButtonState.disabled || connectionTestButtonState.accountCount !== 1) {
    throw new Error(`Anthropic connection-test prerequisites are not ready: ${JSON.stringify(connectionTestButtonState)}`);
  }
  await run('window.__settingsTransactionTest.startConnectionTest()');
  await waitFor(webContents, 'window.__settingsTransactionTest.connectionTestErrorVisible()');
  const connectionTestCalls = await run('window.__settingsTransactionTest.connectionTestCalls()');
  if (connectionTestCalls !== 1) throw new Error(`expected one rejected Anthropic connection test, observed ${connectionTestCalls}`);
  const connectionTestErrorBeforeDelete = await run('window.__settingsTransactionTest.connectionTestErrorVisible()');
  await run('document.querySelector(".pa-account .pa-more summary").click()');
  await run('document.querySelector(".pa-account .pa-menu .pa-destructive").click()');
  await waitFor(webContents, '!window.__settingsTransactionTest.connectionTestErrorVisible()');
  const connectionTestErrorAfterDelete = await run('window.__settingsTransactionTest.connectionTestErrorVisible()');
  await waitFor(webContents, 'Boolean(document.querySelector("dialog[open] .pa-danger-button"))');
  await run('document.querySelector("dialog[open] .pa-danger-button").click()');
  await waitFor(webContents, 'window.__settingsTransactionTest.state().removalPending && document.querySelector("dialog[open] .pa-danger-button").disabled');
  escape();
  await delay(50);
  const pendingRemoval = await run('({modalOpen:Boolean(document.querySelector("dialog[open] .pa-danger-button")), accountCount:document.querySelectorAll(".pa-account").length, state:window.__settingsTransactionTest.state()})');
  if (!pendingRemoval.modalOpen || pendingRemoval.accountCount !== 1 || !pendingRemoval.state.removalPending || pendingRemoval.state.closeCalls !== 0) {
    throw new Error(`Escape changed the pending account removal: ${JSON.stringify(pendingRemoval)}`);
  }
  await run('window.__settingsTransactionTest.releaseRemoval()');
  await waitFor(webContents, '!document.querySelector(".pa-account") && !document.querySelector("dialog[open]")');

  const beginWrite = async credential => {
    await run('document.querySelector(".pa-toolbar .pa-primary").click()');
    await waitFor(webContents, 'Boolean(document.querySelector("dialog[open] input[type=password]"))');
    await run(`window.__settingsTransactionTest.setCredential(${JSON.stringify(credential)})`);
    await waitFor(webContents, '!document.querySelector("dialog[open] button[type=submit]").disabled');
    await run('document.querySelector("dialog[open] button[type=submit]").click()');
    await waitFor(webContents, 'window.__settingsTransactionTest.state().persistencePending');
    await waitFor(webContents, 'document.querySelector("dialog[open] button[type=button]").disabled');
  };

  await beginWrite('sk-failed-save');
  escape();
  await delay(50);
  const busyState = await run(`({
    modalOpen: Boolean(document.querySelector('dialog[open]')),
    cancelDisabled: document.querySelector('dialog[open] button[type=button]')?.disabled ?? false,
    settingsOpen: Boolean(document.querySelector('[role="dialog"]')),
    state: window.__settingsTransactionTest.state(),
  })`);
  if (!busyState.modalOpen || !busyState.cancelDisabled || !busyState.settingsOpen || busyState.state.closeCalls !== 0) {
    throw new Error(`Escape changed the pending transaction: ${JSON.stringify(busyState)}`);
  }
  await run('window.__settingsTransactionTest.settlePersistence(true)');
  try {
    await waitFor(webContents, 'document.querySelector("dialog[open] [role=alert]")?.textContent.includes("test credential storage failed")');
  } catch (error) {
    const detail = await run('({dialogs:[...document.querySelectorAll("dialog")].map(d=>({open:d.open,text:d.innerText})), state:window.__settingsTransactionTest.state()})');
    throw new Error(`Credential failure was not shown: ${JSON.stringify(detail)}`, { cause: error });
  }
  const afterFailure = await run('({state:window.__settingsTransactionTest.state(), accountCount:document.querySelectorAll(".pa-account").length})');
  escape();
  await waitFor(webContents, '!document.querySelector("dialog[open]")');
  const afterIdleEscape = await run('window.__settingsTransactionTest.state()');

  await beginWrite('sk-successful-save');
  await run('window.__settingsTransactionTest.settlePersistence(false)');
  await waitFor(webContents, '!document.querySelector("dialog[open]") && document.querySelectorAll(".pa-account").length === 1');
  const afterSuccess = await run('({state:window.__settingsTransactionTest.state(), accountCount:document.querySelectorAll(".pa-account").length, successVisible:document.body.innerText.includes("Account saved")})');
  escape();
  await waitFor(webContents, '!document.querySelector(\'[role="dialog"]\')');
  const afterClose = await run('window.__settingsTransactionTest.state()');
  await run('window.__settingsTransactionTest.switchSessionAndStartWork()');
  await waitFor(webContents, 'document.querySelector("[data-testid=\\"settings-transaction-owner\\"]")?.dataset.sessionId === "session-b"');
  const afterSessionSwitch = await run('window.__settingsTransactionTest.state()');
  return { connectionTestErrorBeforeDelete, connectionTestErrorAfterDelete, pendingRemoval, busyState, afterFailure, afterIdleEscape, afterSuccess, afterClose, afterSessionSwitch };
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
