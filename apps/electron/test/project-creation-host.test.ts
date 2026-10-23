import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, renameSync, rmSync, rmdirSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test, type TestContext } from 'node:test';
import { HostController } from '../src/main/host';
import { DiagnosticBuffer } from '../src/main/host-utils';
import { SettingsStore } from '../src/main/settings';
import type { ProjectSessionCatalog } from '../src/main/session-catalog';
import { CH_PROJECT_APPS_DEFAULT, CH_PROJECT_CREATE, CH_PROJECT_FOLDER_PICK } from '../src/shared/projectCreation';

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
  const configHome = join(root, '.another-product');
  const catalog = { productPaths: async () => ({ configHome, appProjectsRoot: join(configHome, '.apps') }) };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), catalog as ProjectSessionCatalog, ipc as any,
    undefined, undefined, undefined, undefined, dialogs);
  const sender = new EventEmitter() as any;
  sender.mainFrame = { url: 'app://desktop/index.html' }; sender.isDestroyed = () => false; sender.send = () => {};
  host.registerWindow(sender, sender.mainFrame.url); host.registerIpc();
  const event = { sender, senderFrame: sender.mainFrame };
  t.after(() => { host.dispose(); rmSync(root, { recursive: true, force: true }); });
  return { path, other, settings, previous, calls, host, dialogs, event, handlers, configHome, catalog,
    setPicked: (value?: string) => { picked = value; },
    pick: (purpose = 'project') => handlers.get(CH_PROJECT_FOLDER_PICK)!(event, purpose),
    defaults: () => handlers.get(CH_PROJECT_APPS_DEFAULT)!(event),
    create: (id: unknown, source = event, name?: unknown) => handlers.get(CH_PROJECT_CREATE)!(source, id, name) };
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

test('app projects use the Harness product config directory without creating files while staged', async t => {
  const h = fixture(t, true), before = h.settings.getPublic();
  const selection = await h.defaults();
  assert.equal(selection.path, join(h.configHome, '.apps'));
  assert.equal(selection.createDirectory, true);
  assert.equal(existsSync(h.configHome), false);
  assert.deepEqual(h.settings.getPublic(), before);
  assert.deepEqual(h.calls, []);
  const workspace = await h.create(selection.id, h.event, ' Reading garden ');
  assert.equal(workspace.path, join(h.configHome, '.apps', 'Reading garden'));
  assert.equal(existsSync(workspace.path), true);
  assert.deepEqual(h.settings.getPublic().activeSession, h.previous);
  assert.deepEqual(h.calls, [], 'the renderer admits exactly one new authoring chat after confirmation');
  await assert.rejects(h.create(selection.id, h.event, 'Other'), /Choose a source folder/);
});

test('a first app project adds its workspace without spawning a duplicate initial session', async t => {
  const h = fixture(t), selection = await h.defaults();
  const workspace = await h.create(selection.id, h.event, 'First app');
  assert.equal(h.settings.getPublic().activeProject, workspace.path);
  assert.equal(h.settings.getPublic().activeSession, undefined);
  assert.deepEqual(h.calls, []);
});

test('new app directory names cannot traverse the product root or overwrite an existing folder', async t => {
  const h = fixture(t), selection = await h.defaults();
  for (const name of ['', '  ', '.', '..', '../escape', 'a/b', 'a\\b', '\0']) {
    await assert.rejects(h.create(selection.id, h.event, name), /project name/);
  }
  assert.equal(existsSync(h.configHome), false, 'invalid names must not create the root');
  const existing = join(selection.path, 'Existing');
  mkdirSync(existing, { recursive: true });
  writeFileSync(join(existing, 'keep.txt'), 'existing source');
  await assert.rejects(h.create(selection.id, h.event, 'Existing'), /already exists/);
  assert.equal(readFileSync(join(existing, 'keep.txt'), 'utf8'), 'existing source');
  assert.deepEqual(h.settings.getPublic().projects, []);
});

test('an app can explicitly select an existing source folder and skip the initial draft', async t => {
  const h = fixture(t), defaults = await h.defaults();
  let suggested: string | undefined;
  h.dialogs.pickDirectory = async (...args: string[]) => { suggested = args[0]; return h.path; };
  const selection = await h.pick('local-app');
  assert.equal(suggested, defaults.path);
  await h.create(selection.id);
  assert.deepEqual(h.settings.getPublic().projects, [h.path]);
  assert.deepEqual(h.calls, []);
});

test('an app configuration lookup failure does not fall back to a hardcoded product directory', async t => {
  const h = fixture(t);
  h.catalog.productPaths = async () => { throw new Error('Product settings unavailable'); };
  await assert.rejects(h.defaults(), /Product settings unavailable/);
  assert.deepEqual(h.settings.getPublic().projects, []);
  assert.equal(existsSync(h.configHome), false);
});

test('failed project persistence removes only the newly created empty directory and allows retry', async t => {
  const h = fixture(t, true), before = h.settings.getPublic(), selection = await h.defaults();
  rmSync(h.settings.settingsPath); mkdirSync(h.settings.settingsPath);
  await assert.rejects(h.create(selection.id, h.event, 'Retry app'));
  assert.deepEqual(h.settings.getPublic(), before);
  assert.equal(existsSync(join(selection.path, 'Retry app')), false);
  rmdirSync(h.settings.settingsPath);
  const workspace = await h.create(selection.id, h.event, 'Retry app');
  assert.equal(workspace.path, join(selection.path, 'Retry app'));
  assert.deepEqual(h.settings.getPublic().activeSession, h.previous);
});

test('failed project registration never removes a replacement directory at the same path', async t => {
  const h = fixture(t, true), selection = await h.defaults();
  const path = join(selection.path, 'Concurrent app'), moved = path + '-moved';
  h.settings.addProject = () => {
    renameSync(path, moved);
    mkdirSync(path);
    throw new Error('Registration failed after a concurrent rename');
  };
  await assert.rejects(h.create(selection.id, h.event, 'Concurrent app'), /Registration failed/);
  assert.equal(existsSync(path), true, 'the replacement directory belongs to its concurrent creator');
  assert.equal(existsSync(moved), true);
  assert.deepEqual(h.settings.getPublic().activeSession, h.previous);
});

test('a late default directory lookup cannot replace an explicit source folder', async t => {
  const h = fixture(t);
  let finish!: (paths: { configHome: string; appProjectsRoot: string }) => void;
  h.catalog.productPaths = () => new Promise(resolve => { finish = resolve; });
  const delayed = h.defaults();
  const selected = await h.pick('local-app');
  finish({ configHome: h.configHome, appProjectsRoot: join(h.configHome, '.apps') });
  assert.equal(await delayed, null);
  const workspace = await h.create(selected.id);
  assert.equal(workspace.path, h.path);
});
