import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { readFileSync } from 'node:fs';
import { transpileModule, JsxEmit, ModuleKind, ScriptTarget } from 'typescript';
import test from 'node:test';
import { createElement, useState } from 'react';
import { ManualClock } from '@lingxi/bridge-client/remote';
import type { MessageDto } from '@lingxi/bridge-client';
import { Composer } from '../src/remote/Composer';
import { Files } from '../src/remote/Files';
import type { RemoteClient } from '../src/remote/remoteClient';
import { SessionController, questionAnswers, type SessionIo } from '../src/remote/remoteClient';

const { create, act } = createRequire(import.meta.url)('react-test-renderer');
const noPending = { permissions: [], questions: [], computer_access: [] };
const message = (text: string): MessageDto => ({ role: 'assistant', blocks: [{ type: 'text', text }] });
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

test('a delayed file listing preserves a receipt uploaded after the listing started', async () => {
  const listing = deferred<any[]>();
  const receipt = { transfer_id: 'ab'.repeat(16), ref: 'ab'.repeat(16), session_id: 's', name: 'new.bin', size: 0, sha256: '0'.repeat(64) };
  const client = { listFiles: () => listing.promise, uploadFile: async () => receipt } as unknown as RemoteClient;
  let renderer: any;
  await act(async () => { renderer = create(createElement(Files, { client, session: 's', locale: 'en', disabled: false, onAttach() {} })); });
  try {
    await act(async () => { renderer.root.findByType('input').props.onChange({ target: { files: [new File([], 'new.bin')], value: '' } }); });
    assert.equal(renderer.root.findAllByType('li').length, 1);
    await act(async () => { listing.resolve([]); await listing.promise; });
    assert.equal(renderer.root.findAllByType('li').length, 1);
  } finally { await act(async () => renderer.unmount()); }
});
function historyController() {
  const histories: ReturnType<typeof deferred<{ messages: MessageDto[]; first_index: number; has_more: boolean }>>[] = [];
  const io: SessionIo = {
    request: (async (method: string) => {
      if (method !== 'session.history') return {};
      const pending = deferred<{ messages: MessageDto[]; first_index: number; has_more: boolean }>();
      histories.push(pending);
      return pending.promise;
    }) as SessionIo['request'],
    notify() {}, clock: new ManualClock(),
  };
  const controller = new SessionController('s', io, () => true);
  const snapshot = (epoch: string, text: string, first: number) => {
    controller.handleSnapshot({ session_id: 's', phase: 'begin', epoch, first_message_index: first });
    controller.handleSnapshot({ session_id: 's', phase: 'messages', messages: [message(text)] });
    controller.handleSnapshot({ session_id: 's', phase: 'end', cursor: { epoch, seq: 0 }, pending: noPending, turn: { running: false } });
  };
  return { controller, histories, snapshot };
}

test('a delayed older-history page cannot corrupt a reset snapshot', async () => {
  const { controller, histories, snapshot } = historyController();
  snapshot('old', 'old tail', 100);
  const old = controller.loadOlder();
  controller.handleReset();
  snapshot('new', 'new tail', 50);
  histories[0]!.resolve({ messages: [message('OLD HISTORY')], first_index: 0, has_more: false });
  await old;
  assert.deepEqual(controller.getState().transcript.conversation.items.map((item) => item.type === 'narration' ? item.text : item.type), ['new tail']);
  assert.equal(controller.getState().hasOlder, true);
});

test('a stale history rejection cannot release the loading state of a newer request', async () => {
  const { controller, histories, snapshot } = historyController();
  snapshot('old', 'old tail', 100);
  const old = controller.loadOlder();
  controller.handleReset();
  snapshot('new', 'new tail', 50);
  const current = controller.loadOlder();
  histories[0]!.reject(new Error('old request ended late'));
  await old;
  assert.equal(controller.getState().loadingOlder, true);
  histories[1]!.resolve({ messages: [message('NEW HISTORY')], first_index: 0, has_more: false });
  await current;
  assert.equal(controller.getState().loadingOlder, false);
  assert.deepEqual(controller.getState().transcript.conversation.items.map((item) => item.type === 'narration' ? item.text : item.type), ['NEW HISTORY', 'new tail']);
});

test('question text that names an Object prototype property is still sent as an own answer', () => {
  const request = { request_id: 1, questions: [{ question: '__proto__', header: 'Pick', multi_select: false, options: [{ label: 'APPLE', description: '' }] }] };
  const result = questionAnswers(request, new Map([['__proto__', ['APPLE']]]), new Map());
  assert.equal(Object.prototype.hasOwnProperty.call(result, '__proto__'), true);
  assert.equal(JSON.stringify(result), '{"__proto__":"APPLE"}');
});

