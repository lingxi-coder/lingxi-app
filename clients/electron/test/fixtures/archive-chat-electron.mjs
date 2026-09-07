import { app, BrowserWindow } from 'electron';
const url = process.argv.find((argument) => argument.startsWith('http://'));
if (!url || !process.env.LINGXI_TEST_USER_DATA) throw new Error('fixture URL and isolated user data required');
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 1100, height: 800, webPreferences: { sandbox: true } });
  try {
    await window.loadURL(url);
    const run = (script) => window.webContents.executeJavaScript(script);
    const wait = async (expression) => {
      const deadline = Date.now() + 6000;
      while (Date.now() < deadline) { if (await run(expression)) return; await delay(20); }
      throw new Error(`Timed out: ${expression}\n${await run('document.body.innerText')}`);
    };
    const dialog = 'document.querySelector("[role=dialog]")';
    const confirm = 'document.querySelector(".desktop-dialog-action--primary")';
    const cancel = 'document.querySelector(".desktop-dialog-action--cancel")';
    const open = async () => {
      await run('document.querySelector("#opener").focus(); document.querySelector("#opener").click()');
      await wait(`Boolean(${dialog})`);
      await delay(30);
    };
    const configure = async (settings) => {
      await run(`window.archiveChatFixture.configure(${JSON.stringify(settings)})`);
      await delay(30);
    };
    const state = () => run('window.archiveChatFixture.state()');
    await wait('Boolean(window.archiveChatFixture)');
    await configure({ loading: true });
    await open();
    const result = { loadingDisabled: await run(`${confirm}.disabled`) };
    await run(`${confirm}.click(); ${cancel}.click()`);
    await wait(`!${dialog}`);
    result.afterCancel = await state();
    await configure({});
    await open();
    result.content = await run(`${dialog}.textContent`);
    await run(`${confirm}.focus(); ${confirm}.dispatchEvent(new KeyboardEvent('keydown', {key:'Tab', bubbles:true, cancelable:true}))`);
    result.forwardTrap = await run(`document.activeElement === ${dialog}.querySelector('button')`);
    await run(`document.activeElement.dispatchEvent(new KeyboardEvent('keydown', {key:'Tab', shiftKey:true, bubbles:true, cancelable:true}))`);
    result.backwardTrap = await run(`document.activeElement === ${confirm}`);
    await run(`document.activeElement.dispatchEvent(new KeyboardEvent('keydown', {key:'Escape', bubbles:true, cancelable:true}))`);
    await wait(`!${dialog}`);
    result.escapeRestored = await run('document.activeElement === document.querySelector("#opener")');
    await open();
    await configure({ busy: true });
    result.busyFocusContained = await run(`document.activeElement === ${dialog}`);
    result.busyTabPrevented = await run(`!${dialog}.dispatchEvent(new KeyboardEvent('keydown', {key:'Tab', bubbles:true, cancelable:true}))`);
    result.busyAllDisabled = await run(`[...${dialog}.querySelectorAll('button')].every(button => button.disabled)`);
    await run(`${cancel}.click(); ${confirm}.click(); ${dialog}.dispatchEvent(new KeyboardEvent('keydown', {key:'Escape', bubbles:true, cancelable:true}))`);
    await delay(30);
    result.busyRemainsOpen = (await state()).open;
    await configure({ error: 'Unable to check scheduled tasks' });
    result.errorDisabled = await run(`${confirm}.disabled`);
    await configure({});
    await run(`${confirm}.click()`);
    await wait(`!${dialog}`);
    result.afterConfirm = await state();
    process.stdout.write(`${JSON.stringify(result)}\n`);
  } finally { window.destroy(); app.quit(); }
}
main().catch((error) => { process.stderr.write(`${error.stack ?? error}\n`); app.exit(1); });
