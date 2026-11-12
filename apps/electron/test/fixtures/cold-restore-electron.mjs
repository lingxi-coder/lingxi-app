import assert from 'node:assert/strict';
import { app, BrowserWindow } from 'electron';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { configureFixtureWindow, waitForFixture } from './electron-test-environment.mjs';

const url = process.argv.find(argument => argument.startsWith('http://'));
if (!url || !process.env.LINGXI_TEST_USER_DATA) throw new Error('isolated fixture required');
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const ownsInstance = app.requestSingleInstanceLock();
let fixtureWindow;
app.on('second-instance', () => {
  fixtureWindow?.show();
  fixtureWindow?.focus();
});
app.on('window-all-closed', () => {});
async function main() {
  await app.whenReady();
  app.dock?.hide();
  const window = new BrowserWindow({ show: false, width: 900, height: 600, webPreferences: { sandbox: true } });
  fixtureWindow = window;
  window.webContents.on('console-message', (_event, level, message) => { if (level >= 2) console.error(message); });
  const run = script => window.webContents.executeJavaScript(script);
  const evidence = [];
  try {
    for (const scenario of ['normal', 'reject', 'stall']) {
      await window.loadURL(`${url}?${scenario}`);
      await configureFixtureWindow(window);
      await waitForFixture(window.webContents, `window.coldRestoreFixture?.().itemCount === 2 && document.querySelector('[data-message-role="assistant"]')?.textContent.includes('Custom provider verified.')`);
      evidence.push(await run(`({ scenario: ${JSON.stringify(scenario)}, ...window.coldRestoreFixture(),
        textContent: document.body.textContent.includes('Custom provider verified.'), innerText: document.body.innerText.includes('Custom provider verified.') })`));
    }
    // Reproduce the signed-package failure with a genuinely hidden native
    // window, rather than depending on Chromium's offscreen overscan heuristic.
    window.show();
    window.focus();
    await window.webContents.debugger.sendCommand('Emulation.setFocusEmulationEnabled', { enabled: false });
    window.webContents.setBackgroundThrottling(true);
    window.hide();
    await waitForFixture(window.webContents, `document.visibilityState === 'hidden'`);
    await window.loadURL(`${url}?normal`);
    await waitForFixture(window.webContents, `window.coldRestoreFixture?.().itemCount === 2 && document.visibilityState === 'hidden' && !document.body.innerText.includes('Custom provider verified.')`);
    const skipped = await run(`({ scenario: 'skipped-paint', ...window.coldRestoreFixture(),
      textContent: document.body.textContent.includes('Custom provider verified.'), innerText: document.body.innerText.includes('Custom provider verified.'),
      contentVisibility: getComputedStyle(document.querySelector('[data-message-role="assistant"] .transcript-run-item')).contentVisibility })`);
    assert.equal(skipped.textContent, true);
    assert.equal(skipped.innerText, false);
    assert.equal(skipped.contentVisibility, 'auto');
    const started = Date.now();
    await assert.rejects(waitForFixture(window.webContents,
      `document.body.innerText.includes('Custom provider verified.')`, 20_000), /Fixture condition timed out/);
    skipped.legacyTimeoutMs = Date.now() - started;
    assert.equal(await run('window.coldRestoreFixture().itemCount'), 2);
    assert.equal(await run(`document.querySelector('[data-message-role="assistant"]').textContent.includes('Custom provider verified.')`), true);
    evidence.push(skipped);
    await window.webContents.debugger.sendCommand('Page.bringToFront');
    assert.equal(await run('document.visibilityState'), 'hidden', 'CDP focus does not show a native hidden window');
    evidence.push({ scenario: 'native-hidden', restored: await run('window.coldRestoreFixture().itemCount === 2') });
    const second = spawn(process.execPath, [fileURLToPath(import.meta.url), url], { env: process.env, stdio: 'ignore' });
    try {
      const code = await new Promise((resolve, reject) => {
        const timer = setTimeout(() => reject(new Error('second launch did not exit')), 5000);
        second.once('exit', code => { clearTimeout(timer); resolve(code); });
        second.once('error', error => { clearTimeout(timer); reject(error); });
      });
      assert.equal(code, 0);
      await waitForFixture(window.webContents, `document.visibilityState === 'visible' && document.body.innerText.includes('Custom provider verified.')`);
      evidence.push({ scenario: 'second-launch-visible', restored: true });
      evidence.push({ scenario: 'visible-again', restored: true });
    } finally { if (second.exitCode === null && second.signalCode === null) second.kill('SIGTERM'); }
    process.stdout.write(`COLD_RESTORE_RESULT:${JSON.stringify(evidence)}\n`);
  } catch (error) {
    console.error('COLD_RESTORE_FAILURE:' + JSON.stringify(await run(`({ visibility: document.visibilityState,
      focused: document.hasFocus(), state: window.coldRestoreFixture?.(),
      innerText: document.body.innerText.includes('Custom provider verified.'), textContent: document.body.textContent.includes('Custom provider verified.') })`)));
    throw error;
  } finally { window.destroy(); }
}
if (ownsInstance) void main().then(() => app.exit(0), error => { console.error(error); app.exit(1); });
else app.quit();
