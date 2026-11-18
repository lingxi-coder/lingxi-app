import assert from 'node:assert/strict';
import { writeFileSync } from 'node:fs';
import { app, BrowserWindow } from 'electron';
import { configureFixtureWindow, waitForFixture } from './electron-test-environment.mjs';

const url = process.argv.find(value => value.startsWith('http://'));
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);

async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 1200, height: 850,
    webPreferences: { sandbox: true, backgroundThrottling: false } });
  const contents = window.webContents;
  contents.on('console-message', event => {
    if (event.level === 'error') console.error('Renderer:', event.message);
  });
  const run = script => contents.executeJavaScript(script);
  const wait = expression => waitForFixture(contents, expression);
  const delay = ms => new Promise(resolve => setTimeout(resolve, ms));

  try {
    await window.loadURL(url);
    await configureFixtureWindow(window);
    console.error('Waiting for Inbox rows');
    await wait("window.inboxFixture && document.querySelectorAll('.lx-inbox-row').length === 3");
    await wait("document.hasFocus() && document.visibilityState === 'visible'");
    const clickButton = name => run(`(() => {
      const button = [...document.querySelectorAll('button')].find(node => node.textContent.trim() === ${JSON.stringify(name)} && node.offsetWidth > 0);
      if (!button) throw new Error('Button unavailable: ' + ${JSON.stringify(name)});
      button.click();
    })()`);
    const row = name => `document.querySelector('[data-row-key="' + window.inboxFixture.${name} + '"]')`;
    const clickRow = name => run(`(() => {
      const button = ${row(name)};
      button.focus();
      const activatedAt = Date.now();
      button.click();
      return activatedAt;
    })()`);
    const openMore = () => run("document.querySelector('.lx-inbox-more summary').click()");
    const current = name => `${row(name)}?.getAttribute('aria-current') === 'page'`;
    const hasMainText = text => `document.querySelector('.desktop-main')?.innerText.includes(${JSON.stringify(text)})`;
    const rowCount = count => `document.querySelectorAll('.lx-inbox-row').length === ${count}`;
    const waitHome = () => wait("document.querySelector('.desktop-inbox-panel') && document.querySelector('.desktop-rail-button[title=Home][aria-current=page]')");
    const request = (action, name, extra = 'true') => `window.inboxFixture.requests.find(({uid, command}) =>
      uid === 'alice' && command.action === '${action}' && command.key === window.inboxFixture.${name} && (${extra}))`;
    const visible = (name, generation) => `window.inboxFixture.visible.at(-1)?.key === window.inboxFixture.${name}
      && window.inboxFixture.visible.at(-1)?.generation === ${generation}`;
    const screenshot = async () => {
      if (process.env.LINGXI_INBOX_SCREENSHOT) {
        writeFileSync(process.env.LINGXI_INBOX_SCREENSHOT, (await contents.capturePage()).toPNG());
      }
    };

    await waitHome();
    console.error('Checking filters and row actions');
    assert.deepEqual(await run("[...document.querySelectorAll('.lx-inbox-row strong')].map(node => node.textContent)"),
      ['Release checklist', 'Bob', 'general']);
    assert.equal(await run("document.querySelector('.desktop-rail-button[title=Home] .desktop-rail-count')?.textContent"), '2');
    assert.equal(await run(`(() => {
      const home = document.querySelector('.desktop-rail-button[title=Home]');
      const button = home.getBoundingClientRect();
      const badge = home.querySelector('.desktop-rail-count').getBoundingClientRect();
      return badge.left >= button.left && badge.right <= button.right + 3
        && badge.top >= button.top - 1 && badge.bottom <= button.bottom;
    })()`), true, 'position the unread count on the Home button');

    await clickButton('Attention');
    await wait(`${rowCount(1)} && ${row('assistant')}`);
    await clickButton('Unread');
    await wait(`${rowCount(2)} && ${row('assistant')} && ${row('friend')}`);
    await clickButton('All');
    await wait(rowCount(3));
    await run(`(() => {
      const select = document.querySelector('.lx-inbox-tools select');
      select.value = 'team'; select.dispatchEvent(new Event('change', { bubbles: true }));
    })()`);
    await wait(`${rowCount(1)} && ${row('channel')}`);
    await run(`(() => {
      const select = document.querySelector('.lx-inbox-tools select');
      select.value = ''; select.dispatchEvent(new Event('change', { bubbles: true }));
    })()`);
    await wait(rowCount(3));

    await run("document.querySelector('button[aria-label=\"Actions for Bob\"]').click()");
    await clickButton('Pin');
    await wait(`${request('preferences', 'friend', 'typeof command.patch.pinOrder === "number"')} &&
      document.querySelector('.lx-inbox-row').dataset.rowKey === window.inboxFixture.friend`);
    await clickButton('Archive');
    await wait(`${rowCount(2)} && !${row('friend')}`);
    await openMore();
    await clickButton('Archived');
    await wait(`${rowCount(1)} && ${row('friend')}`);
    await run("document.querySelector('button[aria-label=\"Actions for Bob\"]').click()");
    await clickButton('Archive');
    await wait(rowCount(0));
    await openMore();
    await clickButton('Archived');
    await wait(rowCount(3));

    await clickButton('Projects and workspaces');
    await wait("document.querySelector('button[title=\"/fixture/Inbox Project\"]') && !document.querySelector('.desktop-inbox-panel')");
    await clickButton('All chats');
    await wait(rowCount(3));

    console.error('Checking Friends and Teams routes');
    await clickRow('friend');
    await wait(`${hasMainText('The design is ready for your review.')} && document.querySelector('.desktop-inbox-panel')`);
    await waitHome();
    await wait(`${visible('friend', 1)} && ${current('friend')}`);
    assert.equal(await run(`document.activeElement === ${row('friend')}`), true);
    await wait(request('markRead', 'friend', 'command.through === 1 && command.generation === 1'));
    await wait(`!${row('friend')}.querySelector('b[aria-label]')`);

    await clickRow('channel');
    await wait(`${hasMainText('Build 42 passed the smoke checks.')} && document.querySelector('.desktop-inbox-panel')`);
    await waitHome();
    await wait(`${visible('channel', 3)} && ${current('channel')}`);
    assert.equal(await run(`document.activeElement === ${row('channel')}`), true);
    await screenshot();

    // Notification activation can show a committed result but is a preview.
    console.error('Checking Assistant route and reading');
    await run('window.inboxFixture.notifyAssistant()');
    await wait("window.inboxFixture.selected() === 'assistant-result' && document.querySelector('.desktop-assistant-notice-preview')?.textContent.includes('The compatibility report is ready.')");
    await delay(450);
    assert.equal(Boolean(await run(request('markRead', 'assistant'))), false, 'notification preview must preserve unread');

    const activatedAt = await clickRow('assistant');
    await wait("window.inboxFixture.opens.filter(id => id === 'assistant-result').length === 2");
    await run('window.inboxFixture.resumeAssistant()');
    await wait(`${visible('assistant', 1)} && ${current('assistant')}`);
    await wait(request('markRead', 'assistant', 'command.through === 1 && command.generation === 1 && command.observedPendingActionRevision === 2'));
    const elapsed = await run(`${request('markRead', 'assistant')}.at - ${activatedAt}`);
    assert.ok(elapsed >= 295, `committed result became read after only ${elapsed}ms`);
    await wait(`!${row('assistant')}.querySelector('b[aria-label]')`);
    assert.match(await run(`${row('assistant')}.textContent`), /Approval needed/);
    await waitHome();

    // Leaving the live transcript cancels pending browser timers without throwing during Stage cleanup.
    await run("document.querySelector('button[aria-label=\"New assistant conversation\"]').click()");
    await wait("window.inboxFixture.selected() === 'draft' && document.querySelector('.desktop-stage')");
    await waitHome();
    await wait("!document.querySelector('.lx-inbox-row[aria-current=page]')");
    console.log(JSON.stringify({ assistantReadMs: elapsed }));
  } catch (error) {
    console.error(error);
    console.error(await run("document.body.innerText").catch(() => 'renderer unavailable'));
    console.error(await run("JSON.stringify(window.inboxFixture?.requests.slice(-15))").catch(() => 'fixture unavailable'));
    process.exitCode = 1;
  } finally {
    window.destroy();
    app.exit(process.exitCode ?? 0);
  }
}

void main().catch(error => { console.error(error); app.exit(1); });