test('a text follow-up while running preserves the prepared image draft for the next idle send', async () => {
  const original = globalThis.createImageBitmap;
  globalThis.createImageBitmap = (async () => ({ width: 128, height: 128, close() {} })) as typeof createImageBitmap;
  const sent: Array<{ text: string; images: unknown[] }> = [];
  const props = { locale: 'en' as const, disabled: false, onStop() {}, onSend(text: string, images: unknown[]) { sent.push({ text, images }); } };
  let renderer: any;
  try {
    await act(async () => { renderer = create(createElement(Composer, { ...props, running: false })); });
    await act(async () => { await renderer.root.findByProps({ type: 'file' }).props.onChange({ target: { files: [new Blob(['red'], { type: 'image/png' })] } }); });
    assert.equal(renderer.root.findAllByType('img').length, 1);
    await act(async () => { renderer.update(createElement(Composer, { ...props, running: true })); });
    await act(async () => { renderer.root.findByType('textarea').props.onChange({ target: { value: 'follow-up' } }); });
    await act(async () => { renderer.root.findByType('form').props.onSubmit({ preventDefault() {} }); });
    assert.deepEqual(sent, [{ text: 'follow-up', images: [] }]);
    assert.equal(renderer.root.findAllByType('img').length, 1, 'the unsent image must remain in the composer');
    await act(async () => { renderer.update(createElement(Composer, { ...props, running: false })); });
    await act(async () => { renderer.root.findByType('form').props.onSubmit({ preventDefault() {} }); });
    assert.equal(sent.length, 2);
    assert.equal(sent[1]!.images.length, 1);
    assert.equal(renderer.root.findAllByType('img').length, 0);
  } finally {
    if (renderer) act(() => renderer.unmount());
    globalThis.createImageBitmap = original;
  }
});

test('overflow while a snapshot is assembling requests a fresh base instead of skipping live records', async () => {
  const requests: Array<{ method: string; params: unknown }> = [];
  const controller = new SessionController('s', {
    request: (async (method: string, params: unknown) => {
      requests.push({ method, params });
      return { mode: 'snapshot', epoch: 'e1' };
    }) as SessionIo['request'],
    notify() {}, clock: new ManualClock(),
  }, () => true);
  controller.activate();
  await Promise.resolve();
  controller.handleSnapshot({ session_id: 's', phase: 'begin', epoch: 'e1', first_message_index: 0 });
  for (let seq = 1; seq <= 5001; seq++) controller.handleEvent({ seq, record: { type: 'event', event: { type: 'text_delta', text: 'x' } } });
  controller.handleSnapshot({ session_id: 's', phase: 'end', cursor: { epoch: 'e1', seq: 0 }, pending: noPending, turn: { running: true } });
  await Promise.resolve();
  assert.equal(requests.filter((request) => request.method === 'session.attach').length, 2);
  assert.deepEqual(requests.at(-1)!.params, { session_id: 's' }, 'resynchronization cannot resume beyond a record we dropped');
  assert.equal(controller.getState().status, 'loading');
  controller.deactivate();
});

test('a snapshot whose cursor covers the overflow does not need another attach', async () => {
  const requests: string[] = [];
  const controller = new SessionController('s', {
    request: (async (method: string) => { requests.push(method); return { mode: 'snapshot', epoch: 'e1' }; }) as SessionIo['request'],
    notify() {}, clock: new ManualClock(),
  }, () => true);
  controller.activate();
  await Promise.resolve();
  controller.handleSnapshot({ session_id: 's', phase: 'begin', epoch: 'e1', first_message_index: 0 });
  for (let seq = 1; seq <= 5001; seq++) controller.handleEvent({ seq, record: { type: 'event', event: { type: 'text_delta', text: 'x' } } });
  controller.handleSnapshot({ session_id: 's', phase: 'end', cursor: { epoch: 'e1', seq: 5001 }, pending: noPending, turn: { running: false } });
  await Promise.resolve();
  assert.equal(requests.filter((method) => method === 'session.attach').length, 1);
  assert.equal(controller.getState().status, 'live');
  controller.deactivate();
});

