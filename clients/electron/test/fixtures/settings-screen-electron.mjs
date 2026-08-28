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

async function runLayerSwitcherScenario(webContents) {
  await webContents.executeJavaScript('window.__settingsScreenTest.selectPage("permissions")');
  const permissions = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  await webContents.executeJavaScript('window.__settingsScreenTest.selectPage("diagnostics")');
  const diagnostics = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  return { permissions, diagnostics };
}

async function runProjectTabsScenario(webContents) {
  await webContents.executeJavaScript('window.__settingsScreenTest.selectPage("permissions")');
  const withoutProject = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  await webContents.executeJavaScript('window.__settingsScreenTest.setHasProject(true)');
  const withProject = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  return { withoutProject, withProject };
}

async function runPendingBannerScenario(webContents) {
  const noSnapshot = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  await webContents.executeJavaScript(
    'window.__settingsScreenTest.setSnapshot({ model: "opus", theme: "dark" }, { model: "sonnet", theme: "dark" })',
  );
  const pending = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  await webContents.executeJavaScript('window.__settingsScreenTest.setRunning(true)');
  const midTurn = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  await webContents.executeJavaScript('window.__settingsScreenTest.setRunning(false)');
  await webContents.executeJavaScript('window.__settingsScreenTest.clickRestart()');
  await waitFor(webContents, 'window.__settingsScreenTest.state().restartCalls === 1');
  const afterRestart = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  await webContents.executeJavaScript(
    'window.__settingsScreenTest.setSnapshot({ model: "opus" }, { model: "opus" })',
  );
  const resolved = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  return { noSnapshot, pending, midTurn, afterRestart, resolved };
}

async function runPlaceholderScenario(webContents) {
  await webContents.executeJavaScript('window.__settingsScreenTest.selectPage("voice")');
  const voice = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  await webContents.executeJavaScript('window.__settingsScreenTest.selectPage("diagnostics")');
  const diagnostics = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  return { voice, diagnostics };
}

async function runMalformedSnapshotScenario(webContents) {
  await webContents.executeJavaScript('window.__settingsScreenTest.setMalformedSnapshot()');
  const state = await webContents.executeJavaScript('window.__settingsScreenTest.state()');
  return { state };
}

async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 1000, height: 720, webPreferences: { sandbox: true } });
  try {
    await window.loadURL(url);
    const { webContents } = window;
    webContents.focus();
    await waitFor(webContents, 'Boolean(window.__settingsScreenTest && document.querySelector(\'[role="dialog"]\'))');
    const scenario = process.env.LINGXI_SETTINGS_SCREEN_SCENARIO ?? 'layer-switcher';
    const result = scenario === 'project-tabs' ? await runProjectTabsScenario(webContents)
      : scenario === 'pending-banner' ? await runPendingBannerScenario(webContents)
      : scenario === 'placeholder' ? await runPlaceholderScenario(webContents)
      : scenario === 'malformed-snapshot' ? await runMalformedSnapshotScenario(webContents)
      : await runLayerSwitcherScenario(webContents);
    process.stdout.write(`${JSON.stringify(result)}\n`);
  } finally {
    if (!window.isDestroyed()) window.destroy();
    if (app.isReady()) await app.quit();
  }
}

main().catch((error) => { process.stderr.write(`${error instanceof Error ? error.stack : String(error)}\n`); app.exit(1); });
