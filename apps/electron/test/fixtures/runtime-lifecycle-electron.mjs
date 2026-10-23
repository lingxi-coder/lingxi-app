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
  console.log(JSON.stringify(await window.webContents.executeJavaScript('window.runtimeLifecycleFixture.run()')));
} catch (error) { console.error(error); process.exitCode = 1; }
finally { window.destroy(); app.quit(); }
}
void main().catch(error => { console.error(error); app.exit(1); });
