import assert from 'node:assert/strict';
import { app, BrowserWindow } from 'electron';
import { writeFileSync } from 'node:fs';
const url = process.argv.find((value) => value.startsWith('http://'));
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
app.commandLine.appendSwitch('disable-gpu');
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function main() {
await app.whenReady();
const window = new BrowserWindow({ show: false, width: 1000, height: 720, webPreferences: { sandbox: true } });
const run = (expression) => window.webContents.executeJavaScript(expression);
const wait = async (expression) => {
  for (let i = 0; i < 160; i++) { if (await run(expression)) return; await delay(25); }
  throw new Error(`Timed out: ${expression}`);
};
const click = (selector) => run(`document.querySelector(${JSON.stringify(selector)}).click()`);
try {
  await window.loadURL(url);
  await wait('Boolean(window.__codexAuthTest)');
  assert.equal(await run('Boolean(document.querySelector("input[type=password], [data-testid=provider-connection-test]"))'), false);
  await click('[data-testid=codex-login]');
  await wait('Boolean(document.querySelector("[data-testid=codex-login-cancel]"))');
  assert.equal(await run('document.querySelector("[data-testid=codex-login]").disabled'), true);
  assert.equal(await run('document.documentElement.scrollWidth > innerWidth'), false);
  writeFileSync('/tmp/lingxi-codex-auth.png', (await window.webContents.capturePage()).toPNG());
  await click('[data-testid=codex-login-cancel]');
  await wait('!document.querySelector("[data-testid=codex-login-cancel]")');
  assert.equal((await run('window.__codexAuthTest.state()')).cancel, 1);
  await click('[data-testid=codex-login]');
  await run('window.__codexAuthTest.complete()');
  await wait('document.body.textContent.includes("重新登录")');
  // The signed-in account is shown on the 账号 row, and only while signed in.
  assert.equal(await run('document.body.textContent.includes("已登录：user@example.com")'), true);
  assert.equal(await run('document.documentElement.scrollWidth > innerWidth'), false);
  writeFileSync('/tmp/lingxi-codex-auth-signed-in.png', (await window.webContents.capturePage()).toPNG());
  await run('[...document.querySelectorAll("button")].find((button) => button.textContent === "退出登录").click()');
  await wait('!window.__codexAuthTest.state().configured');
  assert.equal(await run('document.body.textContent.includes("user@example.com")'), false);
  await click('[data-testid=codex-login]');
  await run('window.__codexAuthTest.fail()');
  await wait('document.body.textContent.includes("OAuth callback timed out")');
  await click('[data-testid=codex-login]');
  await run('window.__codexAuthTest.close()');
  await wait('window.__codexAuthTest.state().cancel === 2');
  await run('window.__codexAuthTest.open(true)');
  await wait('Boolean(document.querySelector("[data-testid=codex-login]"))');
  await click('[data-testid=codex-login]');
  await run('window.__codexAuthTest.complete()');
  await wait('window.__codexAuthTest.state().close === 1');
  const state = await run('window.__codexAuthTest.state()');
  assert.equal(state.currentModel, 'openai-chatgpt/gpt-5.6-sol');
  assert.equal(state.apply, 1);
  assert.equal(state.logout, 1);
  console.log(JSON.stringify({ ...state, screenshot: '/tmp/lingxi-codex-auth.png' }));
  window.destroy(); app.exit(0);
} catch (error) { console.error(error); window.destroy(); app.exit(1); }

}
void main().catch((error) => { console.error(error); app.exit(1); });