test('starting a turn during image preparation does not drop or prematurely send that draft', async () => {
  const original = globalThis.createImageBitmap;
  const bitmap = deferred<ImageBitmap>();
  globalThis.createImageBitmap = (() => bitmap.promise) as typeof createImageBitmap;
  const sent: Array<{ text: string; images: unknown[] }> = [];
  const props = { locale: 'en' as const, disabled: false, onStop() {}, onSend(text: string, images: unknown[]) { sent.push({ text, images }); } };
  let renderer: any;
  try {
    await act(async () => { renderer = create(createElement(Composer, { ...props, running: false })); });
    await act(async () => { renderer.root.findByProps({ type: 'file' }).props.onChange({ target: { files: [new Blob(['red'], { type: 'image/png' })] } }); });
    await act(async () => { renderer.update(createElement(Composer, { ...props, running: true })); });
    await act(async () => { renderer.root.findByType('textarea').props.onChange({ target: { value: 'follow-up' } }); });
    await act(async () => { renderer.root.findByType('form').props.onSubmit({ preventDefault() {} }); });
    assert.equal(sent.length, 0, 'image preparation must finish before the composer submits');
    await act(async () => { bitmap.resolve({ width: 128, height: 128, close() {} } as ImageBitmap); await new Promise<void>((resolve) => setImmediate(resolve)); });
    assert.equal(renderer.root.findAllByType('img').length, 1);
    await act(async () => { renderer.root.findByType('form').props.onSubmit({ preventDefault() {} }); });
    assert.deepEqual(sent, [{ text: 'follow-up', images: [] }]);
    assert.equal(renderer.root.findAllByType('img').length, 1);
  } finally {
    if (renderer) act(() => renderer.unmount());
    globalThis.createImageBitmap = original;
  }
});

test('the actual remote route remounts session state for a different session or account', async () => {
  // Run the real routing component with connector and leaf-view boundaries
  // substituted. Its element keys must isolate stateful session descendants.
  let account = { status: 'signedIn', uid: 'user-a', email: 'a@example.test' };
  const statefulSession = ({ sessionId }: { sessionId: string }) => {
    const [draft, setDraft] = useState('');
    return createElement('input', { value: draft, 'data-session': sessionId, onChange: (event: { target: { value: string } }) => setDraft(event.target.value) });
  };
  const hooks = {
    getDeviceKeys: () => ({}), signOutUser: async () => {}, supportsRemoteCrypto: async () => true,
    useAuthState: () => account, useConnectionState: () => ({ phase: 'ready' }),
    useHosts: () => ({ hosts: [] }), usePersistentPairing: () => true, usePins: () => [],
    useRemoteHost: () => ({ connection: {}, client: {} }), useRtdb: () => ({}),
  };
  const require = createRequire(import.meta.url);
  const transpiled = transpileModule(readFileSync(new URL('../src/pages/RemotePage.tsx', import.meta.url), 'utf8'), {
    compilerOptions: { jsx: JsxEmit.ReactJSX, module: ModuleKind.CommonJS, target: ScriptTarget.ES2020 },
  }).outputText;
  const module = { exports: {} as { default: (props: any) => ReturnType<typeof createElement> } };
  const substitute = (specifier: string): unknown => {
    if (specifier === '../remote/useRemote') return hooks;
    if (specifier === '../remote/SessionView') return { SessionView: statefulSession };
    if (specifier === '../remote/ConnectionScreen') return { ConnectionScreen: () => null };
    if (specifier === '../remote/HostList') return { HostList: () => null };
    if (specifier === '../remote/LocalApps') return { LocalApps: () => null };
    if (specifier === '../remote/SessionList') return { SessionList: () => null };
    if (specifier === '../remote/deviceKey') return { browserDeviceName: () => 'Test Browser' };
    if (specifier === '../remote/remote.css') return {};
    if (specifier === '../app/router') return require('../src/app/router.ts');
    if (specifier === '../remote/strings') return require('../src/remote/strings.ts');
    return require(specifier);
  };
  new Function('require', 'module', 'exports', transpiled)(substitute, module, module.exports);
  const RemotePage = module.exports.default;
  const originalWindow = globalThis.window;
  globalThis.window = { location: { pathname: '/remote/host/session-a' } } as Window & typeof globalThis;
  let renderer: any;
  const props = { locale: 'en', theme: 'light', route: '/remote/host/session-a', onLocaleChange() {}, onThemeToggle() {} };
  try {
    await act(async () => { renderer = create(createElement(RemotePage, props)); });
    await act(async () => { renderer.root.findByType('input').props.onChange({ target: { value: 'session-a private draft' } }); });
    await act(async () => { renderer.update(createElement(RemotePage, { ...props, theme: 'dark' })); });
    assert.equal(renderer.root.findByType('input').props.value, 'session-a private draft', 'ordinary parent rerenders retain the draft');
    await act(async () => { renderer.update(createElement(RemotePage, { ...props, route: '/remote/host/session-b' })); });
    assert.equal(renderer.root.findByType('input').props.value, '', 'another session cannot inherit the old draft');
    await act(async () => { renderer.root.findByType('input').props.onChange({ target: { value: 'user-a private draft' } }); });
    account = { status: 'signedIn', uid: 'user-b', email: 'b@example.test' };
    await act(async () => { renderer.update(createElement(RemotePage, { ...props, route: '/remote/host/session-b' })); });
    assert.equal(renderer.root.findByType('input').props.value, '', 'another account cannot inherit the session state');
  } finally {
    if (renderer) act(() => renderer.unmount());
    globalThis.window = originalWindow;
  }
});

