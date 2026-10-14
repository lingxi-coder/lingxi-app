import { app, BrowserWindow } from 'electron';
import assert from 'node:assert/strict';
const url = process.argv.find((argument) => argument.startsWith('http://'));
if (!url || !process.env.LINGXI_TEST_USER_DATA) throw new Error('fixture URL and isolated user data required');
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 900, height: 700, webPreferences: { sandbox: true } });
  try {
    window.webContents.on('console-message', (event) => process.stderr.write(`${event.message}\n`));
    await window.loadURL(url);
    const run = (script) => window.webContents.executeJavaScript(script);
    const wait = async (expression) => {
      const deadline = Date.now() + 6000;
      while (Date.now() < deadline) { if (await run(expression)) return; await delay(20); }
      throw new Error(`Timed out: ${expression}\n${await run('document.body.innerText')}`);
    };
    const prompt = async (text) => {
      await run(`(() => { const input = document.querySelector('[aria-label="Prompt"]'); input.textContent = ${JSON.stringify(text)}; input.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText', data: ${JSON.stringify(text)} })); })()`);
      await delay(60);
    };
    const select = async (value) => {
      await run(`(() => { const select = document.querySelector('[aria-label="Fusion 策略"]'); select.value = ${JSON.stringify(value)}; select.dispatchEvent(new Event('change', { bubbles: true })); })()`); await delay(60);
    };
    const send = () => run(`document.querySelector('[aria-label="Send prompt"]').click()`);
    await wait(`Boolean(window.__composerDraftTest && document.querySelector('[aria-label="Prompt"]'))`);
    await prompt('/fusion inspect the code');
    await wait(`Boolean(document.querySelector('[aria-label="Fusion 策略"]'))`);
    assert.deepEqual(await run(`[...document.querySelector('[aria-label="Fusion 策略"]').options].map(option => option.textContent)`), ['选择策略…', '代码审查 · 用户', '代码审查 · 项目', '代码审查 · 本地 · 策略已不可用']);
    assert.equal(await run(`document.querySelector('[aria-label="Fusion 策略"] option[value="local:shared"]').disabled`), true);
    await send(); await delay(60);
    assert.equal(await run(`document.querySelector('[aria-label="Prompt"]').textContent`), '/fusion inspect the code');
    assert.deepEqual(await run('window.__composerDraftTest.slashCommands()'), []);
    assert.equal(await run(`document.body.innerText.includes('请选择一个 Fusion 策略')`), true);
    await select('user:shared'); await send();
    await wait('window.__composerDraftTest.slashCommands().length === 1');
    assert.equal(await run('window.__composerDraftTest.slashCommands()[0]'), '/fusion --strategy user:shared inspect the code');
    await prompt('/fusion --strategy local:shared typed excluded');
    assert.equal(await run(`document.querySelector('[aria-label="Fusion 策略"]').value`), 'local:shared');
    assert.equal(await run(`document.querySelector('[aria-label="Fusion 策略"]').selectedOptions[0].textContent.includes('策略已不可用')`), true);
    await prompt('/fusion --strategy project:shared inspect explicitly');
    assert.equal(await run(`document.querySelector('[aria-label="Fusion 策略"]').disabled`), true);
    assert.equal(await run(`document.querySelector('[aria-label="Fusion 策略"]').value`), 'project:shared');
    await send(); await wait('window.__composerDraftTest.slashCommands().length === 2');
    assert.equal(await run('window.__composerDraftTest.slashCommands()[1]'), '/fusion --strategy project:shared inspect explicitly');
    await prompt('/fusion session A draft'); await select('user:shared');
    await run(`window.__composerDraftTest.setFusionScopeAdmitted('user', false)`); await delay(60);
    assert.equal(await run(`document.querySelector('[aria-label="Fusion 策略"]').value`), 'user:shared');
    assert.equal(await run(`document.querySelector('[aria-label="Fusion 策略"]').selectedOptions[0].textContent.includes('策略已不可用')`), true);
    await run(`window.__composerDraftTest.setFusionScopeAdmitted('user', true)`); await delay(60);
    await run(`window.__composerDraftTest.switchSession('bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb')`); await delay(60);
    await prompt('/fusion session B draft');
    assert.equal(await run(`document.querySelector('[aria-label="Fusion 策略"]').value`), '');
    await run('window.__composerDraftTest.setFusionDefault(true)'); await delay(60);
    assert.equal(await run(`document.querySelector('[aria-label="Fusion 策略"]').selectedOptions[0].textContent`), '默认 · 代码审查 · 项目');
    await send(); await wait('window.__composerDraftTest.slashCommands().length === 3');
    assert.equal(await run('window.__composerDraftTest.slashCommands()[2]'), '/fusion session B draft');
    await prompt('/fusion settings');
    await run(`[...document.querySelectorAll('button')].find(button => button.textContent === '管理策略').click()`);
    assert.deepEqual(await run('window.__composerDraftTest.openedSettings()'), ['fusion']);
    process.stdout.write('Fusion composer interactions passed\n');
  } finally { window.destroy(); app.quit(); }
}
main().catch((error) => { process.stderr.write(`${error.stack ?? error}\n`); app.exit(1); });
