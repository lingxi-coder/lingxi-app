#!/usr/bin/env node

import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';

const port = Number.parseInt(process.argv[2] ?? '', 10);
const outputDir = resolve(process.argv[3] ?? 'dist/permission-ui-audit');
assert(Number.isSafeInteger(port) && port > 0, 'usage: permission-ui-audit.mjs <debug-port> [output-dir]');

const delay = (milliseconds) => new Promise((resolvePromise) => setTimeout(resolvePromise, milliseconds));

async function waitFor(check, label, timeoutMs = 20_000) {
  const deadline = Date.now() + timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    try {
      const value = await check();
      if (value) return value;
    } catch (error) {
      lastError = error;
    }
    await delay(100);
  }
  throw new Error(`${label} did not become ready${lastError ? `: ${lastError.message}` : ''}`);
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

  close() {
    this.socket.close();
  }
}

async function json(url) {
  const response = await fetch(url);
  assert(response.ok, `${url} returned ${response.status}`);
  return await response.json();
}

async function evaluate(page, expression) {
  const result = await page.send('Runtime.evaluate', {
    expression,
    awaitPromise: true,
    returnByValue: true,
  });
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
  await page.send('Emulation.setDeviceMetricsOverride', {
    width: 1180,
    height: 820,
    deviceScaleFactor: 1,
    mobile: false,
  });
  await delay(300);

  const selectorState = await evaluate(page, `(() => {
    const button = document.querySelector('button[aria-label^="Permission mode:"]');
    return {
      found: Boolean(button),
      disabled: button?.disabled,
      label: button?.getAttribute('aria-label'),
      bodyText: document.body.innerText.slice(0, 2_000),
    };
  })()`);
  process.stderr.write(`[permission-ui-audit] selector state ${JSON.stringify(selectorState)}\n`);

  await waitFor(
    () => evaluate(page, `(() => {
      const button = document.querySelector('button[aria-label^="Permission mode:"]');
      return Boolean(button && !button.disabled);
    })()`),
    'enabled permission selector',
  );

  await evaluate(page, `(() => {
    window.__permissionAuditEvents = [];
    window.__permissionAuditUnsubscribe?.();
    window.__permissionAuditUnsubscribe = window.lingxi.onEvent((event) => {
      if (event.type === 'permission_mode_changed') window.__permissionAuditEvents.push(event.mode);
    });
    const auditStyle = document.createElement('style');
    auditStyle.id = 'permission-ui-audit-stability';
    auditStyle.textContent = '* { animation: none !important; transition: none !important; }';
    document.querySelector('#permission-ui-audit-stability')?.remove();
    document.head.append(auditStyle);
    const rgb = getComputedStyle(document.querySelector('main')).backgroundColor.match(/\\d+/g)?.map(Number) ?? [255, 255, 255];
    if ((rgb[0] + rgb[1] + rgb[2]) / 3 < 128) document.querySelector('button[aria-label="Toggle theme"]')?.click();
  })()`);
  // CDP runs against a non-frontmost window, where Chromium throttles the
  // 150ms entrance animation. Capture the stable end state explicitly.
  await evaluate(page, `(() => {
    const menu = document.querySelector('[role="menu"][aria-label="Permission modes"]');
    menu?.getAnimations().forEach((animation) => animation.finish());
    menu?.style.setProperty('animation', 'none', 'important');
    menu?.style.setProperty('opacity', '1', 'important');
    menu?.style.setProperty('transform', 'none', 'important');
  })()`);

  const openMenu = () => evaluate(page, `(() => {
    const button = document.querySelector('button[aria-label^="Permission mode:"]');
    if (button?.getAttribute('aria-expanded') !== 'true') button?.click();
    return true;
  })()`);
  const selectMode = async (label) => {
    await openMenu();
    await waitFor(
      () => evaluate(page, `Boolean(document.querySelector('[role="menu"][aria-label="Permission modes"]'))`),
      'permission menu',
    );
    await evaluate(page, `(() => {
      const row = [...document.querySelectorAll('[role="menuitemradio"]')]
        .find((element) => element.querySelector('span > span')?.textContent === ${JSON.stringify(label)});
      if (!row) throw new Error(${JSON.stringify(`missing permission row ${label}`)});
      row.click();
    })()`);
    await waitFor(
      () => evaluate(page, `document.querySelector('button[aria-label^="Permission mode:"]')?.getAttribute('aria-label') === ${JSON.stringify(`Permission mode: ${label}`)}`),
      `${label} mode acknowledgement`,
    );
  };

  await selectMode('Ask for approval');
  await openMenu();
  await waitFor(
    () => evaluate(page, `Boolean(document.querySelector('[role="menu"][aria-label="Permission modes"]'))`),
    'permission menu',
  );
  await delay(250);

  const lightScreenshot = await screenshot(page, 'permission-menu-light.png');
  const light = await evaluate(page, `(() => {
    const menu = document.querySelector('[role="menu"][aria-label="Permission modes"]');
    const pill = document.querySelector('button[aria-label^="Permission mode:"]');
    const rect = (element) => {
      const value = element.getBoundingClientRect();
      return { x: value.x, y: value.y, width: value.width, height: value.height };
    };
    return {
      viewport: { width: innerWidth, height: innerHeight },
      pill: rect(pill),
      menu: rect(menu),
      menuOpacity: getComputedStyle(menu).opacity,
      heading: menu.querySelector('strong')?.textContent,
      rows: [...menu.querySelectorAll('[role="menuitemradio"]')].map((row) => ({
        label: row.querySelector('span > span')?.textContent,
        description: row.querySelector('span > span + span')?.textContent,
        checked: row.getAttribute('aria-checked'),
      })),
      appBackground: getComputedStyle(document.querySelector('main')).backgroundColor,
    };
  })()`);

  const dismiss = await evaluate(page, `(async () => {
    const menu = () => document.querySelector('[role="menu"][aria-label="Permission modes"]');
    document.querySelector('header')?.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
    document.querySelector('header')?.click();
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 50));
    const outsideClick = !menu();
    document.querySelector('button[aria-label^="Permission mode:"]')?.click();
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 50));
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 50));
    return {
      outsideClick,
      escape: !menu(),
      focusRestored: document.activeElement?.matches('button[aria-label^="Permission mode:"]') === true,
    };
  })()`);

  await selectMode('Accept edits');
  await selectMode('Full access');
  const fullAccessLabel = await evaluate(page, `document.querySelector('button[aria-label^="Permission mode:"]')?.getAttribute('aria-label')`);
  await selectMode('Ask for approval');
  const restoredLabel = await evaluate(page, `document.querySelector('button[aria-label^="Permission mode:"]')?.getAttribute('aria-label')`);
  const events = await evaluate(page, `window.__permissionAuditEvents`);

  await openMenu();
  await evaluate(page, `document.querySelector('button[aria-label="Toggle theme"]')?.click()`);
  await delay(250);
  const darkScreenshot = await screenshot(page, 'permission-menu-dark.png');
  const dark = await evaluate(page, `(() => {
    const menu = document.querySelector('[role="menu"][aria-label="Permission modes"]');
    return {
      appBackground: getComputedStyle(document.querySelector('main')).backgroundColor,
      menuBackground: getComputedStyle(menu).backgroundColor,
      menuColor: getComputedStyle(menu).color,
    };
  })()`);
  await evaluate(page, `document.querySelector('button[aria-label="Toggle theme"]')?.click()`);

  const report = {
    lightScreenshot,
    darkScreenshot,
    light,
    dark,
    dismiss,
    transitions: { events, fullAccessLabel, restoredLabel },
  };
  const reportPath = resolve(outputDir, 'report.json');
  writeFileSync(reportPath, `${JSON.stringify(report, null, 2)}\n`);
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
} finally {
  page.close();
}
