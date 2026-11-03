import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { app, BrowserWindow } from 'electron';

const url = process.argv.find(arg => arg.startsWith('http://'));
// Opt-in: a directory to leave screenshots of the window in, for looking at the result (light and dark, Teams and Bots).
const screenshots = process.env.TEAMS_E2E_SCREENSHOTS;
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
async function main() {
await app.whenReady();
const win = new BrowserWindow({ show: false, width: 1280, height: 850, webPreferences: { sandbox: true, backgroundThrottling: false } });
const errors = [];
win.webContents.on('console-message', details => { if (details.level === 'error' || details.level === 3) errors.push(details.message); });
const run = script => win.webContents.executeJavaScript(script);
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function wait(expression, tries = 300) {
  for (let i = 0; i < tries; i++) { if (await run(expression)) return; await delay(20); }
  throw new Error(`Timed out: ${expression}\n${await run('document.body.innerText')}`);
}
// A fresh renderer: the Teams commands a window keeps in IndexedDB (a message sent, say) must not follow it into the next one.
async function load(query = '') {
  await win.webContents.session.clearStorageData({ storages: ['indexdb', 'localstorage'] });
  await win.loadURL(`${url}${query}`);
}
const click = selector => run(`document.querySelector(${JSON.stringify(selector)}).click()`);
const rail = name => `.desktop-nav-rail [aria-label="${name}"]`;
const railTeams = '.desktop-nav-rail .desktop-rail-teams';
const sidebar = '.teams-sidebar';
const row = id => `${sidebar} [data-row="${id}"]`;
// The marked buttons of the rail; the Teams button is named with its unread count, which these checks are not about.
const marked = () => run(`Array.from(document.querySelectorAll('.desktop-nav-rail button[data-active]')).map(button => button.getAttribute('aria-label').replace(/^Teams, \\d+ unread$/, 'Teams'))`);
const shown = selector => run(`document.querySelector(${JSON.stringify(selector)})?.getClientRects().length > 0`);
const calls = () => run('window.fixtureCalls');
const writes = async () => (await calls()).filter(call => call[0] === 'updateSettings');
const menuAction = action => run(`window.applicationMenuAction(${JSON.stringify(action)})`);
const rows = () => run(`Array.from(document.querySelectorAll('${sidebar} [data-row]')).map(button => button.getAttribute('data-row'))`);
const setValue = (selector, value) => run(`(() => { const element = document.querySelector(${JSON.stringify(selector)}); Object.getOwnPropertyDescriptor(Object.getPrototypeOf(element), 'value').set.call(element, ${JSON.stringify(value)}); element.dispatchEvent(new Event('input', { bubbles: true })); })()`);
const drawerWidth = () => run(`Math.round(document.querySelector('.tw-drawer').getBoundingClientRect().width)`);
const result = {};
try {
  // The rail counts the unread messages of every team, before Teams is open: the number for the eye, the label for the rest.
  await load();
  await wait(`document.querySelector('${railTeams}[aria-label="Teams, 7 unread"]')`);
  assert.equal(await run(`document.querySelector('${railTeams} .desktop-rail-count').textContent`), '7');
  assert.equal(await run(`document.querySelector('${railTeams} .desktop-rail-count').getAttribute('aria-hidden')`), 'true', 'the digits are not read out as well');
  assert.equal(await run(`document.querySelector('${railTeams}').title`), 'Teams, 7 unread');
  assert.deepEqual(await run(`Array.from(document.querySelectorAll('.desktop-nav-rail button')).map(button => button.getAttribute('aria-label'))`),
    ['Home', 'Bots', 'Teams, 7 unread', 'Apps', 'Scheduled tasks', 'Review changes', 'Settings']);
  result.railBadge = true;

  // Teams mode: the workspace's own sidebar, the Teams page beside it, and the mode saved.
  await click(railTeams);
  await wait(`document.querySelectorAll('${sidebar} [data-row]').length === 4`);
  await wait(`document.querySelector('.teams-page .tw-conv-empty')`);
  assert.deepEqual(await marked(), ['Teams']);
  assert.equal(await run(`document.querySelector('${sidebar} .tw-switcher-name').textContent`), 'Acme');
  assert.deepEqual(await run(`Array.from(document.querySelectorAll('${sidebar} h3')).map(heading => heading.textContent)`), ['Channels', 'Direct messages', 'Bot DMs']);
  assert.deepEqual(await rows(), ['chan-general', 'chan-design', 'dm-bob', 'bot-chat']);
  assert.deepEqual(await run(`Array.from(document.querySelectorAll('${sidebar} .tw-row-preview')).map(preview => preview.textContent)`),
    ['Bob: Ship it', 'Bob: Mockups are in the folder', 'You: Lunch?', 'Done: the summary is ready']);
  assert.equal(await run(`Boolean(document.querySelector('${row('chan-general')}').closest('.tw-row').querySelector('[role="img"][aria-label="4 unread"]'))`), true, 'the unread count of the row');
  assert.equal(await run(`Boolean(document.querySelector('${row('bot-chat')} .tw-avatar-custom img'))`), true, 'a Bot wears its avatar');
  assert.match(await run(`document.querySelector('.teams-page').textContent`), /Choose a conversation/);
  assert.equal(await run(`Boolean(document.querySelector('.sidebar-new-chat'))`), false, 'the projects are Home\'s');
  assert.deepEqual((await writes()).at(-1), ['updateSettings', { sidebar: { mode: 'teams' } }]);
  result.teamsMode = true;

  // The sidebar's search box narrows the conversations.
  await click('.sidebar-header-icon[aria-label="Search chats"]');
  await wait(`document.querySelector('.sidebar-search input')`);
  await setValue('.sidebar-search input', 'lunch');
  await wait(`document.querySelectorAll('${sidebar} [data-row]').length === 1`);
  assert.deepEqual(await rows(), ['dm-bob']);
  await setValue('.sidebar-search input', 'nothing says this');
  await wait(`document.querySelector('${sidebar} .tw-empty-line')?.textContent === 'No matching conversations.'`);
  await setValue('.sidebar-search input', '');
  await wait(`document.querySelectorAll('${sidebar} [data-row]').length === 4`);
  await click('.sidebar-search button[aria-label="Close search"]');
  result.search = true;

  // A conversation opens in the page: the stream with its day and unread dividers and its grouped runs.
  await click(row('chan-general'));
  await wait(`document.querySelectorAll('.teams-conversation .tw-msg').length === 7`);
  assert.equal(await run(`document.querySelector('${row('chan-general')}').getAttribute('aria-current')`), 'page');
  assert.equal(await run(`document.querySelector('.teams-conversation .tw-conv-head h2').textContent`), '#general');
  assert.equal(await run(`document.querySelectorAll('.teams-conversation [data-unread-divider]').length`), 1);
  assert.equal(await run(`document.querySelectorAll('.teams-conversation .tw-msg[data-grouped="true"]').length`), 2);
  assert.equal(await shown('.beta-rich-prompt'), false, 'the chat is not on screen');
  result.stream = true;

  // Sending calls the service with what was typed, and the message joins the stream.
  await setValue('.teams-conversation textarea[aria-label="Message"]', 'Hello team');
  await click('.teams-conversation .tw-send');
  await wait(`window.fixtureCalls.some(call => call[0] === 'messages' && call[1].operation === 'create' && call[1].text === 'Hello team' && call[1].conversationId === 'chan-general')`);
  await wait(`document.querySelector('.teams-conversation [role="log"]').textContent.includes('Hello team')`);
  assert.equal(await run(`document.querySelector('.teams-conversation textarea[aria-label="Message"]').value`), '', 'and the box is empty again');
  result.sends = true;

  // The details panel: the header button, the width the keyboard sets and the window keeps, the View menu, and the sidebar's team menu.
  assert.equal(await shown('.tw-drawer'), false);
  await click('.teams-conversation button[aria-label="Team details"]');
  await wait(`document.querySelector('.teams-page > .tw-drawer')`);
  assert.equal(await run(`document.querySelector('.teams-conversation button[aria-label="Team details"]').getAttribute('aria-pressed')`), 'true');
  assert.equal(await run(`document.querySelector('.tw-drawer h2').textContent`), 'Acme');
  assert.equal(await drawerWidth(), 360, 'the panel\'s own default, when none was chosen');
  await run(`(() => { const handle = document.querySelector('.tw-resize'); handle.focus(); handle.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowLeft', bubbles: true, cancelable: true })); })()`);
  await wait(`document.querySelector('.tw-drawer').getBoundingClientRect().width === 376`);
  await wait(`window.fixtureCalls.some(call => call[0] === 'updateSettings' && call[1].sidebar?.teamsDrawerWidth === 376)`);
  assert.equal((await writes()).filter(call => call[1].sidebar?.teamsDrawerWidth !== undefined).length, 1, 'one write for the width, not one for every step');
  result.drawer = true;

  await menuAction('team-details');
  await wait(`!document.querySelector('.tw-drawer')`);
  await menuAction('team-details');
  await wait(`document.querySelector('.tw-drawer')`);
  assert.equal(await drawerWidth(), 376, 'and it comes back as wide as it was');
  // In a narrow window the panel lies over the conversation, which keeps the whole page; in a wide one it stands beside it.
  const beside = () => run(`(() => { const page = document.querySelector('.teams-page').getBoundingClientRect(), conversation = document.querySelector('.teams-conversation').getBoundingClientRect(), panel = document.querySelector('.tw-drawer').getBoundingClientRect(); return { sameWidth: Math.round(conversation.width) === Math.round(page.width), flush: Math.round(panel.right) === Math.round(page.right), position: getComputedStyle(document.querySelector('.tw-drawer')).position }; })()`);
  assert.deepEqual(await beside(), { sameWidth: false, flush: true, position: 'relative' });
  win.setSize(900, 700);
  await wait(`getComputedStyle(document.querySelector('.tw-drawer')).position === 'absolute'`);
  assert.deepEqual(await beside(), { sameWidth: true, flush: true, position: 'absolute' });
  assert.equal(await drawerWidth(), 376, 'as wide as it was, over the conversation');
  win.setSize(1280, 850);
  await wait(`getComputedStyle(document.querySelector('.tw-drawer')).position === 'relative'`);
  assert.deepEqual(await beside(), { sameWidth: false, flush: true, position: 'relative' });
  await click('.tw-drawer button[aria-label="Close details"]');
  await wait(`!document.querySelector('.tw-drawer')`);
  await click(`${sidebar} [aria-label="Team members & settings"]`);
  await wait(`document.querySelector('.tw-drawer')`);
  await menuAction('team-details');
  await wait(`!document.querySelector('.tw-drawer')`);
  // Off the Teams page the menu item does nothing: it must not have opened the panel behind the chat.
  await click(rail('Home'));
  await wait(`!document.querySelector('${sidebar}') && document.querySelector('.sidebar-new-chat')`);
  await menuAction('team-details');
  await delay(150);
  await click(railTeams);
  await wait(`document.querySelector('.teams-conversation .tw-msg')`);
  assert.equal(await shown('.tw-drawer'), false, 'the item was ignored on Home');
  result.drawerMenu = true;

  // The sidebar lists Teams while a page is open, and pressing a conversation brings the Teams page back.
  await click(rail('Apps'));
  await wait(`document.querySelector('${rail('Apps')}[data-active]')`);
  assert.deepEqual(await marked(), ['Teams', 'Apps']);
  assert.equal(await shown(sidebar), true);
  assert.equal(await shown('.teams-page'), false);
  await click(row('dm-bob'));
  await wait(`document.querySelector('${row('dm-bob')}[aria-current="page"]') && document.querySelector('.teams-page')?.getClientRects().length > 0`);
  assert.deepEqual(await marked(), ['Teams']);
  result.pageForward = true;

  // Reading a conversation takes its messages off the rail's count.
  await click(row('chan-general'));
  await wait(`document.querySelector('${row('chan-general')}[aria-current="page"]')`);
  const read = await run(`Array.from(document.querySelectorAll('.teams-conversation button')).find(button => /^Mark as read$/.test(button.textContent.trim()))?.disabled === false`);
  if (read) await run(`Array.from(document.querySelectorAll('.teams-conversation button')).find(button => /^Mark as read$/.test(button.textContent.trim())).click()`);
  await wait(`document.querySelector('${railTeams}').getAttribute('aria-label') === 'Teams, 3 unread'`);
  assert.equal(await run(`document.querySelector('${railTeams} .desktop-rail-count').textContent`), '3');
  result.badgeFollowsReading = true;

  // The View menu's Teams (the key ⌘3) takes the keyboard to the open conversation in the sidebar, and leaves it where it is when it is in Teams already.
  await click(row('dm-bob'));
  await wait(`document.querySelector('${row('dm-bob')}[aria-current="page"]')`);
  await menuAction('home');
  await wait(`!document.querySelector('${sidebar}') && document.querySelector('.sidebar-new-chat')`);
  await menuAction('teams');
  await wait(`document.activeElement === document.querySelector('${row('dm-bob')}')`);
  await run(`document.querySelector('.teams-conversation textarea[aria-label="Message"]').focus()`);
  await menuAction('teams');
  await delay(300);
  assert.equal(await run(`document.activeElement === document.querySelector('.teams-conversation textarea[aria-label="Message"]')`), true, 'the composer keeps the keyboard');
  result.focus = true;

  // A team invitation from a link opens in Teams mode even when the sidebar is hidden, reviewed in the join dialog and not joined.
  await menuAction('home');
  await wait(`!document.querySelector('${sidebar}') && document.querySelector('.sidebar-new-chat')`);
  await menuAction('toggle-sidebar');
  await wait(`document.querySelector('.desktop-navigation')?.getAttribute('data-collapsed') === 'true'`);
  const code = 'AbCdEfGhIjKlMnOpQrStUvWxYz012345';
  await run(`window.deliverLink('team/join', { code: ${JSON.stringify(code)} })`);
  await wait(`document.querySelector('dialog.tw-dialog[open]')`);
  assert.equal(await run(`document.querySelector('.desktop-navigation').hasAttribute('data-collapsed')`), false, 'the sidebar came back for it');
  assert.deepEqual(await marked(), ['Teams']);
  assert.equal(await run(`document.querySelector('dialog.tw-dialog[open] .tw-dialog-title').textContent`), 'Join a team');
  assert.equal(await run(`document.querySelector('dialog.tw-dialog[open] input').value`), code);
  await wait(`window.fixtureCalls.some(call => call[0] === 'acknowledge' && call[2] === null)`);
  assert.deepEqual((await calls()).filter(call => call[0] === 'execute' && call[1].action === 'join'), [], 'a link never joins by itself');
  await run(`Array.from(document.querySelectorAll('dialog.tw-dialog[open] button')).find(button => button.textContent.trim() === 'Cancel').click()`);
  await wait(`!document.querySelector('dialog.tw-dialog[open]')`);
  result.invite = true;

  // A window that was last in Teams mode opens in it, with the panel as wide as the file says.
  await load('?saved=teams&drawerWidth=420');
  await wait(`document.querySelectorAll('${sidebar} [data-row]').length === 4 && document.querySelector('.teams-page')`);
  assert.deepEqual(await marked(), ['Teams']);
  await delay(200);
  assert.deepEqual(await writes(), [], 'bringing the mode back is not a change to save');
  await click(row('chan-general'));
  await wait(`document.querySelector('.teams-conversation .tw-msg')`);
  await click('.teams-conversation button[aria-label="Team details"]');
  await wait(`document.querySelector('.tw-drawer')`);
  assert.equal(await drawerWidth(), 420);
  await delay(600);
  assert.deepEqual(await writes(), [], 'and showing the saved width is not a change either');
  result.restored = true;

  // Without an account Teams says what to do, in the sidebar and on the page, and the rail has no count.
  await load('?signedOut=1&saved=teams');
  const page = 'main.desktop-main section[aria-labelledby]';
  await wait(`document.querySelector('${sidebar} h2') && document.querySelector('${page} h2')`);
  assert.equal(await run(`document.querySelector('${sidebar}').textContent.includes('Sign in to your LingXi account to use Teams.')`), true);
  assert.equal(await run(`document.querySelector('${page} h2').textContent`), 'Sign in to use Teams');
  assert.equal(await run(`document.querySelector('${page}').textContent.includes('Sign in to your LingXi account to use Teams.')`), true);
  assert.equal(await run(`Array.from(document.querySelectorAll('${sidebar} button, ${page} button')).map(button => button.textContent.trim()).join()`), 'Sign In,Sign In');
  // Each names itself: two headings never share an id.
  assert.equal(await run(`Array.from(document.querySelectorAll('${sidebar} section, ${page}')).every(section => document.getElementById(section.getAttribute('aria-labelledby')) === section.querySelector('h2'))`), true);
  assert.equal(await run(`document.querySelector('${railTeams}').getAttribute('aria-label')`), 'Teams');
  assert.equal(await run(`Boolean(document.querySelector('${railTeams} .desktop-rail-count'))`), false);
  result.signedOut = true;

  // Past 99 the rail says 99+ and the label still has the number.
  await load('?many=1');
  await wait(`document.querySelector('${railTeams}[aria-label="Teams, 253 unread"]')`);
  assert.equal(await run(`document.querySelector('${railTeams} .desktop-rail-count').textContent`), '99+');
  result.cappedBadge = true;

  if (screenshots) {
    mkdirSync(screenshots, { recursive: true });
    const snap = async name => { writeFileSync(join(screenshots, name), (await win.webContents.capturePage()).toPNG()); };
    for (const theme of ['light', 'dark']) {
      await load(`?saved=teams&theme=${theme}`);
      await wait(`document.querySelectorAll('${sidebar} [data-row]').length === 4`);
      await click(row('chan-general'));
      await wait(`document.querySelectorAll('.teams-conversation .tw-msg').length === 7`);
      await delay(500);
      await snap(`teams-${theme}.png`);
      await click('.teams-conversation button[aria-label="Team details"]');
      await wait(`document.querySelector('.tw-drawer')`);
      await delay(500);
      await snap(`teams-drawer-${theme}.png`);
      await load(`?saved=bots&theme=${theme}`);
      await wait(`document.querySelector('.bots-page')?.getClientRects().length > 0 && document.querySelectorAll('nav[aria-label="Bots"] button[aria-expanded]:not([aria-haspopup])').length === 2`);
      await delay(500);
      await snap(`bots-${theme}.png`);
    }
    // Beyond those, for a look at the places a layout gives way first: the panel's Bots tab, the narrowest window with the panel open, and Teams without an account.
    await load('?saved=teams&theme=light');
    await wait(`document.querySelectorAll('${sidebar} [data-row]').length === 4`);
    await click(row('chan-general'));
    await wait(`document.querySelectorAll('.teams-conversation .tw-msg').length === 7`);
    await click('.teams-conversation button[aria-label="Team details"]');
    await wait(`document.querySelector('.tw-drawer')`);
    await run(`Array.from(document.querySelectorAll('.tw-drawer .tw-tab')).find(tab => tab.textContent.trim() === 'Bots').click()`);
    await delay(500);
    await snap('teams-drawer-bots-light.png');
    win.setSize(900, 700);
    await delay(500);
    await snap('teams-narrow-light.png');
    win.setSize(1280, 850);
    await load('?signedOut=1&saved=teams&theme=light');
    await wait(`document.querySelector('main.desktop-main section h2')`);
    await delay(300);
    await snap('teams-signed-out-light.png');
  }

  assert.deepEqual(errors, []);
  console.log(JSON.stringify(result));
  win.destroy(); app.exit(0);
} catch (error) { console.error(error, '\nconsole errors:', errors); win.destroy(); app.exit(1); }
}
void main().catch(error => { console.error(error); app.exit(1); });
