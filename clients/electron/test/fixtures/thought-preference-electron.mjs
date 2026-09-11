import assert from 'node:assert/strict';
import { app, BrowserWindow } from 'electron';
import { mkdir, writeFile } from 'node:fs/promises';
import { dirname, extname } from 'node:path';
const url = process.argv.find((arg) => arg.startsWith('http://'));
if (!url || !process.env.LINGXI_TEST_USER_DATA) throw new Error('isolated fixture required');
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
async function main() {
  await app.whenReady();
  const win = new BrowserWindow({ show: false, width: 900, height: 600, webPreferences: { sandbox: true } });
  const evaluate = (code) => win.webContents.executeJavaScript(code);
  const wait = () => new Promise((resolve) => setTimeout(resolve, 100));
  const state = () => evaluate(`({
    disclosure: Boolean(document.querySelector('.desktop-stage [aria-expanded]')),
    legacyToggle: Boolean(document.querySelector('[role="switch"]')),
    thinking: Boolean(document.querySelector('[data-run-type="thinking"]')),
    animated: Boolean(document.querySelector('[data-run-type="thinking"] .running-sweep')),
    text: document.querySelector('.desktop-stage').textContent
  })`);
  const assertAgentRows = async () => {
    const rows = await evaluate(`Array.from(document.querySelectorAll('.transcript-agent-row')).map(row => {
      const { top, bottom, left, right } = row.getBoundingClientRect();
      return { top, bottom, left, right };
    })`);
    assert.equal(rows.length, 2);
    assert.ok(rows[1].top >= rows[0].bottom, 'agents are separate vertically stacked rows');
    assert.equal(rows[0].left, rows[1].left);
    assert.equal(rows[0].right, rows[1].right);
    assert.ok(await evaluate(`Array.from(document.querySelectorAll('.transcript-agent-row')).every(row => row.scrollWidth <= row.clientWidth)`), 'agent rows do not overflow');
  };
  try {
    await win.loadURL(url);
    for (let n = 0; n < 80 && !await evaluate('Boolean(window.thoughtFixture)'); n++) await wait();
    await assertAgentRows();
    await evaluate(`document.querySelector('[data-agent-id="explorer"]').click()`); await wait();
    assert.equal(await evaluate(`document.querySelector('[data-opened-agent]').dataset.openedAgent`), 'explorer');
    await evaluate(`document.querySelector('[data-agent-id="reviewer"]').click()`); await wait();
    assert.equal(await evaluate(`document.querySelector('[data-opened-agent]').dataset.openedAgent`), 'reviewer');
    const live = await state();
    assert.equal(live.disclosure, false);
    assert.doesNotMatch(live.text, /Stage.tsx/);
    assert.match(live.text, /npm run typecheck/);
    assert.equal(live.legacyToggle, false);
    assert.equal(live.thinking, true);
    assert.equal(live.animated, true);
    assert.doesNotMatch(live.text, /Streaming reasoning/);
    assert.ok(live.text.indexOf('Checking the project.') < live.text.indexOf('Thinking'));
    assert.ok(live.text.indexOf('Thinking') < live.text.indexOf('Following message.'));
    await evaluate('window.thoughtFixture.changeLegacyPreference()'); await wait();
    assert.equal((await state()).thinking, true);
    assert.doesNotMatch((await state()).text, /Streaming reasoning/);
    await evaluate('window.thoughtFixture.finishTools()'); await wait();
    assert.match((await state()).text, /npm run typecheck/);
    assert.doesNotMatch((await state()).text, /Used 2 tools/);
    assert.doesNotMatch((await state()).text, /Stage.tsx/);
    await evaluate(`document.querySelector('.tool-group-trigger').click()`); await wait();
    assert.match((await state()).text, /Stage.tsx/);
    assert.match((await state()).text, /npm run typecheck/);
    await evaluate('window.thoughtFixture.finish()'); await wait();
    assert.equal((await state()).thinking, false);
    assert.doesNotMatch((await state()).text, /Finished reasoning/);
    await evaluate('window.thoughtFixture.restart()'); await wait();
    assert.equal((await state()).thinking, true);
    await evaluate('window.thoughtFixture.stop()'); await wait();
    assert.equal((await state()).thinking, false);
    await evaluate('window.thoughtFixture.restart()'); await wait();
    await evaluate('window.thoughtFixture.switchSession()'); await wait();
    assert.equal((await state()).thinking, false, 'history never renders unfinished reasoning as active');
    assert.equal(await evaluate(`document.querySelector('.tool-group-trigger').getAttribute('aria-expanded')`), 'false', 'new session resets tool group expansion');
    await evaluate('window.thoughtFixture.restart()'); await wait();
    await evaluate('window.thoughtFixture.waitWithoutReasoning()'); await wait();
    assert.equal((await state()).thinking, true, 'waiting indicator works without reasoning deltas');
    assert.doesNotMatch((await state()).text, /Finished reasoning/);
    const screenshotBase = process.env.LINGXI_THOUGHT_SCREENSHOT;
    const capture = async (suffix) => {
      if (!screenshotBase) return;
      const extension = extname(screenshotBase) || '.png';
      const stem = extname(screenshotBase) ? screenshotBase.slice(0, -extension.length) : screenshotBase;
      const path = suffix ? `${stem}-${suffix}${extension}` : screenshotBase;
      await mkdir(dirname(path), { recursive: true });
      await writeFile(path, (await win.webContents.capturePage()).toPNG());
    };
    await capture('');
    await capture('light');
    await evaluate(`document.querySelector('[data-appearance-option="dark"]').click()`); await wait();
    assert.equal(await evaluate(`document.querySelector('[data-appearance-option="dark"]').getAttribute('aria-pressed')`), 'true');
    await assertAgentRows();
    await capture('dark');
    win.setSize(440, 650); await wait();
    await assertAgentRows();
    await capture('narrow');
    process.stdout.write(JSON.stringify({ passed: true }) + '\n');
  } finally { win.destroy(); app.quit(); }
}
main().catch((error) => { console.error(error); app.exit(1); });
