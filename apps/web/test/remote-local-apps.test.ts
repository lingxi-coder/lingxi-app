import assert from 'node:assert/strict';
import test from 'node:test';
import { LocalAppsController, type AppWindow } from '../src/remote/appsController';
import type { ViewerConnection } from '../src/remote/viewerConnection';

const appId = `a${'1'.repeat(32)}`;
const app = { app_id: appId, name: 'Example', summary: 'Example app', revision: 1, phase: 'running', background: false, execution_host: 'desktop', remote_enabled: true };
const openResult = { app_id: appId, revision: 1, url: `https://${appId}-${'a'.repeat(20)}.desktop.apps.example.com/__lingxi_remote_bootstrap#${'s'.repeat(43)}`, expires_at: 2000 };
const tick = () => new Promise(resolve => setTimeout(resolve, 0));
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function setup() {
  const requests: Array<{ method: string; params: unknown; options?: { timeoutMs?: number }; pending: ReturnType<typeof deferred<unknown>> }> = [];
  let listener: ((state: { phase: string }) => void) | undefined;
  const connection = {
    state: { phase: 'ready' },
    subscribe(fn: typeof listener) { listener = fn; return () => { listener = undefined; }; },
    request(method: string, params: unknown, options?: { timeoutMs?: number }) { const pending = deferred<unknown>(); requests.push({ method, params, options, pending }); return pending.promise; },
  };
  const controller = new LocalAppsController(connection as unknown as ViewerConnection, () => 1000);
  const disconnect = () => { connection.state.phase = 'reconnecting'; listener?.(connection.state); };
  const reconnect = () => { connection.state.phase = 'ready'; listener?.(connection.state); };
  return { controller, requests, disconnect, reconnect };
}
function popup() {
  let navigated: string | undefined;
  let closed = false;
  const window: AppWindow = { get closed() { return closed; }, opener: {}, location: { replace(url) { navigated = url; } }, close() { closed = true; } };
  return { window, get navigated() { return navigated; }, get closed() { return closed; } };
}

test('catalog loading, refresh, error and out-of-order results', async () => {
  const { controller, requests } = setup();
  controller.activate();
  assert.equal(controller.getState().loading, true);
  void controller.refresh();
  requests[1]!.pending.resolve({ apps: [] });
  await tick();
  requests[0]!.pending.resolve({ apps: [app] });
  await tick();
  assert.deepEqual(controller.getState().apps, []);
  void controller.refresh();
  requests[2]!.pending.reject(new Error('network'));
  await tick();
  assert.equal(controller.getState().error, 'list');
  assert.deepEqual(controller.getState().apps, []);
  controller.deactivate();
});

test('reserves a popup before requesting an app and never stores its ticket', async () => {
  const { controller, requests } = setup();
  controller.activate(); requests[0]!.pending.resolve({ apps: [app] }); await tick();
  const win = popup();
  const opening = controller.open(appId, () => win.window);
  assert.equal(requests[1]!.method, 'apps.open');
  assert.deepEqual(requests[1]!.params, { app_id: appId });
  assert.deepEqual(requests[1]!.options, { timeoutMs: 930_000 });
  assert.equal(win.window.opener, null);
  requests[1]!.pending.resolve(openResult);
  await opening;
  assert.equal(win.navigated, openResult.url);
  assert.equal(JSON.stringify(controller.getState()).includes('s'.repeat(43)), false);
});

test('blocked popups, failures and invalid or expired URLs do not navigate', async () => {
  const { controller, requests } = setup();
  controller.activate(); requests[0]!.pending.resolve({ apps: [app] }); await tick();
  await controller.open(appId, () => null);
  assert.equal(controller.getState().error, 'popup');
  assert.equal(requests.length, 1);
  for (const result of [{ ...openResult, url: 'javascript:alert(1)' }, { ...openResult, url: 'https://user:pass@app.example.com/' }, { ...openResult, expires_at: 999 }, { ...openResult, revision: 2 }, { ...openResult, app_id: `a${'2'.repeat(32)}` }]) {
    const win = popup(); const opening = controller.open(appId, () => win.window);
    requests[requests.length - 1]!.pending.resolve(result); await opening;
    assert.equal(win.closed, true); assert.equal(win.navigated, undefined); assert.equal(controller.getState().error, 'invalid');
  }
  const win = popup(); const opening = controller.open(appId, () => win.window);
  requests[requests.length - 1]!.pending.reject(new Error('host unavailable')); await opening;
  assert.equal(win.closed, true); assert.equal(controller.getState().error, 'open');
});

