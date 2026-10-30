import assert from 'node:assert/strict';
import { app, BrowserWindow } from 'electron';

const url = process.argv.find(argument => argument.startsWith('file://'));
if (!url || !process.env.LINGXI_TEST_USER_DATA) throw new Error('fixture URL and isolated user data required');
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));

async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 1100, height: 800, webPreferences: { sandbox: true } });
  try {
    const run = script => window.webContents.executeJavaScript(script);
    const wait = async expression => {
      const deadline = Date.now() + 6000;
      while (Date.now() < deadline) { if (await run(expression)) return; await delay(20); }
      throw new Error(`Timed out: ${expression}\n${await run('document.body.innerText')}\n${JSON.stringify(await run('window.archiveAppFixture?.state()'))}`);
    };
    const fresh = async () => {
      await window.loadURL(url);
      window.showInactive(); window.webContents.focus();
      await wait(`Boolean(document.querySelector('button[data-session-id="A"]'))`);
    };
    const openArchive = async (id, ready = true) => {
      await run(`(() => { const row = document.querySelector('button[data-session-id="${id}"]'); row.focus();
        row.dispatchEvent(new KeyboardEvent('keydown', { key: 'F10', shiftKey: true, bubbles: true, cancelable: true })); })()`);
      await wait(`document.querySelectorAll('.session-actions-menu [role="menuitem"]').length === 3`);
      await run(`document.querySelector('.session-actions-menu [role="menuitem"]:nth-child(3)').click()`);
      await wait(`Boolean(document.querySelector('[role=dialog]'))${ready ? " && !document.querySelector('.desktop-dialog-action--primary').disabled" : ''}`);
    };

    // The archived row is not necessarily the active conversation. Reopening
    // the already active session must not dismiss that row's dialog.
    await fresh();
    await openArchive('B');
    assert.match(await run(`document.querySelector('[role=dialog]').textContent`), /Chat B/);
    await run(`window.archiveAppFixture.deliverSession('a')`);
    await wait(`window.archiveAppFixture.state().opened === 'a' && window.archiveAppFixture.state().acknowledgements.length === 1`);
    assert.match(await run(`document.querySelector('[role=dialog]')?.textContent ?? ''`), /Chat B/,
      'same-session navigation must not dismiss a non-current target');
    await run(`window.archiveAppFixture.holdNextNavigation(); window.archiveAppFixture.deliverSession('B')`);
    await wait(`window.archiveAppFixture.state().navigationHeld`);
    assert.equal(await run(`Boolean(document.querySelector('[role=dialog]'))`), false,
      'a different session must close the old dialog before navigation settles');
    assert.equal(await run(`document.activeElement === document.querySelector('button[data-session-id="B"]')`), false,
      'closing for navigation must not restore the archived row focus');
    await run(`window.archiveAppFixture.releaseNavigation()`);
    await wait(`window.archiveAppFixture.state().opened === 'B' && window.archiveAppFixture.state().acknowledgements.length === 2`);

    // Archiving a non-current row is still a valid operation.
    await fresh();
    await openArchive('B');
    await run(`document.querySelector('.desktop-dialog-action--primary').click()`);
    await wait(`!document.querySelector('[role=dialog]') && !document.querySelector('button[data-session-id="B"]')`);
    assert.equal((await run(`window.archiveAppFixture.state()`)).archiveCalls, 1);
    assert.equal(await run(`Boolean(document.querySelector('button[data-session-id="A"]'))`), true);

    // Settings and its nested project/archived-chat pages receive the same
    // navigation wrappers as the App deep-link handler.
    await fresh();
    await openArchive('A');
    await run(`window.archiveAppFixture.openSettings()`);
    await wait(`Boolean(document.querySelector('#settings-open-b'))`);
    await run(`document.querySelector('#settings-open-b').click()`);
    await wait(`window.archiveAppFixture.state().opened === 'B' && !document.querySelector('[role=dialog]')`);

    // The host serializes archive and navigation; response delivery to the old
    // caller can still lag B opening and B's own archive dialog.
    for (const outcome of ['failure', 'success']) {
      await fresh();
      await run(`window.archiveAppFixture.holdArchive(${JSON.stringify(outcome)})`);
      await openArchive('A');
      await run(`document.querySelector('.desktop-dialog-action--primary').click()`);
      await wait(`window.archiveAppFixture.state().hostArchiveHeld`);
      await run(`window.archiveAppFixture.deliverSession('B')`);
      await wait(`!document.querySelector('[role=dialog]')`);
      assert.equal((await run(`window.archiveAppFixture.state()`)).opened, null,
        'B must wait for the held host archive operation');
      assert.equal(await run(`document.activeElement === document.querySelector('button[data-session-id="A"]')`), false,
        'A focus must not be restored while B is opening');
      await run(`window.archiveAppFixture.releaseHostArchive()`);
      await wait(`window.archiveAppFixture.state().opened === 'B' && window.archiveAppFixture.state().archiveDeliveryHeld`);
      await openArchive('B');
      await run(`window.__newArchiveDialog = document.querySelector('[role=dialog]'); window.__newArchiveFocus = document.activeElement`);
      await run(`window.archiveAppFixture.releaseArchiveDelivery()`);
      await wait(`window.archiveAppFixture.state().completedArchiveDeliveries === 1`);
      await delay(50);
      assert.equal(await run(`document.querySelector('[role=dialog]') === window.__newArchiveDialog`), true,
        `old ${outcome} must not close the new dialog`);
      assert.equal(await run(`Boolean(document.querySelector('[role=dialog] [role=alert]'))`), false,
        `old ${outcome} must not show an error in the new dialog`);
      assert.equal(await run(`document.activeElement === window.__newArchiveFocus`), true,
        `old ${outcome} must not move focus from the new dialog`);
      assert.match(await run(`document.querySelector('[role=dialog]').textContent`), /Chat B/);
    }
    for (const outcome of ['failure', 'success']) {
      await fresh();
      await run(`window.archiveAppFixture.holdPreflight(${JSON.stringify(outcome)})`);
      await openArchive('A', false);
      await wait(`window.archiveAppFixture.state().preflightHeld`);
      await run(`window.archiveAppFixture.deliverSession('B')`);
      await wait(`!document.querySelector('[role=dialog]') && window.archiveAppFixture.state().opened === 'B'`);
      await openArchive('B');
      await run(`window.__newArchiveDialog = document.querySelector('[role=dialog]'); window.__newArchiveFocus = document.activeElement`);
      await run(`window.archiveAppFixture.releasePreflight()`);
      await wait(`window.archiveAppFixture.state().completedPreflights === 1`);
      await delay(50);
      assert.equal(await run(`document.querySelector('[role=dialog]') === window.__newArchiveDialog`), true);
      assert.equal(await run(`Boolean(document.querySelector('[role=dialog] [role=alert]'))`), false,
        `old preflight ${outcome} must not show an error in the new dialog`);
      assert.equal(await run(`document.querySelector('[role=dialog]').textContent.includes('Old task')`), false,
        `old preflight ${outcome} must not add A's tasks to B's dialog`);
      assert.equal(await run(`document.activeElement === window.__newArchiveFocus`), true);
    }
    process.stdout.write(`${JSON.stringify({ ok: true })}\n`);
  } finally { window.destroy(); app.quit(); }
}
main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); app.exit(1); });
