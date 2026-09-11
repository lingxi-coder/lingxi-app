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
    await wait('document.querySelectorAll("li").length === 27');
    const result = {
      count: await run('document.querySelectorAll("li").length'),
      navPresent: await run('Boolean(document.querySelector("[data-nav-page=archived-chats]"))'),
      projects: await run('[...new Set([...document.querySelectorAll("li")].map(row => row.textContent.match(/\\/fixture\\/project-[a-z]+/)[0]))].sort()'),
    };
    const search = async (value) => {
      await run(`(() => { const input = document.querySelector('input[aria-label="搜索已归档会话"]'); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, ${JSON.stringify(value)}); input.dispatchEvent(new Event('input', { bubbles: true })); })()`);
    };
    await search('project-beta');
    await wait('document.querySelectorAll("li").length === 13');
    result.projectSearchCount = await run('document.querySelectorAll("li").length');
    await search('no matching archive');
    await wait('Boolean(document.querySelector("[role=status]"))');
    result.emptySearch = await run('document.querySelector("[role=status]").textContent');
    await search('conversation 23');
    await wait('document.querySelectorAll("li").length === 1');
    await run('document.querySelector("li button").click()');
    await wait('Boolean(document.querySelector("[role=alert]"))');
    result.failed = await run('({ ...window.archivedSettingsFixture.state(), error: document.querySelector("[role=alert]").textContent, retryEnabled: !document.querySelector("li button").disabled })');
    await delay(100); // Let Chromium paint the verified error state before capturing it.
    writeFileSync('/tmp/lingxi-archived-settings.png', (await window.webContents.capturePage()).toPNG());
    await run('document.querySelector("li button").click()');
    await wait('!document.querySelector("[role=dialog]")');
    result.succeeded = await run('window.archivedSettingsFixture.state()');
    process.stdout.write(`${JSON.stringify(result)}\n`);
  } finally { window.destroy(); app.quit(); }
}
main().catch((error) => { process.stderr.write(`${error.stack ?? error}\n`); app.exit(1); });
