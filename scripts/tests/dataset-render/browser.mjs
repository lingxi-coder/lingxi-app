import assert from 'node:assert/strict';
import { execFileSync, spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, readFileSync } from 'node:fs';
import { createInterface } from 'node:readline';
import { setTimeout as delay } from 'node:timers/promises';

const observer = '172.29.76.2';
const nonce = process.argv[2];
assert.match(nonce, /^[a-f0-9]{12}$/u);
const mode = process.argv[3];
assert.ok(['control', 'isolated', 'timing', 'active'].includes(mode));
const origin = 'https://' + observer + ':18443';
const documentUrl = origin + '/app-' + nonce;
const documentBytes = readFileSync(new URL('./fixture.html', import.meta.url));

class Chromium {
  pending = new Map();
  events = new Map();
  sequence = 0;
  constructor() {
    this.process = spawn('/usr/bin/chromium', [
      '--headless', '--no-sandbox', '--remote-debugging-pipe', '--user-data-dir=/tmp/viewer',
      '--no-first-run', '--disable-background-networking', '--disable-component-update',
      '--disable-sync', '--disable-gpu', '--window-size=480,640', 'about:blank',
    ], { stdio: ['ignore', 'ignore', 'inherit', 'pipe', 'pipe'] });
    let rest = '';
    this.process.stdio[4].setEncoding('utf8');
    this.process.stdio[4].on('data', part => {
      const messages = (rest + part).split('\0');
      rest = messages.pop();
      for (const message of messages) {
        const payload = JSON.parse(message);
        const call = this.pending.get(payload.id);
        if (call) {
          this.pending.delete(payload.id);
          if (payload.error) call.reject(new Error(call.method + ': ' + payload.error.message));
          else call.resolve(payload.result);
        } else {
          this.events.get(payload.method)?.(payload);
        }
      }
    });
    this.process.on('exit', () => {
      for (const call of this.pending.values()) call.reject(new Error('Chromium exited during ' + call.method));
      this.pending.clear();
    });
  }
  send(method, params = {}, sessionId) {
    return new Promise((resolve, reject) => {
      const id = ++this.sequence;
      this.pending.set(id, { method, resolve, reject });
      this.process.stdio[3].write(JSON.stringify({ id, method, params, sessionId }) + '\0');
    });
  }
}

