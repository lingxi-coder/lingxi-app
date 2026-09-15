import { app, BrowserWindow } from 'electron';
import { mkdir, writeFile } from 'node:fs/promises';
const url = process.argv.find((argument) => argument.startsWith('http://'));
if (!url || !process.env.LINGXI_TEST_USER_DATA) throw new Error('fixture URL and isolated user data required');
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 1000, height: 800, webPreferences: { sandbox: true, backgroundThrottling: false } });
  try {
    await window.loadURL(url);
    const run = (script) => window.webContents.executeJavaScript(script);
    const settle = () => delay(100);
    const deadline = Date.now() + 6000;
    while (!await run('Boolean(window.stageScrollFixture && document.querySelector(".transcript-thinking"))')) {
      if (Date.now() > deadline) throw new Error('Stage fixture did not render');
      await delay(20);
    }
    await settle();
    const metrics = `(() => { const node = document.querySelector('.desktop-stage'); return { top: node.scrollTop, gap: node.scrollHeight - node.clientHeight - node.scrollTop, ancestor: document.querySelector('#scroll-ancestor').scrollTop }; })()`;
    const initial = await run(metrics);
    const result = { initialGap: initial.gap, ancestorScroll: initial.ancestor };
    await run(`(() => { const node = document.querySelector('.desktop-stage'); node.dispatchEvent(new PointerEvent('pointerdown', {bubbles:true})); node.scrollTop = node.scrollHeight - node.clientHeight - 40; })()`);
    await settle();
    result.awayBefore = (await run(metrics)).top;
    await run('window.stageScrollFixture.insert()');
    await settle();
    result.dragAfter = (await run(metrics)).top;
    await run("window.dispatchEvent(new PointerEvent('pointerup', {bubbles:true}))");
    await run('window.stageScrollFixture.insert()');
    await settle();
    result.awayAfter = (await run(metrics)).top;
    await run(`(() => { const node = document.querySelector('.desktop-stage'); node.dispatchEvent(new PointerEvent('pointerdown', {bubbles:true})); node.scrollTop = node.scrollHeight; })()`);
    await settle();
    await run("window.dispatchEvent(new PointerEvent('pointerup', {bubbles:true}))");
    result.streamingGaps = [];
    result.immediateStreamingGaps = [];
    for (let i = 0; i < 5; i++) {
      result.immediateStreamingGaps.push(await run(`(() => { window.stageScrollFixture.insert(); return ${metrics}.gap; })()`));
      await settle();
      result.streamingGaps.push((await run(metrics)).gap);
    }
    // Local child layout changes do not update Stage props (e.g. late markdown/media).
    await run(`document.querySelector('.transcript-thinking').style.height = '150px'`);
    await settle();
    result.resizeGap = (await run(metrics)).gap;
    await run(`document.querySelector('.transcript-thinking').style.height = '20px'`);
    await settle();
    result.shrinkGap = (await run(metrics)).gap;
    result.finalAncestorScroll = (await run(metrics)).ancestor;
    await run(`(() => { const style = document.createElement('style'); style.textContent = '* { animation: none !important; transition: none !important; }'; document.head.append(style); })()`);
    result.deliveryLayouts = [];
    for (const long of [false, true]) {
      for (const status of ['pending', undefined, 'failed', undefined]) {
        await run(`window.stageScrollFixture.delivery(${JSON.stringify(status)}, ${long})`);
        await settle();
        result.deliveryLayouts.push(await run(`(() => {
          const bubble = document.querySelector('.user-message-bubble');
          const rect = bubble.getBoundingClientRect();
          return { width: rect.width, height: rect.height, top: rect.top, nextTop: document.getElementById('narration-content-after-delivery').getBoundingClientRect().top, scrollHeight: document.querySelector('.desktop-stage').scrollHeight };
        })()`));
        if (!long && status === 'pending' && process.env.LINGXI_DELIVERY_SCREENSHOTS) {
          await mkdir(process.env.LINGXI_DELIVERY_SCREENSHOTS, { recursive: true });
          await writeFile(process.env.LINGXI_DELIVERY_SCREENSHOTS + '/pending.png', (await window.webContents.capturePage()).toPNG());
        }
      }
    }
    process.stdout.write(`${JSON.stringify(result)}\n`);
  } finally { window.destroy(); app.quit(); }
}
main().catch((error) => { process.stderr.write(`${error.stack ?? error}\n`); app.exit(1); });
