/** Run via apps/electron/node_modules/.bin/electron. No runtime WebView required. */
const { app, BrowserWindow } = require('electron');
const { readFileSync, mkdirSync, writeFileSync, mkdtempSync, rmSync } = require('node:fs');
const { resolve, join } = require('node:path');
const { tmpdir } = require('node:os');
const root = resolve(__dirname, '../../..');
const scratch = mkdtempSync(join(tmpdir(), 'lingxi-avatar-export-'));
app.setPath('userData', scratch);
async function main() {
  await app.whenReady();
  const win = new BrowserWindow({ show: false, webPreferences: { sandbox: true } });
  try {
    await win.loadURL('about:blank');
    for (let index = 0; index < 28; index++) {
      const suffix = String(index).padStart(2, '0');
      const imageSet = join(root, 'apps/ios/native/Resources/Assets.xcassets', `AgentAvatar${suffix}.imageset`);
      mkdirSync(imageSet, { recursive: true });
      for (const theme of ['light', 'dark']) {
        const svg = readFileSync(join(root, 'apps/electron/src/renderer/assets/agent-avatars', `variant-${suffix}-${theme}.svg`), 'utf8');
        const uri = 'data:image/svg+xml;base64,' + Buffer.from(svg).toString('base64');
        const png = await win.webContents.executeJavaScript(`(async () => {
          const image = new Image(); image.src = ${JSON.stringify(uri)}; await image.decode();
          const canvas = document.createElement('canvas'); canvas.width = canvas.height = 96;
          canvas.getContext('2d').drawImage(image, 0, 0, 96, 96); return canvas.toDataURL('image/png').split(',')[1];
        })()`);
        const bytes = Buffer.from(png, 'base64');
        const androidDir = join(root, 'apps/android/native/app/src/main/res/drawable-nodpi');
        mkdirSync(androidDir, { recursive: true });
        writeFileSync(join(androidDir, `agent_avatar_${suffix}_${theme}.png`), bytes);
        writeFileSync(join(imageSet, `${theme}.png`), bytes);
      }
      writeFileSync(join(imageSet, 'Contents.json'), JSON.stringify({
        images: [{ filename: 'light.png', idiom: 'universal' }, { filename: 'dark.png', idiom: 'universal', appearances: [{ appearance: 'luminosity', value: 'dark' }] }],
        info: { author: 'xcode', version: 1 }, properties: { 'template-rendering-intent': 'original' },
      }, null, 2) + '\n');
    }
    console.log('Exported 28 original avatar pairs for each native client at 96px.');
  } finally { win.destroy(); app.quit(); }
}
main().catch(error => { console.error(error); app.exit(1); }).finally(() => rmSync(scratch, { recursive: true, force: true }));