async function run() {
  const nss = '/tmp/viewer-home/.pki/nssdb';
  mkdirSync(nss, { recursive: true });
  execFileSync('certutil', ['-N', '-d', 'sql:' + nss, '--empty-password']);
  execFileSync('certutil', ['-A', '-d', 'sql:' + nss, '-n', 'Viewer fixture', '-t', 'C,,', '-i', '/opt/lingxi/fixture-ca.crt']);
  const started = performance.now();
  const browser = new Chromium();
  try {
    const version = await browser.send('Browser.getVersion');
    const { targetId } = await browser.send('Target.createTarget', { url: 'about:blank' });
    const { sessionId } = await browser.send('Target.attachToTarget', { targetId, flatten: true });
    const cdp = (method, params = {}) => browser.send(method, params, sessionId);
    async function evaluate(expression) {
      const reply = await cdp('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
      if (reply.exceptionDetails) throw new Error(reply.exceptionDetails.exception?.description ?? reply.exceptionDetails.text);
      return reply.result.value;
    }
    async function frame(includeBytes = false) {
      const { data } = await cdp('Page.captureScreenshot', { format: 'png' });
      const png = Buffer.from(data, 'base64');
      assert.equal(png.readUInt32BE(0), 0x89504e47);
      const digest = createHash('sha256').update(png).digest('hex');
      return includeBytes ? { png: data, sha256: digest } : digest;
    }
    await cdp('Page.enable');
    await cdp('Accessibility.enable');
    await cdp('Network.enable');
    const failures = [];
    browser.events.set('Network.loadingFailed', ({ params }) => {
      if (params.type === 'Fetch') failures.push(params.corsErrorStatus?.corsError ?? params.blockedReason ?? params.errorText);
    });
    await cdp('Emulation.setDeviceMetricsOverride', { width: 480, height: 640, deviceScaleFactor: 1, mobile: true });
    await cdp('Emulation.setTouchEmulationEnabled', { enabled: true, maxTouchPoints: 1 });
    if (mode !== 'control') {
      browser.events.set('Fetch.requestPaused', event => {
        assert.equal(event.params.request.url, documentUrl);
        void cdp('Fetch.fulfillRequest', {
          requestId: event.params.requestId, responseCode: 200,
          responseHeaders: [{ name: 'Content-Type', value: 'text/html; charset=utf-8' }],
          body: documentBytes.toString('base64'),
        });
      });
      await cdp('Fetch.enable', { patterns: [{ urlPattern: documentUrl, resourceType: 'Document' }] });
    }
    const mountTime = performance.now();
    await cdp('Page.navigate', { url: documentUrl });
    for (let i = 0; i < 100 && !await evaluate('typeof observeNetwork === "function"'); i++) await delay(20);
    assert.equal(await evaluate('document.querySelector("#qty").value'), '7');
    const originalFrame = await frame();
    console.log(JSON.stringify({ phase: 'frame', ms: Math.round(performance.now() - started),
      warmMountToFrameMs: Math.round(performance.now() - mountTime), version: version.product,
      htmlSha256: createHash('sha256').update(documentBytes).digest('hex') }));
    const { nodes } = await cdp('Accessibility.getFullAXTree');
    const accessible = (role, name) => nodes.some(n => n.role?.value === role && n.name?.value === name);
    assert.ok(accessible('button', '加一') && accessible('textbox', '备注'));
    const point = await evaluate('(() => {const r=document.querySelector("#add").getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}})()');
    const inputAt = performance.now();
    await cdp('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [point] });
    await cdp('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
    assert.equal(await evaluate('document.querySelector("#qty").value'), '8');
    const changedFrame = await frame();
    assert.notEqual(originalFrame, changedFrame);
    const inputFrameMs = Math.round(performance.now() - inputAt);
    if (mode === 'timing') {
      console.log(JSON.stringify({ phase: 'result', inputFrameMs }));
      return;
    }
    if (mode === 'active') {
      console.log(JSON.stringify({ phase: 'interactive' }));
      let n = 0;
      for await (const line of createInterface({ input: process.stdin })) {
        const action = JSON.parse(line);
        if (action.type === 'close') { assert.equal(n, 120); break; }
        assert.ok(action.type === 'tap' && action.n === n + 1 && n < 120);
        n++;
        await cdp('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [point] });
        await cdp('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
        const quantity = Number(await evaluate('document.querySelector("#qty").value'));
        assert.equal(quantity, 8 + n);
        console.log(JSON.stringify({ phase: 'screen', n, quantity, ...await frame(true) }));
      }
      assert.equal(n, 120);
      console.log(JSON.stringify({ phase: 'result', inputs: n, finalQty: 8 + n }));
      return;
    }
    await evaluate('document.querySelector("#note").focus()');
    await cdp('Input.imeSetComposition', { text: '订单', selectionStart: 2, selectionEnd: 2 });
    await cdp('Input.insertText', { text: '订单' });
    assert.equal(await evaluate('document.querySelector("#note").value'), '订单');
    const compositions = await evaluate('compositions.map(e => e[0])');
    assert.ok(compositions.includes('compositionstart') && compositions.includes('compositionend'));
    await cdp('Input.dispatchKeyEvent', { type: 'keyDown', key: 'a', code: 'KeyA', modifiers: 2, commands: ['selectAll'] });
    await cdp('Input.dispatchKeyEvent', { type: 'keyUp', key: 'a', code: 'KeyA', modifiers: 2 });
    assert.deepEqual(await evaluate('[document.activeElement.selectionStart,document.activeElement.selectionEnd]'), [0, 2]);
    const viewport = await evaluate('({width:innerWidth,height:innerHeight,scrollHeight:document.scrollingElement.scrollHeight})');
    assert.ok(viewport.scrollHeight > viewport.height, JSON.stringify(viewport));
    const x = Math.floor(viewport.width / 2);
    const startY = Math.floor(viewport.height * 0.75);
    await cdp('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x, y: startY }] });
    for (let i = 1; i <= 8; i++) {
      await delay(20);
      await cdp('Input.dispatchTouchEvent', { type: 'touchMove', touchPoints: [{ x, y: startY - 32 * i }] });
    }
    await cdp('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
    for (let i = 0; i < 40 && await evaluate('scrollY') === 0; i++) await delay(25);
    const scrollY = await evaluate('scrollY');
    assert.ok(scrollY > 0, 'Touch swipe left the viewport at ' + scrollY);
    const network = await evaluate('observeNetwork()');
    const plainUrl = 'http://' + observer + ':18080/wire-' + nonce;
    await cdp('Runtime.evaluate', { expression: 'location.assign(' + JSON.stringify(plainUrl) + ')', userGesture: true });
    await delay(1200);
    console.log(JSON.stringify({ phase: 'result', version: version.product, originalFrame, changedFrame, inputFrameMs,
      accessibility: true, composedText: '订单', selectedChars: 2, touchScroll: true, scrollY, networkErrors: failures, ...network }));
  } finally {
    browser.process.kill('SIGTERM');
  }
}

const timeout = setTimeout(() => { console.error('Renderer fixture timed out'); process.exit(2); }, mode === 'active' ? 630_000 : 75_000);
try { await run(); } finally { clearTimeout(timeout); }
