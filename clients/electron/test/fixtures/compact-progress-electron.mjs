import { app, BrowserWindow } from 'electron';
import { mkdir, writeFile } from 'node:fs/promises';
import { dirname } from 'node:path';

const url = process.argv.find((argument) => argument.startsWith('http://'));
if (!url) throw new Error('fixture URL is required');
if (!process.env.LINGXI_TEST_USER_DATA) throw new Error('isolated test user data is required');
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);

const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));
async function waitFor(webContents, expression) {
  const deadline = Date.now() + 8_000;
  while (Date.now() < deadline) {
    if (await webContents.executeJavaScript(expression)) return;
    await delay(25);
  }
  throw new Error(`timed out waiting for: ${expression}`);
}

async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 900, height: 260, webPreferences: { sandbox: true } });
  try {
    await window.loadURL(url);
    const { webContents } = window;
    await waitFor(webContents, 'Boolean(window.compactFixture)');
    const dispatch = (event) => webContents.executeJavaScript(`window.compactFixture.dispatch(${JSON.stringify(event)})`);
    const snapshot = () => webContents.executeJavaScript(`(() => {
      const row = document.querySelector('.compact-status');
      const progress = row?.querySelector('[role="progressbar"]');
      const state = window.compactFixture.state();
      return {
        rowCount: document.querySelectorAll('.compact-status').length,
        text: row?.textContent ?? '',
        title: row?.querySelector('.compact-status-title')?.textContent ?? '',
        progressCount: document.querySelectorAll('[role="progressbar"]').length,
        valueNow: progress?.getAttribute('aria-valuenow') ?? null,
        elapsed: row?.querySelector('.compact-status-elapsed')?.getAttribute('aria-label') ?? null,
        id: state.items.find((item) => item.type === 'compaction')?.id,
        item: state.items.find((item) => item.type === 'compaction'),
        summaries: state.summaries,
      };
    })()`);
    const results = {};
    for (const phase of ['preparing', 'summarizing', 'restoring']) {
      await dispatch({ type: 'compaction_status', phase });
      await waitFor(webContents, `Boolean(document.querySelector('.compact-status'))`);
      results[phase] = await snapshot();
    }
    await webContents.executeJavaScript('window.compactFixture.ageAndRemount()');
    await waitFor(webContents, `document.querySelector('[role="progressbar"]')?.getAttribute('aria-valuenow') === '99'`);
    await delay(100);
    if (process.env.LINGXI_COMPACT_SCREENSHOT) {
      const image = await webContents.capturePage();
      await mkdir(dirname(process.env.LINGXI_COMPACT_SCREENSHOT), { recursive: true });
      await writeFile(process.env.LINGXI_COMPACT_SCREENSHOT, image.toPNG());
    }

    await waitFor(webContents, `document.querySelector('.compact-status-elapsed')?.getAttribute('aria-label')?.match(/9[0-9] seconds/)`);
    results.aged = await snapshot();
    await webContents.executeJavaScript('window.compactFixture.remount()');
    results.remounted = await snapshot();

    await dispatch({ type: 'compaction_status', phase: 'complete' });
    results.complete = await snapshot();
    await dispatch({ type: 'compaction_completed', messages_before: 42, messages_after: 7, bytes_saved: 38_912, summary: 'Preserved the actual conversation context.' });
    results.summary = await snapshot();

    for (const phase of ['cancelled', 'error', 'skipped']) {
      await webContents.executeJavaScript('window.compactFixture.reset()');
      await dispatch({ type: 'compaction_status', phase: 'preparing' });
      const started = await snapshot();
      await dispatch({ type: 'compaction_status', phase, ...(phase === 'error' ? { error: 'Summarizer disconnected' } : {}) });
      results[phase] = { ...await snapshot(), startedId: started.id };
    }
    process.stdout.write(`${JSON.stringify(results)}\n`);
  } finally {
    if (!window.isDestroyed()) window.destroy();
    app.quit();
  }
}

main().catch((error) => {
  process.stderr.write(`${error instanceof Error ? error.stack : String(error)}\n`);
  app.exit(1);
});
