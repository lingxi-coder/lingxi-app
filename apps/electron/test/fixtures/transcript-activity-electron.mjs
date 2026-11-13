import { app, BrowserWindow } from 'electron';
import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { configureFixtureWindow, waitForFixture } from './electron-test-environment.mjs';
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 1000, height: 800, webPreferences: { sandbox: true, backgroundThrottling: false } });
  try {
    await window.loadURL(process.argv.find(arg => arg.startsWith('http://')));
    await configureFixtureWindow(window);
    const run = source => window.webContents.executeJavaScript(source);
    await waitForFixture(window.webContents, 'Boolean(window.activityFixture)');
    const sample = `(() => { const stage = document.querySelector('.desktop-stage');
      const row = document.querySelector('.transcript-row[data-row-type="tool-group"]');
      const marker = [...document.querySelectorAll('.transcript-row[data-row-type="narration"]')].at(-1);
      window.activityRowProbe ??= row;
      return { height: stage.scrollHeight, top: stage.scrollTop, marker: marker.getBoundingClientRect().top,
        row: row?.getBoundingClientRect().height, sameNode: row === window.activityRowProbe,
        slot: Boolean(document.querySelector('[data-thinking-slot]')) }; })()`;
    const toolStart = (id, tool, verb) => ({ type: 'tool_use_started', id, tool, input_json: '{}',
      header: { verb, label: tool, title: `${tool} file`, primary: 'file' } });
    const result = [];
    for (const count of [35, 180]) {
      await run(`window.activityFixture.reset(${count}); window.activityRowProbe = null`);
      await run('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
      const events = [
        { type: 'thinking_delta', thinking: 'Inspect layout' },
        { type: 'api_retry', message: 'Rate limited. '.repeat(30), attempt: 2, max_retries: 5, delay_ms: 30000 },
        toolStart('read', 'Read', 'read'),
        { type: 'tool_use_result', id: 'read', tool: 'Read', is_error: false, result_json: '{}', display: { headline: 'Read file', body: 'Contents', body_lines: 1 } },
        { type: 'thinking_delta', thinking: 'Check another tool' },
        toolStart('shell', 'Bash', 'shell'),
        { type: 'tool_use_result', id: 'shell', tool: 'Bash', is_error: true, result_json: '{}', display: { headline: 'Failed', body: 'Retained error', body_lines: 1 } },
        { type: 'thinking_delta', thinking: 'A very long thought with new lines.\n'.repeat(100) },
        { type: 'turn_ended', outcome: { type: 'completed' } },
      ];
      const samples = [];
      for (const event of events) {
        await run(`window.activityFixture.event(${JSON.stringify(event)})`);
        if (event.type === 'api_retry') {
          assert.equal(await run(`document.querySelector('.transcript-api-retry-trigger').textContent.trim()`), 'Reconnecting 2/5', 'a retry updates the existing activity row with the current recovery header');
          assert.ok(await run(`document.querySelector('.transcript-api-retry-message').textContent.includes('Rate limited.')`), 'expanded recovery keeps the provider failure inspectable');
          assert.ok(await run(`document.querySelector('.transcript-api-retry .transcript-activity-details').textContent.includes('Inspect layout')`), 'recovery retains the activity details');
          await run(`document.querySelector('.transcript-api-retry-trigger').click()`);
        }
        samples.push(await run(sample));
        await run('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
        samples.push(await run(sample));
      }
      const base = samples[0];
      for (const current of samples) {
        assert.ok(Math.abs(current.height - base.height) <= 1, `closed activity transitions preserve the pixel-rounded scroll extent: ${JSON.stringify(samples)}`);
        assert.ok(Math.abs(current.marker - base.marker) <= 1, `reading position moved: ${JSON.stringify({ base, current })}`);
        assert.equal(current.sameNode, true, 'thinking, retry and tools retain the exact outer row node');
        assert.ok(Math.abs(current.row - 28) <= 0.5, 'a closed activity has one compact header, including a half-pixel recovery border');
        assert.equal(current.slot, false, 'no independent thinking spacer remains');
      }
      await run(`document.querySelector('.transcript-activity-trigger').click()`);
      assert.equal(await run(`document.querySelectorAll('.transcript-activity-details .transcript-tool-row').length`), 2);
      assert.match(await run(`document.querySelector('.transcript-activity-details').textContent`), /Inspect layout/);
      result.push({ count, samples: samples.length, height: base.height, maximumShift: Math.max(...samples.map(value => Math.abs(value.marker - base.marker))) });
    }
    for (const dark of [false, true]) {
      for (const width of [1000, 440]) {
        window.setSize(width, 800);
        await run(`window.activityFixture.skill(${dark})`);
        await run('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
        const toggle = '.local-app-instructions-toggle';
        assert.equal(await run(`document.querySelector('${toggle}').getAttribute('aria-expanded')`), 'false');
        assert.equal(await run(`document.querySelectorAll('.user-message-bubble').length`), 0);
        assert.equal(await run(`document.querySelector('.local-app-instructions-body').getBoundingClientRect().height`), 0);
        assert.equal(await run(`document.querySelector('.local-app-instructions').scrollWidth <= document.querySelector('.local-app-instructions').clientWidth`), true);
        assert.equal(await run(`document.querySelector('.local-app-instructions').getBoundingClientRect().right <= window.innerWidth`), true);
        await run(`document.querySelector('${toggle}').click()`);
        assert.equal(await run(`document.querySelector('${toggle}').getAttribute('aria-expanded')`), 'true');
        const text = await run(`document.querySelector('.local-app-instructions-body').innerText`);
        assert.match(text, /Choose the execution host and interfaces/);
        assert.doesNotMatch(text, /Base directory|Resolve relative references/);
        await run(`document.querySelector('${toggle}').click()`);
        if (process.env.LINGXI_SKILL_EVIDENCE) {
          mkdirSync(process.env.LINGXI_SKILL_EVIDENCE, { recursive: true });
          writeFileSync(join(process.env.LINGXI_SKILL_EVIDENCE, `skill-${dark ? 'dark' : 'light'}-${width}.png`), (await window.webContents.capturePage()).toPNG());
        }
      }
    }
    console.log(JSON.stringify(result));
  } finally { window.destroy(); }
}
main().then(() => app.quit()).catch(error => { console.error(error); app.exit(1); });
