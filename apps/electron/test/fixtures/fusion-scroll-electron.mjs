import { app, BrowserWindow } from 'electron';
import { writeFileSync } from 'node:fs';
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
    const trigger = `document.querySelector('button[aria-label="选择模型…"]')`;
    const key = async (keyCode) => {
      window.webContents.sendInputEvent({ type: 'keyDown', keyCode });
      window.webContents.sendInputEvent({ type: 'keyUp', keyCode });
      await delay(40);
    };
    await wait(`Boolean(${trigger})`);
    await run(`${trigger}.scrollIntoView({block: 'center'})`);
    await delay(100);
    await run(`${trigger}.click()`);
    await wait('document.querySelectorAll("[role=option]").length === 73');
    const result = await run(`(() => { const list = document.querySelector('[role=listbox]'); return {
      options: list.querySelectorAll('[role=option]').length,
      height: list.closest('[role=dialog]').getBoundingClientRect().height,
      overflow: getComputedStyle(list).overflowY, scrollable: list.scrollHeight > list.clientHeight,
    }; })()`);
    await delay(100);
    writeFileSync('/tmp/fusion-scroll.png', (await window.webContents.capturePage()).toPNG());
    await run(`document.querySelector('[role=listbox]').scrollTop = 99999`);
    await delay(100);
    await run(`document.querySelector('[role=option]:last-child').click()`);
    await wait('!document.querySelector("[role=listbox]")');
    result.clicked = await run(`${trigger}.textContent`);
    await run(`${trigger}.click()`);
    await wait('Boolean(document.querySelector("[role=listbox]"))');
    const search = async (text) => {
      await run(`(() => { const input = document.querySelector('input[aria-label="搜索模型"]'); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, ${JSON.stringify(text)}); input.dispatchEvent(new Event('input', {bubbles:true})); })()`);
      await delay(70);
    };
    await search('Model 71');
    result.searchName = await run(`document.querySelector('[role=listbox]').textContent.includes('Model 71') && !document.querySelector('[role=listbox]').textContent.includes('Model 70')`);
    await search('model-69');
    result.searchId = await run(`document.querySelector('[role=listbox]').textContent.includes('Model 69') && !document.querySelector('[role=listbox]').textContent.includes('Model 70')`);
    await search('Configured provider');
    result.searchProvider = await run(`document.querySelector('[role=listbox]').textContent.includes('Model 00') && document.querySelector('[role=listbox]').textContent.includes('Model 71')`);
    await search('does-not-exist');
    result.empty = await run(`!document.querySelector('[role=listbox]').textContent.includes('Model 00')`);
    await search('');
    await key('Down');
    await key('End');
    await key('Enter');
    await wait('!document.querySelector("[role=listbox]")');
    result.keyboard = await run(`${trigger}.textContent`);
    await run(`${trigger}.click()`);
    await wait('Boolean(document.querySelector("[role=listbox]"))');
    await key('Escape');
    result.escapeClosed = await run('!document.querySelector("[role=listbox]") && Boolean(document.querySelector("[data-nav-page=fusion]"))');
    result.focusReturned = await run(`document.activeElement === ${trigger}`);
    const analyst = `document.querySelector('button[aria-label="选择 analyst…"]')`;
    await run(`${analyst}.scrollIntoView({block:'center', behavior:'instant'})`);
    const analystBottom = await run(`${analyst}.getBoundingClientRect().bottom`);
    window.setContentSize(1100, Math.ceil(analystBottom + 40));
    await run(`${analyst}.scrollIntoView({block:'end', behavior:'instant'})`);
    await delay(100);
    await run(`document.querySelector('button[aria-label="选择 analyst…"]').click()`);
    await wait('Boolean(document.querySelector("[role=listbox]"))');
    result.upwardPlacement = await run(`(() => {
      const trigger = ${analyst}.getBoundingClientRect();
      const menu = document.querySelector('[role=listbox]').closest('[role=dialog]').getBoundingClientRect();
      return { spaceBelow: innerHeight - trigger.bottom, triggerTop: trigger.top,
        menuTop: menu.top, menuBottom: menu.bottom, menuHeight: menu.height };
    })()`);
    process.stdout.write(`${JSON.stringify(result)}\n`);
  } finally { window.destroy(); app.quit(); }
}
main().catch((error) => { process.stderr.write(`${error.stack ?? error}\n`); app.exit(1); });
