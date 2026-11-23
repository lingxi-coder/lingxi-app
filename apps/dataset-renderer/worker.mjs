import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import { setTimeout as delay } from 'node:timers/promises';

const origin = 'https://page.lingxi.invalid';
const documentURL = origin + '/';
const browser = spawn('/usr/bin/chromium', [
  '--headless', '--no-sandbox', '--remote-debugging-pipe', '--user-data-dir=/tmp/viewer',
  '--no-first-run', '--disable-background-networking', '--disable-component-update', '--disable-sync', '--disable-gpu', 'about:blank',
], { stdio: ['ignore', 'ignore', 'ignore', 'pipe', 'pipe'] });
const pending = new Map();
const responses = new Map();
let sequence = 0;
let chunks = '';
let sessionId;
let bundle;
let initialState;
let dimensions;
const actions = [];
let focusableNodes = new Set();
const keyboardCodes = new Map([
  ['Backspace', 8], ['Tab', 9], ['Enter', 13], ['Escape', 27], ['End', 35], ['Home', 36],
  ['ArrowLeft', 37], ['ArrowUp', 38], ['ArrowRight', 39], ['ArrowDown', 40], ['Delete', 46],
]);

function send(method, params = {}, session = sessionId) {
  return new Promise((resolve, reject) => {
    const id = ++sequence;
    const timer = setTimeout(() => { pending.delete(id); reject(new Error('Renderer timed out')); }, 5000);
    pending.set(id, { resolve, reject, timer });
    browser.stdio[3].write(JSON.stringify({ id, method, params, sessionId: session }) + '\0', error => {
      if (!error) return;
      pending.delete(id);
      clearTimeout(timer);
      reject(error);
    });
  });
}

browser.stdio[4].setEncoding('utf8');
browser.stdio[4].on('data', bytes => {
  const messages = (chunks + bytes).split('\0');
  chunks = messages.pop();
  for (const message of messages) {
    const event = JSON.parse(message);
    const call = pending.get(event.id);
    if (call) {
      pending.delete(event.id);
      clearTimeout(call.timer);
      if (event.error) call.reject(new Error('Chromium rejected the renderer command'));
      else call.resolve(event.result);
    } else if (event.method === 'Fetch.requestPaused') {
      void fulfill(event.params).catch(() => { browser.kill(); });
    } else if (event.method === 'Runtime.bindingCalled' && event.params.name === '__lingxiRequest') {
      acceptAction(event.params);
    }
  }
});
browser.on('exit', () => {
  for (const reply of pending.values()) { clearTimeout(reply.timer); reply.reject(new Error('Chromium exited')); }
  pending.clear();
});

async function fulfill({ requestId, request, resourceType }) {
  const { url, method } = request;
  let payload;
  if (method === 'GET' && url === documentURL && resourceType === 'Document') {
    payload = { mime: 'text/html; charset=utf-8', data: Buffer.from(bundle.html).toString('base64') };
  } else if (method === 'GET' && url.startsWith(origin + '/assets/')) {
    const name = new URL(url).pathname.split('/').at(-1);
    if (Object.hasOwn(bundle.assets, name)) payload = bundle.assets[name];
  }
  if (!payload) return send('Fetch.failRequest', { requestId, errorReason: 'BlockedByClient' });
  return send('Fetch.fulfillRequest', { requestId, responseCode: 200, body: payload.data,
    responseHeaders: [
      { name: 'Content-Type', value: payload.mime },
      { name: 'X-Content-Type-Options', value: 'nosniff' },
      ...(resourceType === 'Document' ? [{ name: 'Content-Security-Policy', value: "default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'none'; worker-src 'none'; frame-src 'none'; form-action 'none'; base-uri 'none'; object-src 'none'" }] : []),
    ] });
}

function acceptAction({ payload, executionContextId }) {
  if (payload.length > 32_768 || actions.length >= 16 || responses.size >= 16) return;
  let request;
  try { request = JSON.parse(payload); } catch { return; }
  if (!request || typeof request.id !== 'string' || !/^[A-Za-z0-9_-]{1,64}$/u.test(request.id) || responses.has(request.id)) return;
  if (!bundle) return;
  if (request.kind === 'state') {
    if (typeof request.state !== 'string' || Buffer.byteLength(request.state) > 16_384 ||
        !Number.isSafeInteger(request.expected) || request.expected < 0) return;
    responses.set(request.id, executionContextId);
    actions.push({ kind: 'state', id: request.id, state: request.state, expected: request.expected });
    return;
  }
  if (!Object.hasOwn(bundle.actions, request.alias)) return;
  responses.set(request.id, executionContextId);
  actions.push({ id: request.id, alias: request.alias, parameters: request.parameters, operation: request.operation });
}

