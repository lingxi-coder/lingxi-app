import test from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { readFileSync, realpathSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { tmpdir } from 'node:os';
import vm from 'node:vm';
import ts from 'typescript';
import * as contract from '../../../packages/deep-links/index';
import { DeepLinkQueue } from '../src/main/deepLinkQueue';

function runtime(ready: () => Promise<void> = async () => {}) {
  const app = Object.assign(new EventEmitter(), { isReady: () => state.ready, isPackaged: false, setAsDefaultProtocolClient: () => false });
  const state = { ready: false, clipboard: '', shown: 0, notices: [] as unknown[], dialogs: [] as any[] };
  const windows: any[] = [];
  const handlers = new Map<string, (...args: any[]) => any>();
  const source = ts.transpileModule(readFileSync(new URL('../src/main/deepLinks.ts', import.meta.url), 'utf8'), { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText;
  const exported: any = {};
  vm.runInNewContext(source, {
    exports: exported,
    process: { argv: ['electron', 'main.js'] },
    require: (id: string) => {
      if (id === 'electron') return { app, BrowserWindow: { getAllWindows: () => windows }, ipcMain: { handle: (name: string, handler: any) => handlers.set(name, handler) }, dialog: { showMessageBox: async (value: any) => state.dialogs.push(value) }, clipboard: { readText: () => state.clipboard, writeText: (value: string) => { state.clipboard = value; } } };
      if (id === 'node:crypto') return { createHash };
      if (id === 'node:fs') return { realpathSync };
      if (id.endsWith('packages/deep-links/index')) return contract;
      if (id === './deepLinkQueue') return { DeepLinkQueue };
      throw new Error(id);
    },
  });
  const api = exported.installDeepLinks(() => [tmpdir()], () => state.shown++, ready);
  const window = () => {
    const webContents = Object.assign(new EventEmitter(), { mainFrame: {}, send: (...args: any[]) => state.notices.push(args) });
    const value = { webContents };
    windows.push(value);
    app.emit('browser-window-created', {}, value);
    return { value, sender: { sender: webContents, senderFrame: webContents.mainFrame } };
  };
  const open = (url: string) => app.emit('open-url', { preventDefault() {} }, url);
  return { app, state, windows, handlers, api, window, open };
}

test('real main handlers buffer cold launch until restoration and replay after renderer/window recreation', async () => {
  let finish!: () => void;
  const restored = new Promise<void>(resolve => { finish = resolve; });
  const r = runtime(() => restored);
  r.open('lingxi://session/first');
  assert.equal(r.api.hasPending(), true);
  assert.equal(r.state.shown, 0);
  r.state.ready = true;
  r.app.emit('ready');
  const firstWindow = r.window();
  let delivered = false;
  const pending = r.handlers.get('lingxi:deep-link:next')!(firstWindow.sender).then((value: unknown) => { delivered = true; return value as any; });
  await Promise.resolve();
  assert.equal(delivered, false);
  finish();
  const first = await pending;
  assert.equal(first.target.route, 'session/first');
  r.windows.splice(0);
  const recreated = r.window();
  recreated.value.webContents.emit('did-finish-load');
  assert.ok(r.state.notices.length > 0);
  const replay = await r.handlers.get('lingxi:deep-link:next')!(recreated.sender);
  assert.equal(replay.id, first.id);
  r.handlers.get('lingxi:deep-link:ack')!(recreated.sender, first.id);
  assert.equal(await r.handlers.get('lingxi:deep-link:next')!(recreated.sender), null);
});

test('real handlers reject subframes, deduplicate second-instance delivery and report errors without raw URL', async () => {
  const r = runtime();
  r.open('lingxi://import/provider#payload=%7B%22apiKey%22%3A%22do-not-display-this%22%7D');
  assert.equal(r.state.dialogs.length, 0);
  r.state.ready = true;
  r.app.emit('ready');
  assert.equal(r.state.dialogs.length, 1);
  assert.doesNotMatch(JSON.stringify(r.state.dialogs), /do-not-display-this/);
  const w = r.window();
  const untrusted = { ...w.sender, senderFrame: {} };
  await assert.rejects(r.handlers.get('lingxi:deep-link:next')!(untrusted), /Invalid sender/);
  r.app.emit('second-instance', {}, ['electron', 'lingxi://session/a', 'https://lingxios.net/open/session/a']);
  const first = await r.handlers.get('lingxi:deep-link:next')!(w.sender);
  assert.throws(() => r.handlers.get('lingxi:deep-link:ack')!(untrusted, first.id), /Invalid sender/);
  r.handlers.get('lingxi:deep-link:ack')!(w.sender, first.id, 'Session is no longer on this device');
  assert.equal(r.state.dialogs.at(-1).detail, 'Session is no longer on this device');
  assert.equal(await r.handlers.get('lingxi:deep-link:next')!(w.sender), null);
});

test('main copy API generates stable UUID message links and rejects unknown workspaces and unsaved sessions', () => {
  const r = runtime();
  r.api.copySessionLink(tmpdir(), 'Saved-ID', '123e4567-e89b-12d3-a456-426614174000');
  const target = contract.parseDeepLink(r.state.clipboard);
  assert.equal(target.route, 'session/Saved-ID');
  assert.equal(target.params.message, '123e4567-e89b-12d3-a456-426614174000');
  assert.match(target.params.workspace, /^project\.[0-9a-f]{64}$/);
  assert.throws(() => r.api.copySessionLink('/unknown', 'Saved-ID'), /Session unavailable/);
  assert.throws(() => r.api.copySessionLink(tmpdir(), 'pending-123'), /Session unavailable/);
  assert.throws(() => r.api.copySessionLink(tmpdir(), 'Saved-ID', 'unstable-row-id'), /Invalid message/);
});


test('an explicit unavailable workspace cannot dispatch any route into the active project', async () => {
  const r = runtime();
  r.state.ready = true;
  r.app.emit('ready');
  const w = r.window();
  for (const route of ['import/provider', 'new', 'terminal/local', 'runtime/task/task-1', 'project/project.missing']) {
    r.open(`lingxi://${route}?workspace=project.missing`);
  }
  assert.equal(await r.handlers.get('lingxi:deep-link:next')!(w.sender), null);
  assert.equal(r.api.hasPending(), false, 'missing workspace links cannot block later local deliveries');
  assert.equal(r.state.dialogs.length, 5);
  assert.ok(r.state.dialogs.every(dialog => /workspace.*not available on this device/.test(dialog.detail)));

  const key = `project.${createHash('sha256').update(realpathSync.native(tmpdir())).digest('hex')}`;
  r.open('lingxi://new?workspace=project.missing');
  r.open(`lingxi://new?workspace=${key}`);
  const local = await r.handlers.get('lingxi:deep-link:next')!(w.sender);
  assert.equal(local.target.route, 'new');
  assert.equal(local.projectPath, tmpdir(), 'a subsequent resolvable workspace still dispatches its own project');
});
