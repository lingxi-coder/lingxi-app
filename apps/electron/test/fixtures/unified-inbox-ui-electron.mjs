import assert from 'node:assert/strict';
import { writeFileSync } from 'node:fs';
import { app, BrowserWindow } from 'electron';
import { configureFixtureWindow, settleFixtureAnimations, waitForFixture } from './electron-test-environment.mjs';

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
    const rail = view => `document.querySelector('.desktop-rail-${view}')`;
    const clickRail = view => run(`${rail(view)}.click()`);
    const selectedView = view => `${rail(view)}?.getAttribute('aria-current') === 'page'`;
    const waitChats = () => wait(`document.querySelector('.desktop-inbox-panel') && ${selectedView('chats')}`);
    const request = (action, name, extra = 'true') => `window.inboxFixture.requests.find(({uid, command}) =>
      uid === 'alice' && command.action === '${action}' && command.key === window.inboxFixture.${name} && (${extra}))`;
    const visible = (name, generation) => `window.inboxFixture.visible.at(-1)?.key === window.inboxFixture.${name}
      && window.inboxFixture.visible.at(-1)?.generation === ${generation}`;
    const screenshot = async () => {
      if (process.env.LINGXI_INBOX_SCREENSHOT) {
        await settleFixtureAnimations(contents, '.desktop-nav-rail');
        writeFileSync(process.env.LINGXI_INBOX_SCREENSHOT, (await contents.capturePage()).toPNG());
      }
    };

    await waitChats();
    if (process.argv.includes('--inbox-toast')) {
      const toast = "document.querySelector('.desktop-inbox-toast')";
      await run('window.inboxFixture.notifyAssistant(false)');
      await wait(`${toast}?.querySelector('p')?.textContent === 'Task completed · The compatibility report is ready.'`);
      assert.equal(await run("document.querySelector('.desktop-inbox-toast-announcement[role=status]')?.textContent"),
        'Release checklist. Task completed · The compatibility report is ready.');
      if (process.env.LINGXI_INBOX_TOAST_SCREENSHOT) {
        await settleFixtureAnimations(contents, '.desktop-inbox-toast');
        const frame = await run(`(() => {
          const { left, bottom } = ${toast}.getBoundingClientRect();
          const x = Math.max(0, Math.floor(left) - 40);
          return { x, y: 0, width: innerWidth - x, height: Math.ceil(bottom) + 28 };
        })()`);
        writeFileSync(process.env.LINGXI_INBOX_TOAST_SCREENSHOT, (await contents.capturePage(frame)).toPNG());
      }
      await run(`${toast}.querySelector('button[aria-label="Dismiss notification"]').click()`);
      await wait(`!${toast}`);
      assert.equal(await run("window.inboxFixture.opens.includes('assistant-result')"), false, 'dismissing the toast must not open the conversation');

      await run('window.inboxFixture.notifyFriend()');
      await wait(`${toast}?.querySelector('p')?.textContent === 'New direct message · The design is ready for your review.'`);
      assert.equal(await run(`${toast}.querySelector('strong').textContent`), 'Bob');
      await run(`${toast}.dispatchEvent(new PointerEvent('pointerover', { bubbles: true }))`);
      await delay(8200);
      assert.equal(await run(`Boolean(${toast})`), true, 'hover pauses dismissal');
      await run(`${toast}.querySelector('button').focus()`);
      await run(`${toast}.dispatchEvent(new PointerEvent('pointerout', { bubbles: true }))`);
      await delay(8200);
      assert.equal(await run(`Boolean(${toast})`), true, 'keyboard focus pauses dismissal');
      await run(`${toast}.querySelector('button').blur()`);
      await waitForFixture(contents, `!${toast}`, 9000);
      assert.equal(Boolean(await run(request('markRead', 'friend'))), false, 'auto dismissal preserves unread');

      await run('window.inboxFixture.notifyAssistant(false)');
      await wait(toast);
      await run(`${toast}.querySelector('.desktop-inbox-toast-view').click()`);
      await wait(`!${toast}`);
      await wait("window.inboxFixture.selected() === 'assistant-result' && document.querySelector('.desktop-assistant-notice-preview')?.textContent.includes('The compatibility report is ready.')");
      await delay(450);
      assert.equal(Boolean(await run(request('markRead', 'assistant'))), false, 'notification preview must preserve unread');
      return;
    }
    console.error('Checking filters and row actions');
    assert.deepEqual(await run("[...document.querySelectorAll('.lx-inbox-row strong')].map(node => node.textContent)"),
      ['Release checklist', 'Bob', 'general']);
    assert.equal(await run("document.querySelector('.desktop-rail-chats .desktop-rail-count')?.textContent"), '2');
    assert.equal(await run(`(() => {
      const home = document.querySelector('.desktop-rail-chats');
      const button = home.getBoundingClientRect();
      const badge = home.querySelector('.desktop-rail-count').getBoundingClientRect();
      return badge.left >= button.left && badge.right <= button.right + 3
        && badge.top >= button.top - 1 && badge.bottom <= button.bottom;
    })()`), true, 'position the unread count on the Chats button');

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

    console.error('Checking independent Chats and Projects subviews');
    await clickButton('Unread');
    await wait(rowCount(2));
    assert.equal(await run("Boolean(document.querySelector('.desktop-nav-rail [title=Home], .desktop-home-tabs'))"), false, 'Chats and Projects replace Home in the main rail');
    assert.deepEqual(await run("[...document.querySelectorAll('.desktop-nav-rail button')].slice(0, 2).map(button => button.title)"), ['Chats', 'Projects']);
    await clickRail('projects');
    await wait(`${selectedView('projects')} && document.querySelector('.projects-sidebar-section')?.getBoundingClientRect().height > 0`);
    assert.equal(await run("document.querySelector('.desktop-inbox-panel').offsetHeight"), 0);
    assert.equal(await run("document.querySelectorAll('.desktop-home-view[role=region]:not([hidden])').length"), 1);
    assert.equal(await run("Boolean(document.querySelector('button[title=\"/fixture/Inbox Project\"]'))"), true);
    if (process.env.LINGXI_PROJECTS_SCREENSHOT) {
      await settleFixtureAnimations(contents, '.desktop-nav-rail');
      writeFileSync(process.env.LINGXI_PROJECTS_SCREENSHOT, (await contents.capturePage()).toPNG());
    }
    // Main navigation uses native Tab and Enter: focusing an item never navigates by itself.
    await run(`${rail('projects')}.focus()`);
    contents.sendInputEvent({ type: 'keyDown', keyCode: 'Tab', modifiers: ['shift'] });
    contents.sendInputEvent({ type: 'keyUp', keyCode: 'Tab', modifiers: ['shift'] });
    await wait(`document.activeElement === ${rail('chats')}`);
    assert.equal(await run(selectedView('projects')), true, 'focus alone preserves the active view');
    contents.sendInputEvent({ type: 'keyDown', keyCode: 'Enter' });
    contents.sendInputEvent({ type: 'char', keyCode: '\r' });
    contents.sendInputEvent({ type: 'keyUp', keyCode: 'Enter' });
    await wait(`${selectedView('chats')} && document.activeElement === ${rail('chats')}`);
    await wait(rowCount(2));
    assert.equal(await run("document.querySelector('.lx-inbox-switch button[aria-pressed=true]').textContent.trim()"), 'Unread');
    assert.equal(await run("window.inboxFixture.opens.length"), 0, 'changing subviews does not open a conversation');
    assert.equal(await run("window.inboxFixture.requests.filter(({command}) => command.action === 'markRead').length"), 0);
    await run("(() => { const list = document.querySelector('.lx-inbox-scroll'); list.style.maxHeight = '40px'; list.style.minHeight = '0'; list.scrollTop = 30; list.dispatchEvent(new Event('scroll')); })()");
    await wait("document.querySelector('.lx-inbox-scroll').scrollTop === 30");
    await clickRail('projects');
    await wait(selectedView('projects'));
    await run("document.querySelector('.projects-sidebar-actions button[aria-haspopup=menu]').click()");
    await wait("document.querySelector('.projects-sidebar-menu-layer')");
    await clickRail('chats');
    await wait("!document.querySelector('.projects-sidebar-menu-layer')");
    await wait(`${selectedView('chats')} && document.querySelector('.lx-inbox-scroll').scrollTop === 30`);
    await run("(() => { const list = document.querySelector('.lx-inbox-scroll'); list.style.maxHeight = ''; list.style.minHeight = ''; })()");
    await clickButton('All');
    await wait(rowCount(3));

    console.error('Checking Friends and Teams routes');
    await clickRow('friend');
    await wait(`${hasMainText('The design is ready for your review.')} && document.querySelector('.desktop-inbox-panel')`);
    await waitChats();
    await wait(`${visible('friend', 1)} && ${current('friend')}`);
    assert.equal(await run(`document.activeElement === ${row('friend')}`), true);
    await wait(request('markRead', 'friend', 'command.through === 1 && command.generation === 1'));
    await wait(`!${row('friend')}.querySelector('b[aria-label]')`);

    await clickRow('channel');
    await wait(`${hasMainText('Build 42 passed the smoke checks.')} && document.querySelector('.desktop-inbox-panel')`);
    await waitChats();
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
    await waitChats();

    // Leaving the live transcript cancels pending browser timers without throwing during Stage cleanup.
    await run("document.querySelector('button[aria-label=\"New assistant conversation\"]').click()");
    await wait("window.inboxFixture.selected() === 'draft' && document.querySelector('.desktop-stage')");
    await waitChats();
    await wait("!document.querySelector('.lx-inbox-row[aria-current=page]')");

    console.error('Checking Apps to Chats content restoration');
    await run("document.querySelector('.desktop-nav-rail button[title=Apps]').click()");
    await wait("document.querySelector('.local-apps-sidebar-disclosure')");
    await run("document.querySelector('.local-apps-sidebar-disclosure').click()");
    await wait("document.querySelector('.local-apps-sidebar-chats button')");
    await run("document.querySelector('.local-apps-sidebar-chats button').click()");
    await wait("window.inboxFixture.selected() === 'app-author' && document.querySelector('.desktop-nav-rail button[title=Apps][aria-current=page]')");
    await clickRail('chats');
    await wait(`${selectedView('chats')} && window.inboxFixture.selected() === 'draft'`);
    assert.equal(await run("document.querySelector('.local-apps-sidebar')?.offsetHeight ?? 0"), 0);

    // Large text and reduced motion retain usable first-level navigation.
    if (!contents.debugger.isAttached()) contents.debugger.attach('1.3');
    await contents.debugger.sendCommand('Emulation.setEmulatedMedia', { features: [
      { name: 'prefers-reduced-motion', value: 'reduce' }, { name: 'prefers-contrast', value: 'more' },
    ] });
    await run("document.documentElement.style.fontSize = '32px'");
    await clickRail('projects');
    await wait(selectedView('projects'));
    assert.equal(await run(`getComputedStyle(${rail('projects')}).transitionDuration`), '0s');
    assert.equal(await run(`(() => {
      const item = ${rail('projects')}.getBoundingClientRect();
      return item.width >= 24 && item.height >= 24 && item.left >= 0 && item.bottom <= innerHeight;
    })()`), true, 'large-text navigation stays visible and operable');
    await run(`${rail('chats')}.focus()`);
    contents.sendInputEvent({ type: 'keyDown', keyCode: 'Space' });
    contents.sendInputEvent({ type: 'char', keyCode: ' ' });
    contents.sendInputEvent({ type: 'keyUp', keyCode: 'Space' });
    await wait(selectedView('chats'));
    await run("document.documentElement.style.fontSize = ''");
    await contents.debugger.sendCommand('Emulation.setEmulatedMedia', { features: [] });
    contents.debugger.detach();
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
