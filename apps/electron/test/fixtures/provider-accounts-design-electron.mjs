import { app, BrowserWindow } from 'electron';
import { mkdir } from 'node:fs/promises';
import { join } from 'node:path';
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
app.commandLine.appendSwitch('disable-gpu');
async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 1024, height: 900, webPreferences: { sandbox: true, backgroundThrottling: false } });
  try {
    window.webContents.on('console-message', event => { if (event.level === 'error') process.stderr.write(`renderer: ${event.message}\n`); });
    await window.loadURL(process.argv.find(value => value.startsWith('http://')));
    const js = source => window.webContents.executeJavaScript(source);
    const deadline = Date.now() + 10000;
    while (!await js('window.accountDesign?.ready()')) { if (Date.now() > deadline) throw new Error('Fixture not ready'); await new Promise(resolve => setTimeout(resolve, 20)); }
    if (process.env.LINGXI_DESIGN_SCREENSHOTS) {
      const { writeFile } = await import('node:fs/promises');
      const directory = process.env.LINGXI_DESIGN_SCREENSHOTS; await mkdir(directory, { recursive: true });
      const capture = async name => { await new Promise(resolve => setTimeout(resolve, 100)); await writeFile(join(directory, `${name}.png`), (await window.webContents.capturePage()).toPNG()); };
      await capture('accounts-light'); await js('window.accountDesign.show(true)'); await capture('accounts-dark');
      window.setSize(660, 900); await capture('accounts-narrow'); window.setSize(1024, 900);
      await js('window.accountDesign.show(false)'); await js('window.accountDesign.open()'); await capture('add-account');
      await js("document.querySelector('.pa-inline-form footer button[type=button]').click()");
      await js('window.accountDesign.show(false, true)'); await capture('providers');
      await js('window.accountDesign.show(false)');
    }
    const result = await js('window.accountDesign.run()');
    if (process.env.LINGXI_DESIGN_SCREENSHOTS) {
      const { writeFile } = await import('node:fs/promises');
      await writeFile(join(process.env.LINGXI_DESIGN_SCREENSHOTS, 'first-account.png'), (await window.webContents.capturePage()).toPNG());
    }
    process.stdout.write(`${JSON.stringify(result)}\n`);
  } finally { window.destroy(); }
}
main().then(() => app.quit(), error => { process.stderr.write(`${error.stack}\n`); app.exit(1); });
