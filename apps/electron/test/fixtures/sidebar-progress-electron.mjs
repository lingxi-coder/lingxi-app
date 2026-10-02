import { configureFixtureWindow } from './electron-test-environment.mjs';
import { app, BrowserWindow } from 'electron';
import { writeFileSync } from 'node:fs';
const url = process.argv.find(argument => argument.startsWith('http://'));
if (!url || !process.env.LINGXI_TEST_USER_DATA) throw new Error('fixture URL and isolated user data required');
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 1000, height: 800, webPreferences: { sandbox: true, backgroundThrottling: false } });
  try {
    await window.loadURL(url);
    await configureFixtureWindow(window);
    const run = script => window.webContents.executeJavaScript(script);
    const wait = async expression => {
      const deadline = Date.now() + 6000;
      while (Date.now() < deadline) { if (await run(expression)) return; await delay(20); }
      throw new Error(`Timed out: ${expression}\n${await run('document.body.innerText')}`);
    };
    await wait('document.querySelectorAll(".sidebar-session-progress").length === 4');
    const result = await run(`({
      bars: [...document.querySelectorAll('.sidebar-session-progress')].map(bar => {
        const rect = bar.getBoundingClientRect();
        const row = bar.closest('.sidebar-tree-row').getBoundingClientRect();
        return { width: rect.width, height: rect.height, rightGap: row.right - rect.right,
          centerOffset: rect.y + rect.height / 2 - row.y - row.height / 2,
          animation: getComputedStyle(bar.firstElementChild).animationName };
      }),
      pendingCount: document.querySelectorAll('[aria-label="Waiting for input"]').length,
      errorCount: document.querySelectorAll('[aria-label="Session error"]').length,
    })`);
    writeFileSync('/tmp/sidebar-progress.png', (await window.webContents.capturePage()).toPNG());
    await run(`document.querySelector('button[aria-label="Search chats"]').click()`);
    await wait(`Boolean(document.querySelector('input[aria-label="Search chats and projects"]'))`);
    await run(`(() => {
      const input = document.querySelector('input[aria-label="Search chats and projects"]');
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, 'long');
      input.dispatchEvent(new Event('input', { bubbles: true }));
    })()`);
    await wait(`!document.querySelector('button[title="Short"]') && Boolean(document.querySelector('button[title="A very long running session title that must truncate before its progress bar"]'))`);
    result.searchFilters = true;
    await run(`document.querySelector('button[aria-label="Close search"]').click()`);
    await wait(`Boolean(document.querySelector('button[title="Short"]'))`);
    await run(`document.querySelector('button[aria-label="Hide sidebar"]').click()`);
    await wait(`document.querySelector('#desktop-session-sidebar').getBoundingClientRect().width <= 2`);
    result.collapsed = await run(`document.querySelector('#desktop-session-sidebar').hasAttribute('inert')`);
    await run(`document.querySelector('button[aria-label="Show sidebar"]').click()`);
    await wait(`document.querySelector('#desktop-session-sidebar').getBoundingClientRect().width > 200`);
    await window.webContents.debugger.sendCommand('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-reduced-motion', value: 'reduce' }] });
    result.reducedMotion = await run(`[...document.querySelectorAll('.sidebar-session-progress > span')].every(span => getComputedStyle(span).animationName === 'none')`);
    await run(`[...document.querySelectorAll('button')].find(button => button.textContent.includes('Show more')).click()`);
    await wait(`Boolean(document.querySelector('button[title="Idle session"]'))`);
    await run(`document.querySelector('button[title="Idle session"]').click()`);
    await wait(`Boolean(document.querySelector('[role=progressbar][aria-label="Opening session"]'))`);
    result.opening = true;
    await run('window.sidebarProgressFixture.finishOpen()');
    await wait(`!document.querySelector('[role=progressbar][aria-label="Opening session"]')`);
    result.settled = true;
    process.stdout.write(`${JSON.stringify(result)}\n`);
  } finally { window.destroy(); app.quit(); }
}
main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); app.exit(1); });