const scriptLiteral = value => JSON.stringify(value).replaceAll('<', '\\u003c');

async function mount(request) {
  if (bundle) throw new Error('Renderer already mounted');
  bundle = request.bundle;
  initialState = request.state;
  dimensions = request.dimensions;
  const { targetInfos } = await send('Target.getTargets');
  const target = targetInfos.find(target => target.type === 'page' && target.url === 'about:blank');
  const { targetId } = target ?? await send('Target.createTarget', { url: 'about:blank' });
  await send('Browser.setDownloadBehavior', { behavior: 'deny' });
  ({ sessionId } = await send('Target.attachToTarget', { targetId, flatten: true }));
  await send('Page.enable');
  await send('Page.bringToFront');
  await send('Accessibility.enable');
  await send('Runtime.enable');
  await send('Runtime.addBinding', { name: '__lingxiRequest' });
  await send('Emulation.setDeviceMetricsOverride', { width: dimensions.width, height: dimensions.height, deviceScaleFactor: 1, mobile: false });
  await send('Page.addScriptToEvaluateOnNewDocument', { source: `(() => {
    const send = globalThis.__lingxiRequest;
    const requests = new Map();
    Reflect.deleteProperty(globalThis, '__lingxiRequest');
    let state = ${scriptLiteral(initialState)};
    let version = ${scriptLiteral(request.stateVersion ?? 0)};
    function submit(intent) {
      return new Promise((resolve, reject) => {
        const id = crypto.randomUUID();
        const timer = setTimeout(() => { requests.delete(id); reject(new Error('Dataset action timed out')); }, 35000);
        requests.set(id, { resolve, reject, timer });
        try { send(JSON.stringify({ ...intent, id })); }
        catch (error) { clearTimeout(timer); requests.delete(id); reject(error); }
      });
    }
    Object.defineProperty(globalThis, 'lingxi', { value: Object.freeze({
      get state() { return structuredClone(state); },
      async saveState(next) {
        const serialized = JSON.stringify(next);
        if (typeof serialized !== 'string' || new TextEncoder().encode(serialized).length > 16384) throw new Error('Invalid page state');
        const result = await submit({ kind: 'state', state: serialized, expected: version });
        state = JSON.parse(serialized);
        version = result.version;
        return version;
      },
      invoke(alias, parameters, operation) {
        return submit({ alias, parameters, operation });
      }
    }) });
    Object.defineProperty(globalThis, '__lingxiResult', { value(id, result, error) {
      const request = requests.get(id);
      if (!request) return;
      clearTimeout(request.timer);
      requests.delete(id);
      if (error) request.reject(new Error(error));
      else request.resolve(result);
    } });
  })()` });
  await send('Fetch.enable', { patterns: [{ urlPattern: '*' }] });
  const navigation = await send('Page.navigate', { url: documentURL });
  if (navigation.errorText) throw new Error('Could not open the Dataset page');
  for (let attempt = 0; attempt < 40; attempt++) {
    const { result } = await send('Runtime.evaluate', {
      expression: `location.href === ${scriptLiteral(documentURL)} && document.readyState !== 'loading'`,
      returnByValue: true,
    });
    if (result.value === true) return;
    await delay(25);
  }
  throw new Error('Dataset page did not finish loading');
}

function coordinates(event) {
  if (!Number.isFinite(event.x) || !Number.isFinite(event.y) || event.x < 0 || event.y < 0 ||
      event.x >= dimensions.width || event.y >= dimensions.height) throw new Error('Invalid input coordinates');
  return { x: event.x, y: event.y };
}