test('disconnect cancels outstanding opens and ignores old catalog results', async () => {
  const { controller, requests, disconnect, reconnect } = setup();
  controller.activate(); requests[0]!.pending.resolve({ apps: [app] }); await tick();
  void controller.refresh();
  const win = popup(); const opening = controller.open(appId, () => win.window);
  disconnect();
  requests[1]!.pending.resolve({ apps: [app] }); requests[2]!.pending.resolve(openResult);
  await opening; await tick();
  assert.equal(win.closed, true); assert.equal(win.navigated, undefined);
  assert.equal(controller.getState().apps, undefined);
  reconnect();
  assert.equal(requests[3]!.method, 'apps.list');
  requests[3]!.pending.resolve({ apps: [] }); await tick();
  assert.deepEqual(controller.getState().apps, []);
});

test('closing a reserved window releases the UI and ignores its late response', async () => {
  const { controller, requests } = setup();
  controller.activate(); requests[0]!.pending.resolve({ apps: [app] }); await tick();
  const first = popup(); const oldOpening = controller.open(appId, () => first.window);
  first.window.close();
  await new Promise(resolve => setTimeout(resolve, 600));
  assert.equal(controller.getState().opening, undefined);
  const second = popup(); const currentOpening = controller.open(appId, () => second.window);
  requests[1]!.pending.resolve(openResult); await oldOpening;
  assert.equal(first.navigated, undefined);
  assert.equal(controller.getState().opening, appId);
  requests[2]!.pending.resolve(openResult); await currentOpening;
  assert.equal(second.navigated, openResult.url);
  controller.deactivate();
});

test('unmount closes an in-flight popup synchronously before passive effects', async () => {
  const { createElement, useSyncExternalStore } = await import('react');
  const { createRequire } = await import('node:module');
  const { create, act } = createRequire(import.meta.url)('react-test-renderer');
  const { readFileSync } = await import('node:fs');
  const { transpileModule, JsxEmit, ModuleKind, ScriptTarget } = await import('typescript');
  const { t } = await import('../src/remote/strings');
  const { displayText } = await import('../src/remote/text');
  const require = createRequire(import.meta.url);
  const source = readFileSync(new URL('../src/remote/LocalApps.tsx', import.meta.url), 'utf8');
  const compiled = transpileModule(source, { compilerOptions: { jsx: JsxEmit.ReactJSX, module: ModuleKind.CommonJS, target: ScriptTarget.ES2020 } }).outputText;
  const module = { exports: {} as { LocalApps: any } };
  const substitute = (specifier: string): unknown => {
    if (specifier === './appsController') return { LocalAppsController };
    if (specifier === './strings') return { t };
    if (specifier === './text') return { displayText };
    if (specifier === './useRemote') return { useStore: (store: LocalAppsController) => useSyncExternalStore(store.subscribe, store.getState) };
    return require(specifier);
  };
  new Function('require', 'module', 'exports', compiled)(substitute, module, module.exports);
  const { LocalApps } = module.exports;
  const requests: Array<{ method: string; params: unknown; options?: { timeoutMs?: number }; pending: ReturnType<typeof deferred<unknown>> }> = [];
  const win = popup();
  const previousWindow = globalThis.window;
  const connection = {
    state: { phase: 'ready' },
    subscribe() { return () => {}; },
    request(method: string, params: unknown, options?: { timeoutMs?: number }) { const pending = deferred<unknown>(); requests.push({ method, params, options, pending }); return pending.promise; },
  };
  Object.assign(globalThis, { window: { open: () => win.window } });
  let renderer: any;
  try {
    await act(async () => { renderer = create(createElement(LocalApps, { locale: 'en', connection, ready: true })); });
    await act(async () => { requests[0]!.pending.resolve({ apps: [app] }); await tick(); });
    act(() => { renderer.root.findAllByType('button')[1].props.onClick(); });
    assert.equal(win.closed, false);
    renderer.unmount();
    assert.equal(win.closed, true);
    requests[1]!.pending.resolve(openResult);
    await tick();
    assert.equal(win.navigated, undefined);
  } finally {
    Object.assign(globalThis, { window: previousWindow });
  }
});
