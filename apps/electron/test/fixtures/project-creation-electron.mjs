import { app, BrowserWindow } from 'electron';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const url = process.argv.find(value => value.startsWith('http://'));
const evidence = process.env.LINGXI_PROJECT_CREATE_EVIDENCE;
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function main() {
  await app.whenReady();
  app.dock?.hide();
  const window = new BrowserWindow({ show: false, width: 900, height: 700, useContentSize: true, webPreferences: { sandbox: true, backgroundThrottling: false } });
  try {
    const run = script => window.webContents.executeJavaScript(script);
    const wait = async expression => {
      for (let i = 0; i < 250; i++) { if (await run(expression)) return; await delay(20); }
      throw new Error(`Timed out: ${expression}\n${await run('document.body.innerText')}`);
    };
    const check = async (expression, label) => { if (!await run(expression)) throw new Error(label); };
    const open = async () => {
      await run(`document.querySelector('[aria-label="Add project"]').focus(); document.querySelector('[aria-label="Add project"]').click()`);
      await wait(`!!document.querySelector('[aria-label="Create project"]')`);
    };
    const screenshot = async name => {
      if (!evidence) return;
      await run(`Promise.all(document.querySelector('.desktop-dialog-overlay').getAnimations({subtree:true}).map(animation => animation.finished.catch(() => {})))`);
      await run(`new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))`);
      mkdirSync(evidence, { recursive: true });
      writeFileSync(join(evidence, name + '.png'), (await window.webContents.capturePage(undefined, { stayHidden: true, stayAwake: true })).toPNG());
      if (name === 'create-project-light') {
        const rectangle = await run(`(() => {
          const r = document.querySelector('.create-project-dialog').getBoundingClientRect();
          const x = Math.max(0, Math.floor(r.left - 16)), y = Math.max(0, Math.floor(r.top - 16));
          return { x, y, width: Math.min(innerWidth - x, Math.ceil(r.right + 16) - x), height: Math.min(innerHeight - y, Math.ceil(r.bottom + 16) - y) };
        })()`);
        writeFileSync(join(evidence, 'create-project-preview.png'), (await window.webContents.capturePage(rectangle, { stayHidden: true, stayAwake: true })).toPNG());
      }
    };
    await window.loadURL(url);
    await wait(`!!document.querySelector('[aria-label="Add project"]') && !document.querySelector('[aria-label="Add project"]').closest('[inert]')`);
    await open();
    await check(`document.activeElement?.getAttribute('aria-label') === 'Project name' && document.activeElement.readOnly`, 'Folder-derived name must receive initial focus');
    await check(`document.querySelector('.create-project-submit').disabled && window.projectCreationFixture.picks === 0`, 'Opening dialog must not select or create a folder');
    await check(`!!document.querySelector('[aria-label="Add project"]').closest('[inert]')`, 'The background must become inert while the project dialog is open');
    await screenshot('create-project-light');
    await run(`document.querySelector('.create-project-add-folder').click()`);
    await wait(`!document.querySelector('form[aria-busy=true]')`);
    await check(`document.querySelector('.create-project-submit').disabled`, 'Cancelled native picker must keep empty draft');
    await run(`window.projectCreationFixture.nextFolder = {id:'selected-1',path:'/Users/tester/Projects/Hanzi Play'}; document.querySelector('.create-project-add-folder').click()`);
    await wait(`document.querySelector('[aria-label="Project name"]').value === 'Hanzi Play'`);
    await check(`window.projectCreationFixture.creates.length === 0 && window.projectCreationFixture.projects.length === 1`, 'Selecting a folder must not add a project');
    await screenshot('create-project-selected');
    await run(`document.querySelector('.create-project-cancel').click()`);
    await wait(`!document.querySelector('[aria-label="Create project"]')`);
    await check(`window.projectCreationFixture.creates.length === 0 && document.activeElement.getAttribute('aria-label') === 'Add project'`, 'Cancel must preserve projects and restore focus');
    await open();
    await check(`document.querySelector('[aria-label="Project name"]').value === ''`, 'Reopening must start an empty draft');
    await run(`window.projectCreationFixture.pickerError='Fixture picker unavailable'; document.querySelector('.create-project-add-folder').click()`);
    await wait(`document.querySelector('.create-project-error')?.textContent.includes('Fixture picker unavailable')`);
    await run(`window.projectCreationFixture.pickerError=''; document.querySelector('.create-project-add-folder').click()`);
    await wait(`document.querySelector('[aria-label="Project name"]').value === 'Hanzi Play'`);
    await run(`window.projectCreationFixture.createError='Fixture creation failed'; document.querySelector('.create-project-submit').click()`);
    await wait(`document.querySelector('.create-project-error')?.textContent.includes('Fixture creation failed')`);
    await check(`!document.querySelector('.create-project-cancel').disabled`, 'Failed creation must permit retry or cancel');
    await run(`window.projectCreationFixture.createError=''; window.projectCreationFixture.holdCreate=true; document.querySelector('.create-project-submit').click()`);
    await wait(`!!window.projectCreationFixture.finishCreate`);
    await check(`document.querySelector('.create-project-submit').disabled && document.querySelector('.create-project-close').disabled`, 'Pending creation must prevent duplicate submissions');
    await check(`document.activeElement === document.querySelector('[aria-label="Create project"]')`, 'The pending dialog must retain keyboard focus when its controls disable');
    await run(`document.querySelector('[aria-label="Create project"]').dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}))`);
    await check(`!!document.querySelector('[aria-label="Create project"]')`, 'Escape must not cancel an admitted creation');
    await run(`window.projectCreationFixture.finishCreate()`);
    await wait(`!document.querySelector('[aria-label="Create project"]') && !!document.querySelector('button[title="/Users/tester/Projects/Hanzi Play"]')`);
    await check(`window.projectCreationFixture.creates.length === 2 && window.projectCreationFixture.projects.length === 2`, 'Retry must add exactly one project and refresh the actual sidebar');

    await open();
    await run(`window.projectCreationFixture.nextFolder={id:'selected-2',path:'/Users/tester/Projects/Another project'}; document.querySelector('.create-project-add-folder').click()`);
    await wait(`document.querySelector('[aria-label="Project name"]').value === 'Another project'`);
    await run(`window.projectCreationFixture.holdCreate=false; window.projectCreationFixture.bootstrapErrorOnce=true; document.querySelector('.create-project-submit').click()`);
    await wait(`!document.querySelector('[aria-label="Create project"]')`);
    await check(`window.projectCreationFixture.projects.length === 3 && document.body.innerText.includes('Project created; refreshing the project list failed')`, 'A metadata refresh failure must not offer to retry an already successful creation');

    await run(`document.querySelector('.desktop-rail-settings').click()`);
    await wait(`!!document.querySelector('.settings-screen')`);
    await run(`document.querySelector('[data-nav-page="projects"]').click()`);
    await wait(`Array.from(document.querySelectorAll('.settings-screen button')).some(b => b.textContent.includes('添加项目'))`);
    await run(`Array.from(document.querySelectorAll('.settings-screen button')).find(b => b.textContent.includes('添加项目')).focus(); Array.from(document.querySelectorAll('.settings-screen button')).find(b => b.textContent.includes('添加项目')).click()`);
    await wait(`!!document.querySelector('[aria-label="Create project"]')`);
    await check(`!!document.querySelector('.settings-screen').closest('[inert]')`, 'Settings must remain underneath the create dialog');
    await run(`document.querySelector('[aria-label="Project name"]').dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}))`);
    await wait(`!document.querySelector('[aria-label="Create project"]')`);
    await check(`!!document.querySelector('.settings-screen') && !document.querySelector('.settings-screen').closest('[inert]')`, 'Escape must close only the project dialog and restore Settings');
    await check(`window.projectCreationFixture.rendererErrors.length === 0`, 'The real App must not throw renderer errors');

    await window.loadURL(url + '?dark');
    await wait(`!!document.querySelector('[aria-label="Add project"]') && !document.querySelector('[aria-label="Add project"]').closest('[inert]')`);
    await open(); await screenshot('create-project-dark');
    await run(`document.querySelector('.create-project-close').focus()`);
    await run(`document.activeElement.dispatchEvent(new KeyboardEvent('keydown',{key:'Tab',shiftKey:true,bubbles:true,cancelable:true}))`);
    await check(`document.activeElement.classList.contains('create-project-cancel')`, 'Shift-Tab must wrap within the modal');
    await run(`document.querySelector('[aria-label="Create project"]').dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}))`);
    await wait(`!document.querySelector('[aria-label="Create project"]')`);
    window.setContentSize(360, 650);
    await open();
    await check(`(() => { const d=document.querySelector('.create-project-dialog'),r=d.getBoundingClientRect(); return r.left>=0 && r.right<=innerWidth && d.scrollWidth<=d.clientWidth; })()`, 'Narrow dialog must fit the viewport');
    await screenshot('create-project-narrow');
    console.log(JSON.stringify({ opening: true, stagedSelection: true, cancel: true, retry: true, busy: true, sidebarRefresh: true, refreshFailure: true, settingsEntry: true, focusAndEscape: true, narrow: true }));
  } catch (error) { console.error(error); process.exitCode = 1; }
  finally { window.destroy(); app.quit(); }
}
void main().catch(error => { console.error(error); app.exit(1); });
