import assert from 'node:assert/strict';
import { readFile, writeFile, mkdtemp, rm } from 'node:fs/promises';
import { createServer } from 'node:http';
import { pathToFileURL } from 'node:url';
import { join } from 'node:path';
import { tmpdir } from 'node:os';

// Use an installed Playwright, or the desktop workspace's bundled runtime.
const { chromium, webkit } = await import(process.env.PLAYWRIGHT_MODULE_PATH
  ? pathToFileURL(process.env.PLAYWRIGHT_MODULE_PATH).href : 'playwright');
const html = await readFile(new URL('../../resources/mermaid/mermaid.html', import.meta.url));
assert.ok(html.subarray(0, 1024).toString().includes('<meta charset="utf-8">'));
const source = await readFile(new URL('fixtures/remote-access.mmd', import.meta.url), 'utf8');
const temporary = await mkdtemp(join(tmpdir(), 'lingxi-mermaid-browser-'));
const localParent = join(temporary, 'parent.html');
const localRenderer = new URL('../../resources/mermaid/mermaid.html', import.meta.url).href;
await writeFile(localParent, `<meta http-equiv="Content-Security-Policy" content="default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'none'"><iframe title="Mermaid" sandbox="allow-scripts" src="${localRenderer}" style="width:100%;height:640px;border:0"></iframe>`);
const server = createServer((request, response) => {
  if (request.url === '/mermaid.html') {
    response.setHeader('Content-Type', 'text/html');
    response.end(html);
  } else {
    // Match the desktop renderer's restrictive parent script policy.
    response.setHeader('Content-Security-Policy', "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'none'");
    response.setHeader('Content-Type', 'text/html');
    response.end('<iframe title="Mermaid" sandbox="allow-scripts" src="/mermaid.html" style="width:100%;height:640px;border:0"></iframe>');
  }
});
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
const origin = `http://127.0.0.1:${server.address().port}`;
try {
  for (const [name, engine] of [['chromium', chromium], ['webkit', webkit]]) {
    const executablePath = process.env[`MERMAID_${name.toUpperCase()}_PATH`];
    const browser = await engine.launch({ headless: true, ...(executablePath ? { executablePath } : {}) });
    try {
      const page = await browser.newPage({ viewport: { width: 1200, height: 900 } });
      const external = [];
      page.on('request', (request) => { if (!request.url().startsWith(origin)) external.push(request.url()); });
      await page.goto(origin);
      await page.waitForLoadState('networkidle');
      const frame = page.frames().find((item) => item.url().endsWith('/mermaid.html'));
      assert.ok(frame);
      await frame.waitForFunction(() => typeof window.renderMermaid === 'function');
      const render = async (text, dark = false) => {
        await page.evaluate((input) => document.querySelector('iframe').contentWindow.postMessage({ channel: 'lingxi-mermaid', ...input }, '*'), { source: text, dark });
      };
      await render(source);
      await frame.waitForSelector('#diagram svg');
      assert.equal(await frame.locator('.node').count(), 7);
      assert.equal(await frame.locator('.flowchart-link').count(), 9);
      assert.equal(await frame.locator('.cluster').count(), 1);
      const labels = await frame.locator('svg').textContent();
      assert.ok(labels.includes('默认浏览器'));
      assert.ok(labels.includes('Desktop 主动建立出站隧道'));
      assert.ok(!labels.includes('<br'));
      assert.equal(await frame.locator('foreignObject').count(), 0);
      const width = await frame.locator('svg').evaluate((element) => element.getBoundingClientRect().width);
      await frame.locator('#in').click();
      assert.ok(await frame.locator('svg').evaluate((element) => element.getBoundingClientRect().width) > width);
      await frame.locator('#source').click();
      assert.equal(await frame.locator('#code').innerText(), source);
      await frame.locator('#source').click();
      await page.setViewportSize({ width: 390, height: 850 });
      await render(source, true);
      await frame.waitForFunction(() => document.documentElement.classList.contains('dark') && document.getElementById('error').hidden);
      await frame.waitForSelector('svg[id="mermaid-2"]');
      assert.ok(await frame.locator('svg').evaluate((element) => element.getBoundingClientRect().width) <= 390);
      const geometry = await frame.locator('svg').evaluate((element) => {
        const box = element.getBBox();
        const view = element.viewBox.baseVal;
        return { left: box.x - view.x, right: view.x + view.width - box.x - box.width,
          top: box.y - view.y, bottom: view.y + view.height - box.y - box.height };
      });
      assert.ok(Object.values(geometry).every((value) => value >= -1), `SVG labels must fit: ${JSON.stringify(geometry)}`);
      if (process.env.MERMAID_SCREENSHOT_DIR) {
        await page.screenshot({ path: `${process.env.MERMAID_SCREENSHOT_DIR}/${name}-mobile.png`, fullPage: true });
      }
      await render('flowchart TB\nA[broken');
      await frame.waitForSelector('#error:not([hidden])');
      assert.equal(await frame.locator('#code').innerText(), 'flowchart TB\nA[broken');
      await render('x'.repeat(50_001));
      await frame.waitForFunction(() => document.getElementById('code').textContent.length === 50_001 && !document.getElementById('error').hidden);
      const authored = '%%{init: {"securityLevel":"loose","htmlLabels":true,"themeCSS":"body{display:none}"}}%%\nflowchart LR\nA["<img src=https://example.invalid/track onerror=alert(1)>hello"] --> B\nclick B "https://example.invalid"';
      await render(authored);
      await frame.waitForSelector('svg[id="mermaid-5"]');
      assert.equal(await frame.locator('a, img, image, script[src], foreignObject').count(), 0);
      assert.ok(await frame.locator('body').isVisible());
      assert.deepEqual(external, []);
      await render('flowchart LR\nA --> B');
      await render('flowchart LR\nFINAL --> DONE');
      await frame.waitForFunction(() => document.querySelector('#diagram svg')?.textContent.includes('FINAL'));
      assert.equal(await frame.locator('#diagram svg').count(), 1);
      // Packaged Desktop uses file URLs, with the same parent CSP and sandbox.
      await page.goto(pathToFileURL(localParent).href);
      await page.waitForLoadState('networkidle');
      const localFrame = page.frames().find((item) => item.url() === localRenderer);
      assert.ok(localFrame);
      await localFrame.waitForFunction(() => typeof window.renderMermaid === 'function');
      await page.evaluate((source) => document.querySelector('iframe').contentWindow.postMessage({ channel: 'lingxi-mermaid', source, dark: false }, '*'), source);
      await localFrame.waitForSelector('#diagram svg');
      assert.equal(await localFrame.locator('.node').count(), 7);
      console.log(`${name}: sample, labels, edges, subgraph, zoom, source, theme, mobile width, invalid/oversized source, isolation, rapid updates and packaged file URLs passed`);
    } finally { await browser.close(); }
  }
} finally { server.close(); await rm(temporary, { recursive: true, force: true }); }
