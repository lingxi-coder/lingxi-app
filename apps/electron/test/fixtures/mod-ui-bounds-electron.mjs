import assert from 'node:assert/strict';
import { app, BrowserWindow } from 'electron';
import { setTimeout as delay } from 'node:timers/promises';

app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 900, height: 650, webPreferences: { contextIsolation: true, sandbox: true, backgroundThrottling: false } });
  try {
    await window.loadURL(process.argv[2]);
    const run = script => window.webContents.executeJavaScript(script);
    const wait = async condition => {
      const deadline = Date.now() + 5000;
      while (Date.now() < deadline) {
        if (await run(condition)) return;
        await delay(20);
      }
      throw new Error(`Timed out: ${condition}`);
    };
    await wait(`document.querySelector('#large')?.textContent === 'Mod: small' && document.querySelector('#weather')?.textContent === 'Weather sibling'`);
    const before = await run('window.boundsFixture.snapshot()');
    const ownedRevision = before.renders.filter(r => r.instance === 'large-row').at(-1).revision;
    await run(`window.boundsFixture.set('x'.repeat(100001))`);
    await wait(`document.querySelector('#native-output')?.textContent.length === 100001 && window.boundsFixture.snapshot().unmounts.includes(${ownedRevision})`);
    assert.equal(await run(`Boolean(document.querySelector('#large .mod-ui-parent-site'))`), false);
    const skipped = await run('window.boundsFixture.snapshot()');
    assert.equal(skipped.renders.filter(r => r.instance === 'large-row').length, before.renders.filter(r => r.instance === 'large-row').length);
    await run('window.boundsFixture.invalidate(); window.dispatchEvent(new Event("resize"))');
    // Flush the two animation-frame turns used by resize observation.
    await run('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
    const afterInvalidation = await run('window.boundsFixture.snapshot()');
    assert.equal(afterInvalidation.renders.filter(r => r.instance === 'large-row').length, before.renders.filter(r => r.instance === 'large-row').length);
    assert.equal(await run(`document.querySelector('#weather').textContent`), 'Weather sibling');
    await run(`window.boundsFixture.set('recovered')`);
    await wait(`document.querySelector('#large')?.textContent === 'Mod: recovered'`);
    const final = await run('window.boundsFixture.snapshot()');
    assert.ok(final.renders.every(r => r.intact));
    assert.equal(final.renders.filter(r => r.instance === 'large-row').length, before.renders.filter(r => r.instance === 'large-row').length + 1);
    process.stdout.write(JSON.stringify({ passed: true, fullNativeOutputLength: 100001, exactRevisionReleased: ownedRevision, noOversizedOrRetryIPC: true, recoveredWithoutTruncation: true, weatherUnaffected: true }) + '\n');
  } finally { window.destroy(); app.quit(); }
}
main().catch(error => { process.stderr.write(String(error.stack || error) + '\n'); app.exit(1); });
