import { app, BrowserWindow } from 'electron';
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
app.commandLine.appendSwitch('disable-gpu');
async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 1400, height: 900, webPreferences: { sandbox: true } });
  try {
    const js = source => window.webContents.executeJavaScript(source);
    await window.loadURL(process.argv.find(value => value.startsWith('http://')));
    const until = async expression => {
      const deadline = Date.now() + 8000;
      while (!await js(expression)) { if (Date.now() > deadline) throw new Error(`Missing state: ${expression}`); await new Promise(resolve => setTimeout(resolve, 20)); }
    };
    await until('Boolean(window.__composerDraftTest)');
    await js(`document.querySelector('button[aria-label^="Model: "]').click()`);
    await until(`Boolean(document.querySelector('[aria-label="Model settings"]'))`);
    await js(`Array.from(document.querySelectorAll('[role="menuitem"]')).find(button => button.textContent.trim().startsWith('Model')).click()`);
    await until(`Boolean(document.querySelector('[aria-label="Available models"]'))`);
    const result = await js(`(() => {
      const list = document.querySelector('[aria-label="Available models"]');
      const sections = list.querySelectorAll('section');
      const first = list.querySelector('[role="menuitemradio"]');
      const grouped = sections.length === 1 && sections[0].textContent.includes('OpenAI') && sections[0].textContent.includes('Configured');
      const subscriptionFirst = first && !first.disabled && !first.querySelector('svg') && list.textContent.indexOf('ChatGPT subscription') < list.textContent.indexOf('API key');
      first.click();
      return { grouped, subscriptionFirst };
    })()`);
    await until(`window.__selectedModel === 'openai-chatgpt/gpt-6.1-sol'`);
    result.selectedOAuth = await js(`window.__selectedModel === 'openai-chatgpt/gpt-6.1-sol'`);
    result.noSettings = await js('window.__composerDraftTest.openedSettings().length === 0');
    process.stdout.write(`${JSON.stringify(result)}\n`);
  } finally { window.destroy(); }
}
main().then(() => app.quit(), error => { process.stderr.write(`${error.stack}\n`); app.exit(1); });
