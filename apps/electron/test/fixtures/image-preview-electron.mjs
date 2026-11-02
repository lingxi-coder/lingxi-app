import { fileURLToPath } from 'node:url';
import { app, BrowserWindow, ipcMain } from 'electron';
import { mkdir, stat, writeFile, readFile } from 'node:fs/promises';
const url = process.argv.find((arg) => arg.startsWith('http://'));
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 1440, height: 960, webPreferences: { sandbox: true, preload: fileURLToPath(new URL('./image-preview-preload.cjs', import.meta.url)) } });
  const wc = window.webContents;
  wc.session.on('will-download', event => event.preventDefault());
  let saveMode = 'save';
  const { registerImageSave } = await import(process.env.LINGXI_IMAGE_SAVE_MODULE);
  registerImageSave(ipcMain, event => {
    if (event.sender !== wc || event.senderFrame !== wc.mainFrame) throw new Error('unauthorized');
  }, async () => {
    if (saveMode === 'error') throw new Error('fixture write failure');
    return { canceled: saveMode === 'cancel', filePath: process.env.LINGXI_IMAGE_DOWNLOAD };
  });
  const run = (code) => wc.executeJavaScript(code);
  const key = async (keyCode) => { wc.sendInputEvent({ type: 'keyDown', keyCode }); wc.sendInputEvent({ type: 'keyUp', keyCode }); await delay(50); };
  const open = async (id) => {
    await run(`document.querySelector('#${id} .image-preview-trigger').focus(); document.querySelector('#${id} .image-preview-trigger').click()`);
    for (let n = 0; n < 100 && !await run(`!!document.querySelector('.image-viewer') && document.querySelector('.image-viewer-percentage').textContent !== '—'`); n++) await delay(25);
  };
  const inspect = () => run(`(() => {
    const dialog = document.querySelector('.image-viewer'), image = dialog.querySelector('.image-viewer-image');
    const i = image.getBoundingClientRect(), v = dialog.querySelector('.image-viewer-viewport').getBoundingClientRect();
    const z = dialog.querySelector('.image-viewer-zoom').getBoundingClientRect(), a = dialog.querySelector('.image-viewer-actions').getBoundingClientRect();
    return { fits: i.left >= v.left - 1 && i.right <= v.right + 1 && i.top >= v.top - 1 && i.bottom <= v.bottom + 1,
      centered: Math.abs((i.left + i.right) / 2 - (v.left + v.right) / 2) < 1 && Math.abs((i.top + i.bottom) / 2 - (v.top + v.bottom) / 2) < 1,
      controlsClear: i.bottom <= z.top && (i.top >= a.bottom || i.right <= a.left),
      focus: document.activeElement.getAttribute('aria-label'), modal: dialog.matches(':modal'), scale: dialog.querySelector('.image-viewer-percentage').textContent,
      top: i.top, left: i.left, width: i.width, height: i.height };
  })()`);
  const screenshot = async (name) => {
    if (!process.env.LINGXI_IMAGE_SCREENSHOTS) return;
    await run(`Promise.all(document.querySelector('.image-viewer').getAnimations().map(animation => animation.finished)).then(() => undefined)`);
    await mkdir(process.env.LINGXI_IMAGE_SCREENSHOTS, { recursive: true });
    await writeFile(`${process.env.LINGXI_IMAGE_SCREENSHOTS}/${name}.png`, (await wc.capturePage()).toPNG());
  };
  try {
    await window.loadURL(url);
    for (let n = 0; n < 160 && !await run('!!document.querySelector("#portrait .image-preview-trigger")'); n++) await delay(25);
    await open('portrait'); await delay(180);
    const desktop = await inspect(); await screenshot('portrait-desktop');
    for (let n = 0; n < 8; n++) await key('Tab');
    const focusTrapped = await run(`!!document.activeElement.closest('.image-viewer')`);
    await run(`document.querySelector('.image-viewer-zoom button:last-child').click()`); await delay(25);
    const buttonZoomIn = parseFloat((await inspect()).scale);
    await run(`document.querySelector('.image-viewer-zoom button:first-child').click()`); await delay(25);
    const buttonZoomOut = parseFloat((await inspect()).scale);
    const buttonsZoomed = buttonZoomIn > parseFloat(desktop.scale) && buttonZoomOut < buttonZoomIn;
    await run(`document.querySelector('.image-viewer-percentage').click()`); await delay(25);
    const doubleClick = () => run(`(() => { const image = document.querySelector('.image-viewer-image'), r = image.getBoundingClientRect(); image.dispatchEvent(new MouseEvent('dblclick', { bubbles: true, clientX: (r.left + r.right) / 2, clientY: (r.top + r.bottom) / 2 })); })()`);
    await doubleClick(); await delay(25);
    const doubleClickZoomed = (await inspect()).scale === '100%';
    await doubleClick(); await delay(25);
    await key('1'); const zoomed = await inspect();
    wc.sendInputEvent({ type: 'mouseDown', x: 720, y: 420, button: 'left', clickCount: 1 });
    wc.sendInputEvent({ type: 'mouseMove', x: 720, y: 300, movementY: -120, button: 'left' });
    wc.sendInputEvent({ type: 'mouseUp', x: 720, y: 300, button: 'left', clickCount: 1 });
    await delay(50); const afterPan = await inspect();
    const panned = afterPan.top < zoomed.top - 60;
    await run(`document.querySelector('.image-viewer-viewport').dispatchEvent(new WheelEvent('wheel', { bubbles: true, cancelable: true, clientX: 720, clientY: 420, deltaY: -100 }))`);
    await delay(50); const wheelZoomed = (await inspect()).width > afterPan.width;
    await key('0');
    const clickSave = async () => {
      await run(`document.querySelector('.image-viewer-actions button').click()`);
      for (let n = 0; n < 100 && await run(`document.querySelector('.image-viewer-actions button').disabled`); n++) await delay(25);
    };
    await clickSave();
    const downloaded = (await stat(process.env.LINGXI_IMAGE_DOWNLOAD)).size;
    const original = await run(`document.querySelector('.image-viewer-image').src.split(',')[1]`);
    const savedOriginal = (await readFile(process.env.LINGXI_IMAGE_DOWNLOAD)).equals(Buffer.from(original, 'base64'));
    saveMode = 'cancel'; await clickSave();
    const cancelled = !await run(`!!document.querySelector('.image-viewer [role="alert"]')`);
    saveMode = 'error'; await clickSave();
    const saveError = await run(`!!document.querySelector('.image-viewer [role="alert"]')`);
    await key('Escape');
    const focusRestored = await run(`!document.querySelector('.image-viewer') && document.activeElement.matches('#portrait .image-preview-trigger')`);
    const backgroundScrollRestored = await run(`document.body.style.overflow !== 'hidden'`);
    window.setSize(375, 812); await delay(100);
    await open('portrait'); const narrow = await inspect(); await screenshot('portrait-narrow');
    await run(`document.querySelector('.image-viewer-control[aria-label="Close preview (Esc)"]').click()`);
    const buttonClosed = await run(`!document.querySelector('.image-viewer')`);
    window.setSize(1440, 960); await delay(100);
    await open('landscape'); const landscape = await inspect(); await screenshot('landscape-desktop');
    await key('1'); window.setSize(768, 600); await delay(100); const resized = await inspect();
    wc.sendInputEvent({ type: 'mouseDown', x: 3, y: 300, button: 'left', clickCount: 1 });
    wc.sendInputEvent({ type: 'mouseUp', x: 3, y: 300, button: 'left', clickCount: 1 }); await delay(50);
    const backdropClosed = await run(`!document.querySelector('.image-viewer')`);
    await wc.debugger.attach('1.3');
    await wc.debugger.sendCommand('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-reduced-motion', value: 'reduce' }] });
    await open('portrait');
    const reducedMotion = await run(`getComputedStyle(document.querySelector('.image-viewer')).animationName === 'none'`);
    await key('Escape');
    await run(`document.querySelector('#broken .image-preview-trigger').click()`);
    for (let n = 0; n < 100 && !await run(`!!document.querySelector('.image-viewer [role="alert"]')`); n++) await delay(25);
    const errorShown = await run(`!!document.querySelector('.image-viewer [role="alert"]') && document.querySelector('.image-viewer-zoom button').disabled`);
    await key('Escape');
    const errorClosed = errorShown && await run(`!document.querySelector('.image-viewer')`);
    console.log(JSON.stringify({ desktop, zoomed, panned, wheelZoomed, buttonsZoomed, doubleClickZoomed, focusTrapped, focusRestored, narrow, landscape, resized, downloaded, cancelled, saveError, savedOriginal, backdropClosed, buttonClosed, errorClosed, reducedMotion, backgroundScrollRestored }));
  } finally { window.destroy(); }
}
main().then(() => app.exit(0), (error) => { console.error(error); app.exit(1); });
