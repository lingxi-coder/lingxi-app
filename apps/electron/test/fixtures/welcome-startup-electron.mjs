import { app, BrowserWindow } from 'electron';
import { writeFileSync } from 'node:fs';

const url = process.argv.find(value => value.startsWith('http://'));
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));

async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 1362, height: 900, webPreferences: { sandbox: true } });
  try {
    await window.loadURL(url);
    const run = script => window.webContents.executeJavaScript(script);
    const wait = async expression => {
      const deadline = Date.now() + 6000;
      while (Date.now() < deadline) { if (await run(expression)) return; await delay(20); }
      throw new Error(`Timed out: ${expression}\n${await run('document.body.innerText')}`);
    };
    const prompt = `document.querySelector('[aria-label="Prompt"]')`;
    const send = `document.querySelector('[aria-label="Send prompt"]')`;
    const welcome = `document.body.innerText.includes('What should we build')`;
    const enter = async (text, submit = true) => {
      await run(`(() => { const el = ${prompt}; el.focus(); el.textContent = ${JSON.stringify(text)}; el.dispatchEvent(new Event('input', { bubbles: true })); })()`);
      await delay(30);
      if (!submit) return;
      window.webContents.sendInputEvent({ type: 'keyDown', keyCode: 'Return' });
      window.webContents.sendInputEvent({ type: 'keyUp', keyCode: 'Return' });
      await delay(50);
    };
    await wait(`${prompt} && ${welcome}`);
    const result = { bootstrapWelcome: true, bootstrapEditable: await run(`${prompt}.isContentEditable`) };
    await enter('Draft before secure bootstrap');
    result.bootstrapBlocked = await run(`${send}.disabled && window.welcomeStartupFixture.sent.length === 0 && ${prompt}.textContent === 'Draft before secure bootstrap'`);
    await run('window.welcomeStartupFixture.finishBootstrap()');
    await wait(`${send} && !${send}.disabled`);
    await enter('Previous session message');
    await wait('window.welcomeStartupFixture.sent.length === 1');
    await wait(`document.querySelector('.beta-stage')?.innerText.includes('Previous session message') || document.body.innerText.includes('Previous session message')`);
    await run(`document.querySelector('.sidebar-primary-action').click()`);
    await wait('window.welcomeStartupFixture.newSessionPending');
    await wait(`document.querySelector('main').innerText.includes('What should we build in LingXi-Next?') && ${send}.disabled`);
    result.pendingWelcome = await run(`document.body.innerText.includes('What should we build in LingXi-Next?') && !document.querySelector('main').innerText.includes('Previous session message')`);
    writeFileSync('/tmp/lingxi-new-session-welcome.png', (await window.webContents.capturePage()).toPNG());
    result.pendingEditable = await run(`${prompt}.isContentEditable && ${prompt}.getAttribute('aria-disabled') === 'false'`);
    await enter('Draft while new session starts');
    result.pendingBlocked = await run(`${send}.disabled && window.welcomeStartupFixture.sent.length === 1 && ${prompt}.textContent === 'Draft while new session starts'`);
    await run('window.welcomeStartupFixture.finishNewSession()');
    await wait(`!document.querySelector('.sidebar-primary-action').disabled && !${send}.disabled`);
    result.draftPreserved = await run(`${prompt}.textContent === 'Draft while new session starts'`);
    window.webContents.sendInputEvent({ type: 'keyDown', keyCode: 'Return' });
    window.webContents.sendInputEvent({ type: 'keyUp', keyCode: 'Return' });
    await wait('window.welcomeStartupFixture.sent.length === 2');
    result.connectedSend = await run(`window.welcomeStartupFixture.sent[1][0] === 'new-session' && window.welcomeStartupFixture.sent[1][1] === 'Draft while new session starts'`);
    await enter('Draft in the existing session', false);
    await run(`document.querySelector('.sidebar-primary-action').click()`);
    await wait(`document.querySelector('main').innerText.includes('What should we build in LingXi-Next?') && document.querySelector('.sidebar-primary-action').disabled`);
    await enter('Draft belonging to the failed session', false);
    await run('window.welcomeStartupFixture.failNewSession()');
    await wait(`!document.querySelector('.sidebar-primary-action').disabled && document.body.innerText.includes('fixture engine startup failed')`);
    result.failureRestoresDraft = await run(`${prompt}.textContent === 'Draft in the existing session' && !document.querySelector('main').innerText.includes('Draft belonging to the failed session')`);
    console.log(JSON.stringify(result));
  } catch (error) { console.error(error); process.exitCode = 1; }
  finally { window.destroy(); app.quit(); }
}
void main().catch(error => { console.error(error); app.exit(1); });
