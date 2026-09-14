import { app, BrowserWindow } from 'electron';
import { mkdirSync, writeFileSync } from 'node:fs';
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function main() {
  await app.whenReady();
  const win = new BrowserWindow({ show: false, width: 1200, height: 860, webPreferences: { sandbox: true } });
  try {
    await win.loadURL(process.argv.find(value => value.startsWith('http://')));
    const run = script => win.webContents.executeJavaScript(script);
    for (let i = 0; i < 100 && !await run('!!document.querySelector("button")'); i++) await delay(40);
    mkdirSync('/tmp/lingxi-plan-ui', { recursive: true });
    await run('document.querySelector("main [aria-label=\\"Copy plan\\"]").click()'); await delay(80);
    const copiedWithoutOpening = await run('!document.querySelector("aside") && window.copiedPlan===window.planFixture.markdown && document.querySelector("main").innerText.includes("Copied")');
    await run('window.lingxi.copyText=async()=>{throw new Error("clipboard unavailable")};document.querySelector("main [aria-label=\\"Copy plan\\"]").click()'); await delay(80);
    const copyFailure = await run('document.querySelector("main").innerText.includes("Copy failed") && !document.querySelector("aside")');
    await run('window.lingxi.copyText=async(text)=>{window.copiedPlan=text};document.querySelector("[aria-label=\\"Open full plan\\"]").click()'); await delay(150);
    const opened = await run('!!document.querySelector("aside article h1") && document.querySelector("aside").innerText.includes("默认范围")');
    await run('document.querySelector("aside [aria-label=\\"Copy plan\\"]").click()'); await delay(80);
    const copied = await run('window.copiedPlan===window.planFixture.markdown');
    writeFileSync('/tmp/lingxi-plan-ui/light.png', (await win.webContents.capturePage()).toPNG());
    await run('window.planFixture.theme(true)'); await delay(100);
    writeFileSync('/tmp/lingxi-plan-ui/dark.png', (await win.webContents.capturePage()).toPNG());
    await run('window.planFixture.theme(false);window.planFixture.scenario("history")'); await delay(150);
    const historyCard = await run('!!document.querySelector("main [aria-label=\\"Copy plan\\"]") && !!document.querySelector("main [aria-label=\\"Open full plan\\"]") && !document.querySelector("main").innerText.includes("Show more")');
    writeFileSync('/tmp/lingxi-plan-ui/history.png', (await win.webContents.capturePage()).toPNG());
    await run('window.planFixture.scenario("restored")'); await delay(150);
    const restoredContent = await run('document.querySelector("main").innerText.includes("环境切换") && !document.querySelector("main").innerText.includes("Preparing plan")');
    await run('document.querySelector("main [aria-label=\\"Copy plan\\"]").click()'); await delay(80);
    const restoredCopy = await run('window.copiedPlan===window.planFixture.markdown');
    writeFileSync('/tmp/lingxi-plan-ui/restored.png', (await win.webContents.capturePage()).toPNG());
    console.log(JSON.stringify({ opened, copied, copiedWithoutOpening, copyFailure, historyCard, restoredContent, restoredCopy }));
    win.destroy(); app.exit(0);
  } catch (error) { console.error(error); win.destroy(); app.exit(1); }
}
void main();
