import { app, BrowserWindow } from 'electron';
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
app.commandLine.appendSwitch('disable-gpu');
async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, webPreferences: { sandbox: true, backgroundThrottling: false } });
  try {
    await window.loadURL(process.argv.find(value => value.startsWith('http://')));
    const deadline = Date.now() + 10_000;
    while (!await window.webContents.executeJavaScript('window.catalogFixture?.ready()')) {
      if (Date.now() > deadline) throw new Error('Catalog fixture did not initialize');
      await new Promise(resolve => setTimeout(resolve, 20));
    }
    const result = await window.webContents.executeJavaScript('window.catalogFixture.run()');
    process.stdout.write(`${JSON.stringify(result)}\n`);
  } finally { window.destroy(); }
}
main().then(() => app.quit(), error => { process.stderr.write(`${error.stack}\n`); app.exit(1); });
