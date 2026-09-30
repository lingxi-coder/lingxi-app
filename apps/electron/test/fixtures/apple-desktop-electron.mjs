import { app, BrowserWindow } from 'electron';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
const url = process.argv.find(value => value.startsWith('http://'));
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const errors = [];
const accessibility = [];
let checksPassed = true;
async function main() {
await app.whenReady();
const window = new BrowserWindow({ show: false, width: 1280, height: 850, useContentSize: true, webPreferences: { sandbox: true } });
window.webContents.on('console-message', details => {
  if (details.level === 'error' || details.level === 3) errors.push(details.message);
});
const run = script => window.webContents.executeJavaScript(script);
async function wait(expression) {
  const deadline = Date.now() + 12000;
  while (Date.now() < deadline) { if (await run(expression)) return; await delay(50); }
  throw new Error(`Timed out: ${expression}\n${await run('document.body.innerText')}`);
}
const output = process.env.LINGXI_VISUAL_OUTPUT;
mkdirSync(output, { recursive: true });
try {
  for (const theme of ['light', 'dark']) {
    await window.loadURL(`${url}?theme=${theme}`);
    await wait(`document.querySelector('[contenteditable]') && document.body.innerText.includes('Refine desktop experience')`);
    window.webContents.sendInputEvent({ type: 'mouseMove', x: 900, y: 120 });
    await delay(250);
    writeFileSync(join(output, `desktop-${theme}.png`), (await window.webContents.capturePage()).toPNG());
    window.webContents.debugger.attach('1.3');
    for (const [name, value] of [['prefers-reduced-motion', 'reduce'], ['prefers-reduced-transparency', 'reduce'], ['prefers-contrast', 'more']]) {
      await window.webContents.debugger.sendCommand('Emulation.setEmulatedMedia', { features: [{ name, value }] });
      const check = await run(`(() => {
        const sidebar = getComputedStyle(document.querySelector('.desktop-sidebar'));
        const button = getComputedStyle(document.querySelector('button'));
        return { matched: matchMedia('(${name}: ${value})').matches, backdrop: sidebar.backdropFilter, background: sidebar.backgroundColor, transition: button.transitionDuration, outline: button.outlineStyle, border: sidebar.borderRightColor, color: sidebar.color };
      })()`);
      const passed = check.matched && (name === 'prefers-reduced-motion' ? check.transition.split(',').every(value => parseFloat(value) === 0) : name === 'prefers-reduced-transparency' ? check.backdrop === 'none' && !check.background.startsWith('rgba') : check.outline === 'solid' && check.backdrop === 'none');
      accessibility.push({ theme, name, passed, ...check });
      checksPassed &&= passed;
    }
    await window.webContents.debugger.sendCommand('Emulation.setEmulatedMedia', { features: [] });
    window.webContents.debugger.detach();
    await run(`(() => {
      const editor = document.querySelector('[contenteditable="true"]');
      editor.focus();
      editor.textContent = 'Refine the desktop interface with native macOS details.';
      editor.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText' }));
    })()`);
    await delay(200);
    writeFileSync(join(output, `composer-${theme}.png`), (await window.webContents.capturePage()).toPNG());
    window.webContents.debugger.attach('1.3');
    await window.webContents.debugger.sendCommand('DOM.enable');
    await window.webContents.debugger.sendCommand('CSS.enable');
    const { root } = await window.webContents.debugger.sendCommand('DOM.getDocument');
    const { nodeId } = await window.webContents.debugger.sendCommand('DOM.querySelector', { nodeId: root.nodeId, selector: 'button[aria-label="Send prompt"]' });
    await window.webContents.debugger.sendCommand('CSS.forcePseudoState', { nodeId, forcedPseudoClasses: ['focus', 'focus-visible'] });
    await run(`document.querySelector('button[aria-label="Send prompt"]').focus()`);
    await delay(150);
    const sendCheck = await run(`(() => {
      const send = document.querySelector('button[aria-label="Send prompt"]');
      const style = getComputedStyle(send);
      const luminance = color => {
        const channels = color.match(/[0-9.]+/g).slice(0, 3).map(Number).map(value => {
          const normalized = color.startsWith('color(srgb') ? value : value / 255;
          return normalized <= 0.04045 ? normalized / 12.92 : ((normalized + 0.055) / 1.055) ** 2.4;
        });
        return channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722;
      };
      const foreground = luminance(style.color);
      const background = luminance(style.backgroundColor);
      return { focusVisible: send.matches(':focus-visible'), outline: style.outlineColor, outlineStyle: style.outlineStyle, outlineWidth: style.outlineWidth, foreground: style.color, background: style.backgroundColor, contrast: (Math.max(foreground, background) + 0.05) / (Math.min(foreground, background) + 0.05) };
    })()`);
    const sendPassed = sendCheck.focusVisible && sendCheck.outline !== 'rgb(255, 255, 255)' && sendCheck.outline !== 'rgba(0, 0, 0, 0)' && sendCheck.outlineStyle !== 'none' && parseFloat(sendCheck.outlineWidth) > 0 && sendCheck.foreground === 'rgb(255, 255, 255)' && sendCheck.contrast >= 4.5;
    accessibility.push({ theme, name: 'primary-send-focus-and-contrast', passed: sendPassed, ...sendCheck });
    checksPassed &&= sendPassed;
    writeFileSync(join(output, `send-focus-${theme}.png`), (await window.webContents.capturePage()).toPNG());
    await window.webContents.debugger.sendCommand('CSS.forcePseudoState', { nodeId, forcedPseudoClasses: [] });
    window.webContents.debugger.detach();
    await run(`document.querySelector('button[title="Archive chat"]').click()`);
    await wait(`document.querySelector('[role="dialog"][aria-label="Archive chat"]') && !document.body.innerText.includes('Checking scheduled tasks')`);
    await delay(250);
    writeFileSync(join(output, `archive-${theme}.png`), (await window.webContents.capturePage()).toPNG());
    await run(`document.querySelector('button[aria-label="Close archive dialog"]').click()`);
    await run(`Array.from(document.querySelectorAll('button')).find(b => /^(Settings|设置)/.test(b.textContent.trim())).click()`);
    await wait(`document.querySelector('[data-nav-page="general"]')`);
    await delay(250);
    writeFileSync(join(output, `settings-${theme}.png`), (await window.webContents.capturePage()).toPNG());
  }
  writeFileSync(join(output, 'runtime-errors.json'), JSON.stringify(errors, null, 2));
  writeFileSync(join(output, 'accessibility-checks.json'), JSON.stringify(accessibility, null, 2));
  console.log(JSON.stringify({ output, errors, accessibility }));
  if (errors.length || !checksPassed) process.exitCode = 1;
} catch (error) { console.error(error); process.exitCode = 1; }
finally { window.destroy(); app.exit(process.exitCode || 0); }

}
void main().catch(error => { console.error(error); app.exit(1); });