test('the actual remote host hook never exposes a client from the previous route or account', async () => {
  const require = createRequire(import.meta.url);
  let nextConnection = 0;
  class Connection {
    readonly number = ++nextConnection;
    readonly state: { hostId: string };
    constructor(readonly deps: { uid: string; hostId: string; rtdb: object }) { this.state = { hostId: deps.hostId }; }
    start() {} stop() {} nudge() {}
  }
  class Client { constructor(readonly connection: Connection) {} dispose() {} }
  const substitute = (specifier: string): unknown => {
    if (specifier === 'react') return require('react');
    if (specifier === 'firebase/auth') return {};
    if (specifier === '@lingxi/bridge-client/remote') return {};
    if (specifier === '../app/firebase') return { auth: {} };
    if (specifier === './deviceKey') return { DeviceKeys: class {}, browserDeviceStore: () => undefined };
    if (specifier === './env') return { browserStorage: () => undefined, forceTunnelRequested: () => false };
    if (specifier === './peerLink') return { browserPeerConnectionFactory: () => undefined };
    if (specifier === './remoteClient') return { RemoteClient: Client };
    if (specifier === './tunnelMemory') return { TunnelMemory: class {} };
    if (specifier === './viewerConnection') return { ViewerConnection: Connection };
    if (specifier === './text' || specifier === './visibleSubscription') return {};
    throw new Error(`unexpected hook dependency: ${specifier}`);
  };
  const transpiled = transpileModule(readFileSync(new URL('../src/remote/useRemote.ts', import.meta.url), 'utf8'), {
    compilerOptions: { module: ModuleKind.CommonJS, target: ScriptTarget.ES2020 },
  }).outputText;
  const module = { exports: {} as { useRemoteHost: (uid: string, rtdb: any, hostId: string) => any } };
  new Function('require', 'module', 'exports', transpiled)(substitute, module, module.exports);
  const observations: Array<{ expected: string; actual: string | undefined; expectedRtdb: object; actualRtdb: object | undefined }> = [];
  const Probe = ({ uid, rtdb, hostId }: { uid: string; rtdb: object; hostId: string }) => {
    const remote = module.exports.useRemoteHost(uid, rtdb, hostId);
    const deps = remote?.client.connection.deps;
    observations.push({ expected: `${uid}/${hostId}`, actual: deps && `${deps.uid}/${deps.hostId}`, expectedRtdb: rtdb, actualRtdb: deps?.rtdb });
    return null;
  };
  const originalWindow = globalThis.window;
  const originalDocument = globalThis.document;
  globalThis.window = { location: { search: '' }, addEventListener() {}, removeEventListener() {} } as unknown as Window & typeof globalThis;
  globalThis.document = { visibilityState: 'visible', addEventListener() {}, removeEventListener() {} } as unknown as Document;
  let renderer: any;
  const first = {}, second = {};
  try {
    await act(async () => { renderer = create(createElement(Probe, { uid: 'u1', hostId: 'a', rtdb: first })); });
    await act(async () => { renderer.update(createElement(Probe, { uid: 'u1', hostId: 'b', rtdb: first })); });
    await act(async () => { renderer.update(createElement(Probe, { uid: 'u2', hostId: 'b', rtdb: first })); });
    await act(async () => { renderer.update(createElement(Probe, { uid: 'u2', hostId: 'b', rtdb: second })); });
    assert.equal(observations.filter((item) => item.actual !== undefined).length >= 4, true);
    for (const item of observations) {
      if (item.actual === undefined) continue;
      assert.equal(item.actual, item.expected, 'a render cannot expose the last route or account client before effects replace it');
      assert.equal(item.actualRtdb, item.expectedRtdb, 'replacing the account database fences the prior client too');
    }
  } finally {
    if (renderer) act(() => renderer.unmount());
    globalThis.window = originalWindow;
    globalThis.document = originalDocument;
  }
});