async function input(event) {
  switch (event.kind) {
    case 'tap': {
      const point = coordinates(event);
      await send('Input.dispatchMouseEvent', { type: 'mousePressed', ...point, button: 'left', clickCount: 1 });
      await send('Input.dispatchMouseEvent', { type: 'mouseReleased', ...point, button: 'left', clickCount: 1 });
      break;
    }
    case 'select': {
      const start = coordinates(event.start), end = coordinates(event.end);
      await send('Runtime.evaluate', { expression: `((start, end) => {
        const selection = getSelection();
        selection.removeAllRanges();
        const locate = ({ x, y }) => {
          const element = document.elementFromPoint(x, y);
          if (!element || getComputedStyle(element).userSelect === 'none') return null;
          return document.caretPositionFromPoint(x, y);
        };
        const anchor = locate(start), focus = locate(end);
        if (anchor && focus) selection.setBaseAndExtent(anchor.offsetNode, anchor.offset, focus.offsetNode, focus.offset);
      })(${JSON.stringify(start)}, ${JSON.stringify(end)})` });
      break;
    }
    case 'selection': {
      const { result } = await send('Runtime.evaluate', { returnByValue: true, expression: `(() => {
        const field = document.activeElement;
        if (field instanceof HTMLInputElement && field.type === 'password') return '';
        const selected = String(getSelection()).slice(0, 8192);
        if (selected) return selected;
        if ((field instanceof HTMLInputElement || field instanceof HTMLTextAreaElement) && field.selectionStart !== null) {
          return field.value.slice(field.selectionStart, Math.min(field.selectionEnd, field.selectionStart + 8192));
        }
        return '';
      })()` });
      return typeof result.value === 'string' ? result.value.slice(0, 8192) : '';
    }
    case 'scroll':
      if (!Number.isFinite(event.dy) || Math.abs(event.dy) > 2000) throw new Error('Invalid scroll');
      await send('Input.dispatchMouseEvent', { type: 'mouseWheel', ...coordinates(event), deltaX: 0, deltaY: event.dy });
      break;
    case 'text':
      if (typeof event.text !== 'string' || event.text.length > 8192) throw new Error('Invalid text');
      await send('Input.insertText', { text: event.text });
      break;
    case 'composition':
      if (typeof event.text !== 'string' || event.text.length > 1024) throw new Error('Invalid composition');
      await send('Input.imeSetComposition', { text: event.text, selectionStart: event.text.length, selectionEnd: event.text.length });
      break;
    case 'key': {
      const windowsVirtualKeyCode = keyboardCodes.get(event.key);
      if (windowsVirtualKeyCode === undefined) throw new Error('Unsupported key');
      const key = { key: event.key, code: event.key, windowsVirtualKeyCode, modifiers: event.shift ? 8 : 0 };
      const down = event.key === 'Enter' ? { type: 'keyDown', text: '\r', unmodifiedText: '\r' } : { type: 'rawKeyDown' };
      await send('Input.dispatchKeyEvent', { ...key, ...down });
      await send('Input.dispatchKeyEvent', { ...key, type: 'keyUp' });
      break;
    }
    case 'focusNode':
      if (!focusableNodes.has(event.node)) throw new Error('Unknown accessible element');
      await send('DOM.focus', { backendNodeId: event.node });
      break;
    default: throw new Error('Unsupported input');
  }
}

async function frame(captureOnly) {
  const [{ data }, { nodes }] = await Promise.all([
    send('Page.captureScreenshot', { format: 'png', fromSurface: true, captureBeyondViewport: false }),
    send('Accessibility.getFullAXTree'),
  ]);
  if (data.length > 4 * 1024 * 1024) throw new Error('Dataset page frame exceeds the limit');
  focusableNodes = new Set();
  const accessibility = nodes.filter(node => !node.ignored).slice(0, 256).map(node => {
    const target = Number.isSafeInteger(node.backendDOMNodeId) &&
      node.properties?.some(property => property.name === 'focusable' && property.value?.value === true) ?
      node.backendDOMNodeId : undefined;
    if (target) focusableNodes.add(target);
    return { id: node.nodeId, parent: node.parentId, role: node.role?.value, name: String(node.name?.value ?? '').slice(0, 256), target };
  });
  return { png: data, accessibility, actions: captureOnly ? [] : actions.splice(0) };
}

const requests = createInterface({ input: process.stdin });
try {
  for await (const line of requests) {
    try {
      const request = JSON.parse(line);
      if (request.action === 'close') break;
      if (request.action === 'mount') await mount(request);
      if (!bundle) throw new Error('Renderer not mounted');
      if (request.action === 'frame' && Array.isArray(request.deferredActions)) {
        actions.unshift(...request.deferredActions);
      }
      const selectedText = request.action === 'input' ? await input(request.event) : undefined;
      if (request.action === 'result') {
        const contextId = responses.get(request.id);
        if (contextId !== undefined) {
          responses.delete(request.id);
          await send('Runtime.evaluate', { contextId, expression: `globalThis.__lingxiResult(${scriptLiteral(request.id)},${scriptLiteral(request.result ?? null)},${scriptLiteral(request.error ?? null)})` });
        }
      }
      const scene = request.actionsOnly || request.deferFrame && actions.length
        ? { actions: actions.splice(0) }
        : await frame(request.captureOnly === true);
      console.log(JSON.stringify({ ...scene, selectedText }));
    } catch { console.log(JSON.stringify({ error: 'RENDER_UNAVAILABLE' })); }
  }
} finally {
  browser.kill('SIGTERM');
}
