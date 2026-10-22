import { writeFile } from 'node:fs/promises';
import { configureFixtureWindow } from './electron-test-environment.mjs';
import { app, BrowserWindow } from 'electron';
import assert from 'node:assert/strict';
const url = process.argv.find(arg => arg.startsWith('http://'));
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
async function main() {
await app.whenReady();
const window = new BrowserWindow({show: false, width: 1100, height: 760, webPreferences: {sandbox: true, backgroundThrottling: false}});
try {
  await window.loadURL(url);
  await configureFixtureWindow(window);
  const wc = window.webContents;
  const js = expression => wc.executeJavaScript(expression);
  for (let i = 0; i < 100 && !await js(`!!document.querySelector('.activity-tool-group')`); i++) await new Promise(resolve => setTimeout(resolve, 50));
  const click = async selector => {
    const point = await js(`(() => { const el = document.querySelector(${JSON.stringify(selector)}); const r = el.getBoundingClientRect(); const x = Math.round(r.left+r.width/2), y = Math.round(r.top+r.height/2); if(!el.contains(document.elementFromPoint(x,y))) throw new Error('Unreachable control'); return {x,y}; })()`);
    wc.sendInputEvent({type: 'mouseDown', button: 'left', clickCount: 1, ...point});
    wc.sendInputEvent({type: 'mouseUp', button: 'left', clickCount: 1, ...point});
    await new Promise(resolve => setTimeout(resolve, 80));
  };
  assert.equal(await js(`document.querySelector('.activity-tool-group summary > span').textContent`), 'Read a file, ran a command');
  assert.equal(await js(`document.querySelector('.activity-tool-group summary .activity-tool-failed').textContent`), '1 failed');
  assert.equal(await js(`document.querySelector('.activity-tool-group').open`), false);
  await click('.activity-tool-group summary');
  assert.equal(await js(`document.querySelector('.activity-tool-group').open`), true);
  assert.equal(await js(`document.querySelectorAll('.activity-tool-details .transcript-tool-row').length`), 2, 'expansion retains both tool calls');
  assert.equal(await js(`document.querySelectorAll('.activity-tool-details .tool-group-trigger').length`), 0, 'expanded activity has no nested group heading');
  await click('.activity-tool-group summary');
  assert.equal(await js(`document.querySelector('.activity-tool-group').open`), false);
  assert.equal(await js(`document.querySelector('.session-needs-input').textContent`), 'Needs input');
  await click('#input');
  assert.equal(await js(`!!document.querySelector('.session-needs-input')`), false);
  await click('#input');
  assert.equal(await js(`!!document.querySelector('.session-needs-input')`), true);
  await writeFile('/tmp/lingxi-activity-light.png', (await wc.capturePage()).toPNG());
  await click('#theme');
  await click('.activity-tool-group summary');
  assert.equal(await js(`document.querySelector('.activity-tool-group').open`), true);
  await click('#toggle');
  assert.equal(await js(`!!document.querySelector('.activity-view')`), false);
  assert.equal(await js(`!!document.querySelector('.activity-tool-group')`), false);
  await click('#toggle');
  assert.equal(await js(`!!document.querySelector('.activity-view')`), true);
  await writeFile('/tmp/lingxi-activity-dark.png', (await wc.capturePage()).toPNG());
  console.log(JSON.stringify({checks: 'summary, errors, pointer expand/collapse, dark theme, view toggle'}));
} catch(error) { console.error(error); process.exitCode = 1; }
finally { window.destroy(); app.quit(); }

}
main().catch(error => { console.error(error); app.exit(1); });
