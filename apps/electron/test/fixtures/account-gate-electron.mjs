import { app, BrowserWindow } from 'electron';
const url = process.argv.find(value => value.startsWith('http://'));
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 1100, height: 800, webPreferences: { sandbox: true } });
  try {
    await window.loadURL(url);
    const run = script => window.webContents.executeJavaScript(script);
    const wait = async expression => {
      const deadline = Date.now() + 6000;
      while (Date.now() < deadline) { if (await run(expression)) return; await delay(20); }
      throw new Error(`Timed out: ${expression}\n${await run('document.body.innerText')}`);
    };
    const login = `document.querySelector('main[aria-label="Sign In"]')`;
    const google = `${login}?.querySelector('button')`;
    const draft = `document.querySelector('[aria-label="Workspace draft"]')`;
    const click = async selector => {
      const point = await run(`(() => { const r = (${selector}).getBoundingClientRect(); return { x: Math.round(r.x + r.width/2), y: Math.round(r.y + r.height/2) }; })()`);
      window.webContents.sendInputEvent({ type: 'mouseDown', button: 'left', clickCount: 1, ...point });
      window.webContents.sendInputEvent({ type: 'mouseUp', button: 'left', clickCount: 1, ...point });
    };
    await wait(`window.accountGateFixture && ${draft}`);
    await run('window.accountGateFixture.expire()');
    await wait(`${google} && !${google}.disabled`);
    const result = {};
    result.fullScreen = await run(`(() => { const r = ${login}.getBoundingClientRect(); return r.x === 0 && r.y === 0 && r.width === innerWidth && r.height === innerHeight; })()`);
    result.backgroundHidden = await run(`${draft}.getClientRects().length === 0 && document.querySelector('[aria-label="Workspace"]').getClientRects().length === 0`);
    for (let n = 0; n < 2; n++) {
      window.webContents.sendInputEvent({ type: 'keyDown', keyCode: 'Tab' });
      window.webContents.sendInputEvent({ type: 'keyUp', keyCode: 'Tab' });
      await delay(20);
    }
    result.keyboardNavigation = await run(`document.activeElement?.getAttribute('aria-label') === 'Email' && window.accountGateFixture.backgroundTabs === 0`);
    await click(google);
    await wait('window.accountGateFixture.oauthCalls === 1');
    result.nativeClick = true;
    await run('window.accountGateFixture.timeout()');
    await wait(`!${google}.disabled`);
    await click(google);
    await wait('window.accountGateFixture.oauthCalls === 2');
    result.retryAfterTimeout = true;
    await run('window.accountGateFixture.finishOldReply()');
    await delay(50);
    result.lateReplyIgnored = await run(`${google}.disabled && !document.body.innerText.includes('Stale timeout')`);
    await run('window.accountGateFixture.succeed()');
    await wait(`!${login} && ${draft}.getClientRects().length > 0`);
    result.draftPreserved = await run(`${draft}.value === 'Retain this local draft'`);
    await run('window.accountGateFixture.signOut()');
    await click(`document.querySelector('[aria-label="Workspace"] button')`);
    await wait(`${google} && !${google}.disabled`);
    result.manualSignInFullScreen = await run(`${draft}.getClientRects().length === 0`);
    console.log(JSON.stringify(result));
  } catch (error) { console.error(error); process.exitCode = 1; }
  finally { window.destroy(); app.quit(); }
}
void main().catch(error => { console.error(error); app.exit(1); });
