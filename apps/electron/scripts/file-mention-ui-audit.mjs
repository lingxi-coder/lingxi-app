#!/usr/bin/env node

import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';

const port = Number.parseInt(process.argv[2] ?? '', 10);
const outputDir = resolve(process.argv[3] ?? 'dist/file-mention-ui-audit');
const auditQuery = process.argv[4] ?? 'RTK';
assert(Number.isSafeInteger(port) && port > 0, 'usage: file-mention-ui-audit.mjs <debug-port> [output-dir]');

const delay = (milliseconds) => new Promise((resolvePromise) => setTimeout(resolvePromise, milliseconds));

async function waitFor(check, label, timeoutMs = 20_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const value = await check();
      if (value) return value;
    } catch {}
    await delay(100);
  }
  throw new Error(`${label} did not become ready`);
}

class CdpConnection {
  #nextId = 1;
  #pending = new Map();

  constructor(url) {
    this.socket = new WebSocket(url);
    this.ready = new Promise((resolvePromise, rejectPromise) => {
      this.socket.addEventListener('open', resolvePromise, { once: true });
      this.socket.addEventListener('error', () => rejectPromise(new Error(`failed to connect to ${url}`)), { once: true });
    });
    this.socket.addEventListener('message', (event) => {
      const payload = JSON.parse(String(event.data));
      if (!payload.id) return;
      const pending = this.#pending.get(payload.id);
      if (!pending) return;
      this.#pending.delete(payload.id);
      if (payload.error) pending.reject(new Error(payload.error.message));
      else pending.resolve(payload.result ?? {});
    });
  }

