import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { mkdirSync, mkdtempSync, realpathSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test, type TestContext } from 'node:test';
import { HostController } from '../src/main/host';
import { DiagnosticBuffer } from '../src/main/host-utils';
import { SettingsStore } from '../src/main/settings';
import { CH_PROJECT_CREATE, CH_PROJECT_FOLDER_PICK } from '../src/shared/projectCreation';

function fixture(t: TestContext, existing = false) {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-create-project-')));
  const path = join(root, 'Source folder'), other = join(root, 'Other folder');
  mkdirSync(path); mkdirSync(other);
  const settings = new SettingsStore(join(root, 'settings'));
  const previous = { projectPath: other, sessionId: '11111111-2222-4333-8444-555555555555' };
  if (existing) { settings.addProject(other); settings.activateProject(other); settings.setActiveSession(previous); }
  const calls: string[] = [];
  const bridge = { registerIpc() {}, registerWindow() {}, isProjectClosing: () => false,
    newSession: async (projectPath: string) => { calls.push(projectPath); return { projectPath, sessionId: previous.sessionId }; } };
  const handlers = new Map<string, (...args: any[]) => any>();
  const ipc = { handle: (channel: string, handler: (...args: any[]) => any) => handlers.set(channel, handler), removeHandler: (channel: string) => handlers.delete(channel) };
  let picked: string | undefined = path;
  const dialogs = { pickDirectory: async () => picked, trustDirectory: async () => true };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), undefined, ipc as any,
    undefined, undefined, undefined, undefined, dialogs);
  const sender = new EventEmitter() as any;
  sender.mainFrame = { url: 'app://desktop/index.html' }; sender.isDestroyed = () => false; sender.send = () => {};
  host.registerWindow(sender, sender.mainFrame.url); host.registerIpc();
  const event = { sender, senderFrame: sender.mainFrame };
  t.after(() => { host.dispose(); rmSync(root, { recursive: true, force: true }); });
  return { path, other, settings, previous, calls, host, dialogs, event, handlers,
    setPicked: (value?: string) => { picked = value; },
    pick: () => handlers.get(CH_PROJECT_FOLDER_PICK)!(event),
    create: (id: unknown, source = event) => handlers.get(CH_PROJECT_CREATE)!(source, id) };
}

test('choosing or cancelling a source folder leaves the project list and session untouched', async t => {
  const h = fixture(t, true), before = h.settings.getPublic();
  const selection = await h.pick();
  assert.equal(selection.path, h.path);
  assert.deepEqual(h.settings.getPublic(), before); assert.deepEqual(h.calls, []);
  h.setPicked(undefined); assert.equal(await h.pick(), null);
  assert.deepEqual(h.settings.getPublic(), before);
  // Cancelling Change must retain the folder still displayed by the dialog.
  await h.create(selection.id); assert.deepEqual(h.settings.getPublic().projects, [h.path, h.other]);
});

test('confirmation creates the first directory project and its initial draft only once', async t => {
  const h = fixture(t), selection = await h.pick();
  await h.create(selection.id);
  assert.deepEqual(h.settings.getPublic().projects, [h.path]);
  assert.equal(h.settings.getPublic().activeProject, h.path);
  assert.deepEqual(h.settings.getPublic().activeSession, { projectPath: h.path, sessionId: h.previous.sessionId });
  assert.deepEqual(h.calls, [h.path]);
  await assert.rejects(h.create(selection.id), /Choose a source folder/);
});

test('adding another project preserves the current session and all directory ownership', async t => {
  const h = fixture(t, true), selection = await h.pick();
  await h.create(selection.id);
  assert.deepEqual(h.settings.getPublic().activeSession, h.previous);
  assert.equal(h.settings.getPublic().activeProject, h.other);
  assert.deepEqual(h.calls, []);
});

test('creation accepts only the same window’s current native selection', async t => {
  const h = fixture(t), selection = await h.pick();
  await assert.rejects(h.create(h.path), /Choose a source folder/);
  const otherSender = { ...h.event.sender, mainFrame: { url: h.event.sender.mainFrame.url }, isDestroyed: () => false, once() {} };
  h.host.registerWindow(otherSender as any, otherSender.mainFrame.url);
  await assert.rejects(h.create(selection.id, { sender: otherSender, senderFrame: otherSender.mainFrame } as any), /Choose a source folder/);
  h.setPicked(h.other); const replacement = await h.pick();
  await assert.rejects(h.create(selection.id), /Choose a source folder/);
  await h.create(replacement.id); assert.deepEqual(h.settings.getPublic().projects, [h.other]);
});

test('out-of-order picker results cannot replace the newer choice', async t => {
  const h = fixture(t);
  let finish!: (value: string) => void;
  h.dialogs.pickDirectory = () => new Promise(resolve => { finish = resolve; });
  const old = h.pick();
  h.dialogs.pickDirectory = async () => h.other;
  const current = await h.pick();
  finish(h.path); assert.equal(await old, null);
  await h.create(current.id); assert.deepEqual(h.settings.getPublic().projects, [h.other]);
});

test('a removed folder is revalidated before any project or session is created', async t => {
  const h = fixture(t), selection = await h.pick();
  rmSync(h.path, { recursive: true });
  await assert.rejects(h.create(selection.id), /ENOENT|directory/);
  assert.deepEqual(h.settings.getPublic().projects, []); assert.deepEqual(h.calls, []);
});
