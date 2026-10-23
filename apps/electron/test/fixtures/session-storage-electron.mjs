import { app, BrowserWindow } from 'electron';
import { mkdirSync, writeFileSync } from 'node:fs';
import assert from 'node:assert/strict';

process.on('uncaughtException', (error) => { console.error(error); app.exit(1); });
process.on('unhandledRejection', (error) => { console.error(error); app.exit(1); });
mkdirSync(process.env.LINGXI_TEST_USER_DATA, { recursive: true });
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
async function main() {
await app.whenReady();
const window = new BrowserWindow({ width: 420, height: 400, show: false, webPreferences: { sandbox: true, contextIsolation: true, backgroundThrottling: false, offscreen: true } });
try {
  await window.loadURL(process.argv.find((arg) => arg.startsWith('http://')));
  const run = (source) => window.webContents.executeJavaScript(source, true);
  const settle = () => run('new Promise(resolve => setTimeout(resolve, 30))');
  await settle();
  for (const dark of [false, true]) {
    await run(`storageFixture.theme(${dark})`);
    await settle();
    assert.equal(await run('document.documentElement.scrollWidth <= innerWidth'), true);
    if (process.env.LINGXI_STORAGE_SCREENSHOT) {
      writeFileSync(`${process.env.LINGXI_STORAGE_SCREENSHOT}-${dark ? 'dark' : 'light'}.png`, (await window.webContents.capturePage()).toPNG());
    }
  }
  await run('document.querySelector("button").click(); document.querySelector("button").click()');
  await settle();
  assert.equal(await run('storageFixture.calls'), 1);
  assert.equal(await run('document.querySelector("button").disabled'), true);
  await run('storageFixture.reject(new Error("disk still full"))');
  await settle();
  assert.equal(await run('Boolean(document.querySelector("[role=alert]"))'), true);
  assert.equal(await run('document.querySelector("button").disabled'), false);
  assert.match(await run('document.body.textContent'), /恢复未完成/);
  await run('document.querySelector("button").click()');
  await settle();
  assert.equal(await run('storageFixture.calls'), 2);
  await run('storageFixture.resolve()');
  await settle();
  assert.equal(await run('Boolean(document.querySelector("[role=alert]"))'), true);
  await run('storageFixture.ready()');
  await settle();
  assert.equal(await run('Boolean(document.querySelector("[role=alert]"))'), false);
  assert.match(await run('document.getElementById("history").textContent'), /原 session 的已有历史/);
  console.log(JSON.stringify({ doubleClick: 'one request', failedRetry: 'pause retained', successfulRecovery: 'original history retained', themes: ['light', 'dark'], narrowOverflow: false }));
} finally { window.destroy(); }
}
main().then(() => app.exit(0), (error) => { console.error(error); app.exit(1); });
