import assert from 'node:assert/strict';
import { app, BrowserWindow } from 'electron';

const url = process.argv.find(arg => arg.startsWith('http://'));
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
async function main() {
await app.whenReady();
const win = new BrowserWindow({ show: false, width: 1280, height: 850, webPreferences: { sandbox: true, backgroundThrottling: false } });
const errors = [];
win.webContents.on('console-message', details => { if (details.level === 'error' || details.level === 3) errors.push(details.message); });
const run = script => win.webContents.executeJavaScript(script);
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function wait(expression) {
  for (let i = 0; i < 250; i++) { if (await run(expression)) return; await delay(20); }
  throw new Error(`Timed out: ${expression}\n${await run('document.body.innerText')}`);
}
const click = selector => run(`document.querySelector(${JSON.stringify(selector)}).click()`);
const rail = name => `.desktop-nav-rail [aria-label="${name}"]`;
const sidebar = 'nav[aria-label="Bots"]';
const botRow = name => `${sidebar} button[title="${name}"]`;
const marked = async () => run(`Array.from(document.querySelectorAll('.desktop-nav-rail button[data-active]')).map(button => button.getAttribute('aria-label'))`);
const shown = selector => run(`document.querySelector(${JSON.stringify(selector)})?.getClientRects().length > 0`);
const calls = () => run('window.fixtureCalls');
const menuAction = action => run(`window.applicationMenuAction(${JSON.stringify(action)})`);
const key = name => run(`document.dispatchEvent(new KeyboardEvent('keydown', { key: ${JSON.stringify(name)}, bubbles: true, cancelable: true }))`);
const rightClick = selector => run(`document.querySelector(${JSON.stringify(selector)}).dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 120, clientY: 160 }))`);
const menuItems = () => run(`Array.from(document.querySelectorAll('[role="menu"] [role="menuitem"]')).map(item => item.textContent.trim())`);
const result = {};
try {
  // The rail has the modes, a divider, then the pages; Home is the mode the window opens in.
  await win.loadURL(url);
  await wait(`document.querySelector('${rail('Bots')}')`);
  assert.deepEqual(await run(`Array.from(document.querySelectorAll('.desktop-nav-rail button')).map(button => button.getAttribute('aria-label'))`),
    ['Home', 'Bots', 'Teams', 'Apps', 'Scheduled tasks', 'Review changes', 'Settings']);
  assert.equal(await run(`document.querySelector('.desktop-rail-divider').previousElementSibling.getAttribute('aria-label')`), 'Apps');
  assert.deepEqual(await marked(), ['Home']);
  assert.equal(await run(`Boolean(document.querySelector('.sidebar-new-chat'))`), true, 'Home lists the projects and has its own New chat');
  assert.equal(await run(`Array.from(document.querySelectorAll('.desktop-sidebar .sidebar-primary-action')).some(button => /^(Teams|Bots)$/.test(button.textContent.trim()))`), false, 'and does not repeat the rail');
  result.railGroups = true;

  // Bots mode: the Bots page in the main area, the Bots most recently used first in the sidebar, and the mode saved.
  await click(rail('Bots'));
  await wait(`document.querySelector('.bots-page')?.getClientRects().length > 0`);
  await wait(`document.querySelectorAll('${sidebar} button[aria-expanded]:not([aria-haspopup])').length === 2`);
  assert.deepEqual(await marked(), ['Bots']);
  assert.deepEqual(await run(`Array.from(document.querySelectorAll('${sidebar} button[aria-expanded]:not([aria-haspopup])')).map(button => button.title)`), ['Beta', 'Alpha']);
  assert.equal(await run(`Boolean(document.querySelector('.sidebar-new-chat'))`), false, 'the projects and their actions are Home\'s');
  assert.deepEqual((await calls()).filter(call => call[0] === 'updateSettings').at(-1), ['updateSettings', { sidebar: { mode: 'bots' } }]);
  result.botsMode = true;

  // Opening a Bot lists its chats newest first, five to begin with; which Bots are open is saved.
  await click(botRow('Alpha'));
  await wait(`document.querySelectorAll('${sidebar} [data-session-id]').length === 5`);
  assert.deepEqual(await run(`Array.from(document.querySelectorAll('${sidebar} [data-session-id]')).map(row => row.title)`),
    ['Plan the launch', 'Weekly notes', 'Draft the post', 'Fix the intro', 'Release checklist']);
  assert.match(await run(`Array.from(document.querySelectorAll('${sidebar} button')).find(button => /Show more/.test(button.textContent))?.textContent ?? ''`), /Show more \(2\)/);
  assert.deepEqual((await calls()).filter(call => call[0] === 'updateSettings').at(-1), ['updateSettings', { sidebar: { expandedBots: ['alpha'] } }]);
  await run(`Array.from(document.querySelectorAll('${sidebar} button')).find(button => /Show more/.test(button.textContent)).click()`);
  await wait(`document.querySelectorAll('${sidebar} [data-session-id]').length === 7`);
  await run(`Array.from(document.querySelectorAll('${sidebar} button')).find(button => /Show less/.test(button.textContent)).click()`);
  await wait(`document.querySelectorAll('${sidebar} [data-session-id]').length === 5`);
  result.chatsListed = true;

  // A chat opens in the chat view, the sidebar stays in Bots mode, and the chat is the marked row.
  await click(`${sidebar} [data-session-id="a-2"]`);
  await wait(`document.querySelector('${sidebar} [data-session-id="a-2"][aria-current="page"]')`);
  assert.equal(await shown('.bots-page'), false);
  assert.deepEqual(await marked(), ['Bots']);
  assert.deepEqual((await calls()).filter(call => call[0] === 'openSession'), [['openSession', 'alpha', 'a-2', '/fixture/transcripts/a-2.jsonl']]);
  assert.equal(await run(`Boolean(document.querySelector('.beta-rich-prompt'))`), true);
  result.chatOpens = true;

  // Each workspace has its own sidebar; Cron replaces it with the task list.
  await click(rail('Apps'));
  await wait(`document.querySelector('${rail('Apps')}[data-active]')`);
  assert.deepEqual(await marked(), ['Apps']);
  assert.equal(await shown(sidebar), false);
  assert.equal(await shown('.local-apps-sidebar'), true);
  await click(rail('Scheduled tasks'));
  await wait(`document.querySelector('.desktop-sidebar .scheduled-master')`);
  assert.deepEqual(await marked(), ['Scheduled tasks']);
  assert.equal(await shown(sidebar), false);
  assert.equal(await run(`Boolean(document.querySelector('.desktop-main .scheduled-master'))`), false);
  assert.equal(await run(`Boolean(document.querySelector('[data-conversation-space], .desktop-sidebar-brand .sidebar-header-icon'))`), false);
  result.pageKeepsMode = true;

  // The View menu: Home, Bots (which stays on a Bot's chat), the Bot's own page from the chat's title, and the sidebar's visibility.
  await menuAction('home');
  await wait(`!document.querySelector('${sidebar}') && document.querySelector('.sidebar-new-chat')`);
  assert.deepEqual(await marked(), ['Home']);
  await menuAction('bots');
  await wait(`document.querySelector('${sidebar}')`);
  assert.deepEqual(await marked(), ['Bots']);
  assert.equal(await shown('.bots-page'), false, 'a Bot\'s chat is on screen and stays there');
  assert.equal(await run(`document.querySelector('${sidebar} [data-session-id="a-2"][aria-current="page"]') !== null`), true);
  await menuAction('toggle-sidebar');
  await wait(`document.querySelector('.desktop-navigation')?.getAttribute('data-collapsed') === 'true'`);
  await menuAction('toggle-sidebar');
  await wait(`!document.querySelector('.desktop-navigation')?.hasAttribute('data-collapsed')`);
  result.viewMenu = true;

  await click('[aria-label="Open Bot"]');
  await wait(`document.querySelector('.bots-page h1')?.textContent === 'Alpha'`);
  assert.deepEqual(await marked(), ['Bots']);
  assert.deepEqual(await run(`Array.from(document.querySelectorAll('.bots-tabs button')).map(button => button.textContent)`),
    ['Routines', 'Tasks', 'General', 'Persona', 'Memory', 'Skills', 'Folders', 'Permissions'], 'no Chats tab: the chats are in the sidebar');
  assert.equal(await run(`document.querySelector('.bots-tabs button[aria-current="page"]').textContent`), 'General');
  result.botPageFromTitle = true;

  // A right click on a Bot: its menu, the arrow keys, Escape and where the focus goes back to.
  await rightClick(botRow('Alpha'));
  await wait(`document.querySelector('[role="menu"]')`);
  assert.deepEqual(await menuItems(), ['New chat', 'Bot settings', 'Delete Bot…']);
  await wait(`document.activeElement?.getAttribute('role') === 'menuitem'`);
  assert.equal(await run(`document.activeElement.textContent.trim()`), 'New chat');
  await key('ArrowDown');
  assert.equal(await run(`document.activeElement.textContent.trim()`), 'Bot settings');
  await key('End');
  assert.equal(await run(`document.activeElement.textContent.trim()`), 'Delete Bot…');
  await key('Escape');
  await wait(`!document.querySelector('[role="menu"]')`);
  await wait(`document.activeElement === document.querySelector('${botRow('Alpha')}')`);
  // The same menu from the button on the row, which closes it again when pressed again.
  await click(`${sidebar} [aria-label="Actions for Alpha"]`);
  await wait(`document.querySelector('[role="menu"]')`);
  assert.deepEqual(await menuItems(), ['New chat', 'Bot settings', 'Delete Bot…']);
  await click(`${sidebar} [aria-label="Actions for Alpha"]`);
  await wait(`!document.querySelector('[role="menu"]')`);
  // A chat's menu offers to open it.
  await rightClick(`${sidebar} [data-session-id="a-3"]`);
  await wait(`document.querySelector('[role="menu"]')`);
  assert.deepEqual(await menuItems(), ['Open']);
  await run(`document.querySelector('[role="menuitem"]').click()`);
  await wait(`document.querySelector('${sidebar} [data-session-id="a-3"][aria-current="page"]')`);
  assert.deepEqual((await calls()).filter(call => call[0] === 'openSession').at(-1), ['openSession', 'alpha', 'a-3', '/fixture/transcripts/a-3.jsonl'], 'menu keeps the selected transcript');
  await click('[aria-label="Open Bot"]');
  await wait(`document.querySelector('.bots-page h1')?.textContent === 'Alpha'`);
  result.menus = true;

  // Deleting from the menu asks first, and nothing is deleted until the question is answered yes.
  await rightClick(botRow('Alpha'));
  await wait(`document.querySelector('[role="menu"]')`);
  await run(`Array.from(document.querySelectorAll('[role="menuitem"]')).find(item => /Delete Bot/.test(item.textContent)).click()`);
  await wait(`document.querySelector('[role="dialog"]')`);
  assert.equal(await shown('.bots-page'), true);
  assert.match(await run(`document.querySelector('[role="dialog"]').getAttribute('aria-label')`), /Alpha/);
  assert.deepEqual((await calls()).filter(call => call[0] === 'delete'), []);
  await click('[role="dialog"] .desktop-dialog-action--cancel');
  await wait(`!document.querySelector('[role="dialog"]')`);
  assert.deepEqual((await calls()).filter(call => call[0] === 'delete'), []);
  await rightClick(botRow('Alpha'));
  await wait(`document.querySelector('[role="menu"]')`);
  await run(`Array.from(document.querySelectorAll('[role="menuitem"]')).find(item => /Delete Bot/.test(item.textContent)).click()`);
  await wait(`document.querySelector('[role="dialog"]')`);
  assert.deepEqual((await calls()).filter(call => call[0] === 'delete'), [], 'asked again, not answered');
  await click('[role="dialog"] .desktop-dialog-action--primary');
  await wait(`!document.querySelector('${botRow('Alpha')}')`);
  assert.deepEqual((await calls()).filter(call => call[0] === 'delete'), [['delete', 'alpha']]);
  assert.deepEqual(await run(`Array.from(document.querySelectorAll('${sidebar} button[aria-expanded]:not([aria-haspopup])')).map(button => button.title)`), ['Beta']);
  result.deleteAsksFirst = true;

  // Home brings back the projects, and the mode that was chosen last is what is saved.
  await click(rail('Home'));
  await wait(`!document.querySelector('${sidebar}') && document.querySelector('.sidebar-new-chat')`);
  assert.deepEqual(await marked(), ['Home']);
  assert.deepEqual((await calls()).filter(call => call[0] === 'updateSettings').at(-1), ['updateSettings', { sidebar: { mode: 'home' } }]);
  result.homeAgain = true;

  // A window that was last in Bots mode opens in it.
  const writesBefore = 0;
  await win.loadURL(`${url}?saved=bots`);
  await wait(`document.querySelector('.bots-page')?.getClientRects().length > 0 && document.querySelector('${sidebar} h2')`);
  assert.deepEqual(await marked(), ['Bots']);
  await delay(150);
  assert.equal((await calls()).filter(call => call[0] === 'updateSettings').length, writesBefore, 'bringing the mode back is not a change to save');
  result.restored = true;

  // Edits that are not saved are asked about before anything in the window takes the owner away from the Bot's page: its chats and the
  // other Bots in the sidebar, the New Bot button, the rail and the View menu. No leaves the page and the edit as they were.
  const dialog = '[role="dialog"]';
  const nameField = '.bots-page .bots-fields input';
  const edit = value => run(`(() => {
    const field = document.querySelector(${JSON.stringify(nameField)});
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(field, ${JSON.stringify(value)});
    field.dispatchEvent(new Event('input', { bubbles: true }));
  })()`);
  const nameIs = value => `document.querySelector(${JSON.stringify(nameField)})?.value === ${JSON.stringify(value)}`;
  const unsaved = () => run(`/Unsaved changes/.test(document.querySelector('.bots-save-status')?.textContent ?? '')`);
  const discardAsked = `document.querySelector('${dialog}')?.getAttribute('aria-label') === 'Discard your changes?'`;
  const answer = async button => { await click(`${dialog} .desktop-dialog-action--${button}`); await wait(`!document.querySelector('${dialog}')`); };
  const botMenuItem = async (name, label) => {
    await rightClick(botRow(name));
    await wait(`document.querySelector('[role="menu"]')`);
    await run(`Array.from(document.querySelectorAll('[role="menuitem"]')).find(item => item.textContent.trim() === ${JSON.stringify(label)}).click()`);
  };
  const openBotPage = async name => {
    await botMenuItem(name, 'Bot settings');
    await wait(`document.querySelector('.bots-page h1')?.textContent === ${JSON.stringify(name)} && ${nameIs(name)}`);
  };
  const opened = async () => (await calls()).filter(call => call[0] === 'openSession');
  const askedAndKept = async (why, leave) => {
    await leave();
    await wait(discardAsked);
    assert.deepEqual(await opened(), [], `${why}: nothing is opened while the question is open`);
    await answer('cancel');
    assert.equal(await shown('.bots-page'), true, `${why}: still on the Bot's page`);
    assert.equal(await run(`document.querySelector('.bots-page h1').textContent`), 'Alpha', why);
    assert.equal(await run(nameIs('Alpha edited')), true, `${why}: the edit is kept`);
    assert.equal(await unsaved(), true, why);
    assert.deepEqual(await marked(), ['Bots'], `${why}: the mode is as it was`);
    assert.deepEqual(await opened(), [], why);
  };

  await click(botRow('Alpha'));
  await wait(`document.querySelectorAll('${sidebar} [data-session-id]').length === 5`);
  await openBotPage('Alpha');
  assert.equal(await unsaved(), false);
  await edit('Alpha edited');
  await wait(nameIs('Alpha edited'));
  assert.equal(await unsaved(), true);
  await askedAndKept('a chat of the Bot', () => click(`${sidebar} [data-session-id="a-2"]`));
  await askedAndKept('the Bot\'s new chat button', () => click(`${sidebar} [aria-label="New chat with Alpha"]`));
  await askedAndKept('the Bot\'s new chat menu item', () => botMenuItem('Alpha', 'New chat'));
  await askedAndKept('another Bot', () => botMenuItem('Beta', 'Bot settings'));
  await askedAndKept('the New Bot button', () => click(`${sidebar} [aria-label="New Bot"]`));
  await askedAndKept('the rail\'s Home', () => click(rail('Home')));
  await askedAndKept('the rail\'s Teams', () => click(rail('Teams')));
  await askedAndKept('the View menu\'s Home', () => menuAction('home'));
  await askedAndKept('the View menu\'s Teams', () => menuAction('teams'));
  // The page the owner is on is not left by asking for it again.
  await botMenuItem('Alpha', 'Bot settings');
  await delay(150);
  assert.equal(await run(`Boolean(document.querySelector('${dialog}'))`), false, 'no question for the Bot that is already open');
  assert.equal(await run(nameIs('Alpha edited')), true);

  // Giving the edit up goes where the owner was going, once, and the Bot's page no longer has the edit.
  await click(`${sidebar} [data-session-id="a-2"]`);
  await wait(discardAsked);
  assert.deepEqual(await opened(), []);
  await answer('primary');
  await wait(`document.querySelector('${sidebar} [data-session-id="a-2"][aria-current="page"]')`);
  assert.equal(await shown('.bots-page'), false);
  assert.deepEqual(await opened(), [['openSession', 'alpha', 'a-2', '/fixture/transcripts/a-2.jsonl']]);
  await click('[aria-label="Open Bot"]');
  await wait(`document.querySelector('.bots-page h1')?.textContent === 'Alpha' && ${nameIs('Alpha')}`);
  assert.equal(await unsaved(), false, 'the edit was given up');

  // The same for another Bot, and for the rail: Beta's page, edited, is left for Home.
  await edit('Alpha edited again');
  await wait(nameIs('Alpha edited again'));
  await botMenuItem('Beta', 'Bot settings');
  await wait(discardAsked);
  assert.equal(await run(`document.querySelector('.bots-page h1').textContent`), 'Alpha', 'still on the first Bot while the question is open');
  await answer('primary');
  await wait(`document.querySelector('.bots-page h1')?.textContent === 'Beta' && ${nameIs('Beta')}`);
  assert.equal(await unsaved(), false);
  await edit('Beta edited');
  await wait(nameIs('Beta edited'));
  await click(rail('Home'));
  await wait(discardAsked);
  assert.deepEqual(await marked(), ['Bots']);
  await answer('primary');
  await wait(`!document.querySelector('${sidebar}') && document.querySelector('.sidebar-new-chat')`);
  assert.deepEqual(await marked(), ['Home']);
  assert.equal(await run(`Boolean(document.querySelector('${dialog}'))`), false);
  result.dirtyGuard = true;

  // Where Bots cannot be used the saved mode is not brought back, and the sidebar does not mention them.
  await win.loadURL(`${url}?saved=bots&bots=off`);
  await wait(`document.querySelector('${rail('Home')}[data-active]') && document.querySelector('.sidebar-new-chat')`);
  await delay(150);
  assert.deepEqual(await run(`Array.from(document.querySelectorAll('.desktop-nav-rail button')).map(button => button.getAttribute('aria-label'))`),
    ['Home', 'Teams', 'Apps', 'Scheduled tasks', 'Review changes', 'Settings']);
  assert.equal(await shown('.bots-page'), false);
  assert.equal((await calls()).filter(call => call[0] === 'updateSettings').length, 0);
  await menuAction('bots');
  await delay(100);
  assert.deepEqual(await marked(), ['Home'], 'the menu\'s Bots does nothing where they cannot run');
  result.unavailable = true;

  assert.deepEqual(errors, []);
  console.log(JSON.stringify(result));
  win.destroy(); app.exit(0);
} catch (error) { console.error(error, '\nconsole errors:', errors); win.destroy(); app.exit(1); }
}
void main().catch(error => { console.error(error); app.exit(1); });
