import assert from 'node:assert/strict';
import { app, BrowserWindow } from 'electron';
import { configureFixtureWindow, waitForFixture } from './electron-test-environment.mjs';

const url = process.argv.find(argument => argument.startsWith('http://'));
if (!url || !process.env.LINGXI_TEST_USER_DATA) throw new Error('isolated fixture required');
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
app.on('window-all-closed', () => {});
async function main() {
  await app.whenReady();
  app.dock?.hide();
  const window = new BrowserWindow({ show: false, width: 900, height: 600, webPreferences: { sandbox: true } });
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
    // Chromium skips offscreen content-visibility:auto descendants. Keep the
    // real Stage, restored state and styles; move only the viewport away.
    await run(`document.querySelector('.desktop-main').style.transform = 'translateY(4000px)'`);
    await waitForFixture(window.webContents, `!document.body.innerText.includes('Custom provider verified.')`);
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
    await run(`document.querySelector('.desktop-main').style.transform = ''`);
    window.showInactive();
    await waitForFixture(window.webContents, `document.body.innerText.includes('Custom provider verified.')`);
    evidence.push({ scenario: 'visible-again', restored: true });
    process.stdout.write(`COLD_RESTORE_RESULT:${JSON.stringify(evidence)}\n`);
  } finally { window.destroy(); }
}
main().then(() => app.exit(0), error => { console.error(error); app.exit(1); });