  async send(method, params = {}) {
    await this.ready;
    const id = this.#nextId++;
    return await new Promise((resolvePromise, rejectPromise) => {
      this.#pending.set(id, { resolve: resolvePromise, reject: rejectPromise });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }

  close() { this.socket.close(); }
}

async function json(url) {
  const response = await fetch(url);
  assert(response.ok, `${url} returned ${response.status}`);
  return await response.json();
}

async function evaluate(page, expression) {
  const result = await page.send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
  if (result.exceptionDetails) {
    throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text ?? 'renderer evaluation failed');
  }
  return result.result?.value;
}

async function screenshot(page, name) {
  const result = await page.send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
  const path = resolve(outputDir, name);
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, Buffer.from(result.data, 'base64'));
  return path;
}

await waitFor(() => json(`http://127.0.0.1:${port}/json/version`), 'CDP endpoint');
const target = await waitFor(async () => {
  const targets = await json(`http://127.0.0.1:${port}/json/list`);
  return targets.find((entry) => entry.type === 'page' && entry.title.includes('Desktop'));
}, 'desktop page');

const page = new CdpConnection(target.webSocketDebuggerUrl);
await page.ready;
await Promise.all([page.send('Runtime.enable'), page.send('Page.enable')]);

try {
  await page.send('Emulation.setDeviceMetricsOverride', { width: 1180, height: 820, deviceScaleFactor: 1, mobile: false });
  const promptReady = () => evaluate(page, `document.querySelector('[role="textbox"][aria-label="Prompt"]')?.getAttribute('contenteditable') === 'true'`);
  const alreadyReady = await waitFor(promptReady, 'existing prompt composer', 2_000).catch(() => false);
  if (!alreadyReady) {
    await evaluate(page, `(() => {
      const provider = [...document.querySelectorAll('[role="radiogroup"][aria-label="LLM providers"] button')]
        .find((button) => button.innerText.includes('DeepSeek'));
      provider?.click();
    })()`);
    await waitFor(
      () => evaluate(page, `Boolean(document.querySelector('input[aria-label="DeepSeek API key"]'))`),
      'DeepSeek setup input',
    );
    await evaluate(page, `(() => {
      const input = document.querySelector('input[aria-label="DeepSeek API key"]');
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
      setter.call(input, 'ui-audit-only');
      input.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText', data: 'ui-audit-only' }));
    })()`);
    await waitFor(
      () => evaluate(page, `document.querySelector('input[aria-label="DeepSeek API key"]')?.closest('div')?.querySelector('button:not(:disabled)')?.innerText`),
      'enabled setup connect button',
    );
    await evaluate(page, `document.querySelector('input[aria-label="DeepSeek API key"]')?.closest('div')?.querySelector('button:not(:disabled)')?.click()`);
  }
  await waitFor(
    promptReady,
    'enabled prompt composer',
    60_000,
  );
  await evaluate(page, `(() => {
    const style = document.createElement('style');
    style.id = 'file-mention-ui-audit-stability';
    style.textContent = '* { animation: none !important; transition: none !important; }';
    document.querySelector('#file-mention-ui-audit-stability')?.remove();
    document.head.append(style);
    const main = document.querySelector('main');
    const rgb = getComputedStyle(main).backgroundColor.match(/\d+/g)?.map(Number) ?? [255, 255, 255];
    if ((rgb[0] + rgb[1] + rgb[2]) / 3 < 128) document.querySelector('button[aria-label="Toggle theme"]')?.click();
    const editor = document.querySelector('[role="textbox"][aria-label="Prompt"]');
    editor.replaceChildren();
    editor.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'deleteContentBackward' }));
  })()`);

  const openPicker = async () => {
    await evaluate(page, `document.querySelector('button[aria-label="Add context"]')?.click()`);
    await waitFor(() => evaluate(page, `Boolean(document.querySelector('#mention-results'))`), 'context menu');
    await evaluate(page, `[...document.querySelectorAll('#mention-results [role="option"]')].find((row) => row.textContent.includes('Files and folders'))?.click()`);
    await waitFor(
      () => evaluate(page, `Boolean(document.querySelector('[role="dialog"][aria-label="Search workspace files"] input[aria-label="File search query"]'))`),
      'file search dialog',
    );
  };
  const setSearchQuery = async (value) => {
    await evaluate(page, `(() => {
      const input = document.querySelector('input[aria-label="File search query"]');
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
      setter.call(input, ${JSON.stringify(value)});
      input.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText', data: ${JSON.stringify(value)} }));
    })()`);
    await waitFor(
      () => evaluate(page, `document.querySelector('input[aria-label="File search query"]')?.value === ${JSON.stringify(value)}`),
      `file search query ${JSON.stringify(value)}`,
    );
  };

  await openPicker();
  const autoFocused = await evaluate(page, `document.activeElement?.matches('input[aria-label="File search query"]') === true`);
  assert.equal(autoFocused, true, 'plus button must focus the file search input');
  await setSearchQuery(auditQuery);
  await waitFor(
    () => evaluate(page, `[...document.querySelectorAll('[role="listbox"][aria-label="Workspace files"] [role="option"]')].some((row) => row.innerText.toLocaleLowerCase().includes(${JSON.stringify(auditQuery.toLocaleLowerCase())}))`),
    'filtered workspace file results',
  );
  const filtered = await evaluate(page, `(() => ({
    query: document.querySelector('input[aria-label="File search query"]')?.value,
    options: [...document.querySelectorAll('[role="listbox"][aria-label="Workspace files"] [role="option"]')].map((row) => row.innerText),
  }))()`);
  assert.equal(filtered.query, auditQuery);
  assert(filtered.options.some((text) => text.toLocaleLowerCase().includes(auditQuery.toLocaleLowerCase())), `typed file query must filter to ${auditQuery}`);

  await evaluate(page, `document.querySelector('button[aria-label="Clear mention search"]')?.click()`);
  await waitFor(
    () => evaluate(page, `document.querySelector('input[aria-label="File search query"]')?.value === '' && document.querySelectorAll('[role="listbox"][aria-label="Workspace files"] [role="option"]').length > 1`),
    'cleared file search',
  );
  const initialSelectedIndex = await evaluate(page, `[...document.querySelectorAll('[role="listbox"][aria-label="Workspace files"] [role="option"]')].findIndex((row) => row.getAttribute('aria-selected') === 'true')`);
  assert(initialSelectedIndex >= 0, 'file results must expose one active keyboard option');
  await evaluate(page, `document.querySelector('input[aria-label="File search query"]')?.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }))`);
  await waitFor(
    () => evaluate(page, `[...document.querySelectorAll('[role="listbox"][aria-label="Workspace files"] [role="option"]')].findIndex((row) => row.getAttribute('aria-selected') === 'true') !== ${initialSelectedIndex}`),
    'ArrowDown file selection',
  );
  await evaluate(page, `document.querySelector('input[aria-label="File search query"]')?.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowUp', bubbles: true }))`);
  await waitFor(
    () => evaluate(page, `[...document.querySelectorAll('[role="listbox"][aria-label="Workspace files"] [role="option"]')].findIndex((row) => row.getAttribute('aria-selected') === 'true') === ${initialSelectedIndex}`),
    'ArrowUp file selection',
  );
  await setSearchQuery(auditQuery);
  await waitFor(
    () => evaluate(page, `[...document.querySelectorAll('[role="listbox"][aria-label="Workspace files"] [role="option"]')].some((row) => row.innerText.toLocaleLowerCase().includes(${JSON.stringify(auditQuery.toLocaleLowerCase())}))`),
    'restored filtered results',
  );
  await delay(150);
  const menuScreenshot = await screenshot(page, 'file-mention-menu.png');
  const menu = await evaluate(page, `(() => {
    const list = document.querySelector('[role="listbox"][aria-label="Workspace files"]');
    const picker = document.querySelector('[role="dialog"][aria-label="Search workspace files"]');
    const composer = document.querySelector('.beta-composer');
    const rect = (element) => { const value = element.getBoundingClientRect(); return { x: value.x, y: value.y, width: value.width, height: value.height, bottom: value.bottom }; };
    return {
      viewport: { width: innerWidth, height: innerHeight },
      picker: rect(picker),
      composer: rect(composer),
      query: document.querySelector('input[aria-label="File search query"]')?.value,
      selectedCount: [...list.querySelectorAll('[role="option"]')].filter((row) => row.getAttribute('aria-selected') === 'true').length,
      options: [...list.querySelectorAll('[role="option"]')].slice(0, 5).map((row) => ({
        selected: row.getAttribute('aria-selected'),
        text: row.innerText,
      })),
    };
  })()`);
  assert(menu.picker.x >= 0 && menu.picker.x + menu.picker.width <= menu.viewport.width, 'file menu must stay inside the viewport');
  assert(menu.picker.bottom <= menu.composer.y - 8, 'file menu must sit completely above the input view with a visible gap');
  assert.equal(menu.selectedCount, 1, 'file results must expose one active keyboard option');
  assert.equal(menu.query, auditQuery);

  await evaluate(page, `document.querySelector('input[aria-label="File search query"]')?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }))`);
  const token = await waitFor(
    () => evaluate(page, `(() => {
      const editor = document.querySelector('[role="textbox"][aria-label="Prompt"]');
      const mentions = [...editor.querySelectorAll('[data-file-mention]')];
      const bodyText = [...editor.childNodes]
        .filter((node) => !(node instanceof HTMLElement && node.matches('[data-file-mention]')))
        .map((node) => node.textContent ?? '')
        .join('')
        .replaceAll('\u200b', '');
      return mentions.length ? {
        count: mentions.length,
        label: mentions[0].innerText,
        path: mentions[0].getAttribute('data-file-mention'),
        bodyText,
        menuClosed: !document.querySelector('[role="listbox"][aria-label="Workspace files"]'),
      } : null;
    })()`),
    'selected rich file token',
  );
  assert.equal(token.count, 1, 'a selected file must render exactly one rich token');
  assert.equal(token.bodyText, '', 'a selected file must not leave duplicate @path text in the editor');
  assert(!token.label.includes('/'), 'the rich token must show the compact file name, not the full path');
  assert.equal(token.menuClosed, true);
  const tokenScreenshot = await screenshot(page, 'file-mention-token.png');

  await evaluate(page, `(() => {
    const editor = document.querySelector('[role="textbox"][aria-label="Prompt"]');
    editor.replaceChildren();
    editor.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'deleteContentBackward' }));
  })()`);
  await openPicker();
  await evaluate(page, `document.querySelector('button[aria-label="Close mentions"]')?.click()`);
  const closeButton = await waitFor(
    () => evaluate(page, `!document.querySelector('[role="dialog"][aria-label="Search workspace files"]')`),
    'close button dismissal',
  );

  await openPicker();
  await evaluate(page, `document.querySelector('header')?.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }))`);
  const outsideClick = await waitFor(
    () => evaluate(page, `!document.querySelector('[role="dialog"][aria-label="Search workspace files"]')`),
    'outside click dismissal',
  );

  await openPicker();
  await evaluate(page, `document.querySelector('input[aria-label="File search query"]')?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))`);
  const escape = await waitFor(
    () => evaluate(page, `!document.querySelector('[role="dialog"][aria-label="Search workspace files"]')`),
    'Escape dismissal',
  );

  await evaluate(page, `(() => {
    const editor = document.querySelector('[role="textbox"][aria-label="Prompt"]');
    editor.textContent = '@tech';
    editor.focus();
    const range = document.createRange();
    range.setStart(editor.firstChild, editor.firstChild.nodeValue.length);
    range.collapse(true);
    const selection = getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
    editor.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText', data: '@tech' }));
  })()`);
  const mentionQuery = await waitFor(
    () => evaluate(page, `document.querySelector('input[aria-label="Search mentions"]')?.value === 'tech' ? 'tech' : ''`),
    'direct @ mention query synchronization',
  );
  await waitFor(
    () => evaluate(page, `document.querySelectorAll('#mention-results [role="option"]').length > 0`),
    'direct @ mention search results',
  );
  await evaluate(page, `document.querySelector('[role="textbox"][aria-label="Prompt"]')?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }))`);
  const promotedMention = await waitFor(
    () => evaluate(page, `(() => {
      const editor = document.querySelector('[role="textbox"][aria-label="Prompt"]');
      const mentions = [...editor.querySelectorAll('[data-file-mention]')];
      const bodyText = [...editor.childNodes]
        .filter((node) => !(node instanceof HTMLElement && node.matches('[data-file-mention]')))
        .map((node) => node.textContent ?? '')
        .join('')
        .replaceAll('\u200b', '');
      return mentions.length ? { count: mentions.length, label: mentions[0].innerText, bodyText } : null;
    })()`),
    'direct @ mention promoted to rich text',
  );
  assert.equal(promotedMention.count, 1, 'direct @ selection must render one token');
  assert.equal(promotedMention.bodyText, '', 'direct @ selection must replace the query instead of duplicating it');
  assert(!promotedMention.label.includes('@'), 'the rich token must not render raw @path text');

  await evaluate(page, `(() => {
    const editor = document.querySelector('[role="textbox"][aria-label="Prompt"]');
    editor.replaceChildren();
    editor.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'deleteContentBackward' }));
  })()`);

  process.stdout.write(`${JSON.stringify({ menuScreenshot, tokenScreenshot, menu, token, close: { closeButton, outsideClick, escape }, mentionQuery, promotedMention }, null, 2)}\n`);
} finally {
  page.close();
}
