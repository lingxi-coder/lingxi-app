import { app, BrowserWindow } from 'electron';
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
async function main() {
await app.whenReady();
const window = new BrowserWindow({ show: false, webPreferences: { sandbox: true } });
try {
  await window.loadURL(process.argv.find(value => value.startsWith('http://')));
  for (let i = 0; i < 200; i++) {
    if (await window.webContents.executeJavaScript('!!window.runtimeLifecycleFixture')) break;
    await new Promise(resolve => setTimeout(resolve, 20));
  }
  const result = await window.webContents.executeJavaScript('window.runtimeLifecycleFixture.run()');
  // Flush the assertion result before force-exiting this fixture. It has no
  // application shutdown work, and macOS helper teardown can otherwise hang.
  await new Promise(resolve => process.stdout.write(`${JSON.stringify(result)}\n`, resolve));
  process.exit(0);
} catch (error) {
  await new Promise(resolve => process.stderr.write(`${String(error)}\n`, resolve));
  process.exit(1);
}
}
void main().catch(error => { console.error(error); app.exit(1); });
