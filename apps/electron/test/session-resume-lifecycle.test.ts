import assert from 'node:assert/strict';
import { mkdtempSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test, type TestContext } from 'node:test';
import { SettingsStore } from '../src/main/settings';
import { HostController, CH_SESSION_ARCHIVE, CH_SESSION_METADATA_CAPTURE, CH_SESSION_NEW } from '../src/main/host';
import { CH_CONVERSATION_SPACE } from '../src/shared/conversationSpaces';
import { DiagnosticBuffer } from '../src/main/host-utils';

const sessionId = '11111111-2222-4333-8444-555555555555';
const otherId = '22222222-3333-4444-8555-666666666666';
function gate() {
  let resolve!: () => void;
  const promise = new Promise<void>(yes => { resolve = yes; });
  return { promise, resolve };
}
async function fixture(t: TestContext) {
  const directory = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-resume-lifecycle-')));
  const settings = new SettingsStore(join(directory, 'settings'));
  const ref = { projectPath: directory, sessionId };
  settings.addProject(directory);
  await settings.conversationSpaces.setMode('code');
  await settings.conversationSpaces.addProject(directory, 'code');
  for (const id of [sessionId, otherId]) await settings.conversationSpaces.register({ ...ref, sessionId: id, projectId: directory, mode: 'code' });
  settings.setActiveSession({ ...ref, sessionId: otherId });
  const path = join(directory, `${sessionId}.jsonl`);
  writeFileSync(path, '{}\n');
  const row = { uuid: sessionId, path, mode: 'code', title: 'Saved', message_count: 1, modified_rfc3339: '', empty_session: false };
  let runtime: any;
  const catalog = {
    select: async () => row, captureMetadataSource: async () => 'isolated-receipt',
    mutateMetadataSource: async () => {}, list: async () => ({ sessions: [row] }), invalidate() {},
  };
  const bridge = {
    registerWindow() {}, registerIpc() {}, get: () => runtime, isProjectClosing: () => false,
    activateSession() {}, retainBackgroundSession: () => () => {}, closeSession: async () => { runtime = undefined; },
    openSession: async () => { runtime = { projectPath: directory, sessionId, sessionMode: 'code', connectionState: { status: 'connected' } }; return runtime; },
    newSession: async () => ({ ...ref, sessionId: otherId }),
  };
  const handlers = new Map<string, Function>();
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), catalog as any,
    { handle: (name: string, fn: Function) => handlers.set(name, fn), removeHandler() {} } as any);
  (host as any).bootstrap = () => ({ settings: settings.getPublic() });
  host.attachScheduled({ manage: async () => [], isControllerSession: () => false, dispose() {} } as any);
  const frame = { url: 'http://127.0.0.1:4242' };
  const sender = { mainFrame: frame, isDestroyed: () => false, once() {}, send() {} };
  host.registerWindow(sender as any, frame.url); host.registerIpc();
  t.after(() => { host.dispose(); rmSync(directory, { recursive: true, force: true }); });
  return { host, settings, ref, path, catalog, bridge, handlers, event: { sender, senderFrame: frame },
    setRuntime: (value: any) => { runtime = value; } };
}

test('archive revokes already admitted cold opens without reopening the archived chat', async t => {
  const f = await fixture(t);
  const selection = { scope: 'current', transcriptPath: f.path } as const;
  const source = await f.handlers.get(CH_SESSION_METADATA_CAPTURE)!(f.event, f.ref.projectPath, sessionId, selection);
  const entered = gate(), release = gate();
  const select = f.catalog.select;
  f.catalog.select = async () => { entered.resolve(); await release.promise; return select(); };
  const opening = f.host.openSessionAndActivate(f.ref, selection);
  await entered.promise;
  const queuedOpening = f.host.openSessionAndActivate(f.ref, selection);
  await f.handlers.get(CH_SESSION_ARCHIVE)!(f.event, f.ref.projectPath, sessionId, source);
  release.resolve();
  await Promise.all([opening, queuedOpening]);
  assert.equal(f.settings.isSessionArchived(f.ref), true);
  assert.equal(f.settings.getPublic().activeSession?.sessionId, otherId);
  assert.equal(f.bridge.get(), undefined, 'a revoked preparation must not create a runtime');
  // An explicit later restore is a new intent and may reopen the chat.
  await f.host.openSessionAndActivate(f.ref, selection);
  assert.equal(f.settings.isSessionArchived(f.ref), false);
  assert.equal(f.settings.getPublic().activeSession?.sessionId, sessionId);
});

test('a notified resume waiting for its engine does not block New Chat', async t => {
  const f = await fixture(t);
  const entered = gate(), release = gate();
  const runtime = { projectPath: f.ref.projectPath, sessionId, sessionMode: 'code', connectionState: { status: 'connected' }, getWorkspaceState: async () => ({ cwd: f.ref.projectPath, transcript_path: f.path }) };
  f.setRuntime(runtime);
  f.bridge.openSession = async () => { entered.resolve(); await release.promise; return runtime; };
  let created = false;
  f.bridge.newSession = async () => { created = true; return { ...f.ref, sessionId: otherId }; };
  const notified = f.host.showNotifiedSession(f.ref);
  await entered.promise;
  const creation = f.handlers.get(CH_SESSION_NEW)!(f.event, f.ref.projectPath);
  await new Promise<void>(resolve => setImmediate(resolve));
  assert.equal(created, true, 'unrelated creation must enter while notified engine is still parked');
  release.resolve();
  await Promise.all([notified, creation]);
  assert.equal(f.settings.getPublic().activeSession?.sessionId, otherId);
});

test('archive revokes an engine preparation before metadata commit and a later open waits for archival', async t => {
  const f = await fixture(t);
  const selection = { scope: 'current', transcriptPath: f.path } as const;
  const source = await f.handlers.get(CH_SESSION_METADATA_CAPTURE)!(f.event, f.ref.projectPath, sessionId, selection);
  const engineEntered = gate(), engineRelease = gate(), archiveEntered = gate(), archiveRelease = gate();
  const open = f.bridge.openSession;
  let opens = 0;
  f.bridge.openSession = async () => {
    if (++opens === 1) { engineEntered.resolve(); await engineRelease.promise; }
    return open();
  };
  const opening = f.host.openSessionAndActivate(f.ref, selection);
  await engineEntered.promise;
  f.catalog.mutateMetadataSource = async () => { archiveEntered.resolve(); await archiveRelease.promise; };
  const archival = f.handlers.get(CH_SESSION_ARCHIVE)!(f.event, f.ref.projectPath, sessionId, source);
  await archiveEntered.promise;
  const laterOpen = f.host.openSessionAndActivate(f.ref, selection);
  engineRelease.resolve();
  await opening;
  assert.equal(f.bridge.get(), undefined, 'the revoked new engine is released even before archive commits');
  assert.equal(opens, 1, 'a newer open must await the ongoing archive');
  archiveRelease.resolve();
  await Promise.all([archival, laterOpen]);
  assert.equal(opens, 2);
  assert.equal(f.settings.isSessionArchived(f.ref), false, 'the later explicit restore runs after archive');
  assert.equal(f.settings.getPublic().activeSession?.sessionId, sessionId);
});

test('archive revokes a saved mode restore admitted behind the navigation lane', async t => {
  const f = await fixture(t);
  const owner = f.settings.conversationSpaces.find(f.ref)!;
  await f.settings.conversationSpaces.setNavigation('code', owner.projectId, owner);
  await f.settings.conversationSpaces.setMode('chat');
  const selection = { scope: 'current', transcriptPath: f.path } as const;
  const source = await f.handlers.get(CH_SESSION_METADATA_CAPTURE)!(f.event, f.ref.projectPath, sessionId, selection);
  const release = gate();
  const blocked = (f.host as any).enqueueNavigation(() => release.promise);
  const modeRestore = f.handlers.get(CH_CONVERSATION_SPACE)!(f.event, 'code');
  const archival = f.handlers.get(CH_SESSION_ARCHIVE)!(f.event, f.ref.projectPath, sessionId, source);
  release.resolve();
  await Promise.all([blocked, modeRestore, archival]);
  assert.equal(f.settings.isSessionArchived(f.ref), true);
  assert.equal(f.bridge.get(), undefined);
  assert.equal(f.settings.conversationSpaces.snapshot().activeMode, 'chat');
});

test('archive revokes an older project restore even when its target UUID is discovered later', async t => {
  const f = await fixture(t);
  const selection = { scope: 'current', transcriptPath: f.path } as const;
  const source = await f.handlers.get(CH_SESSION_METADATA_CAPTURE)!(f.event, f.ref.projectPath, sessionId, selection);
  const catalogEntered = gate(), catalogRelease = gate(), archiveEntered = gate(), archiveRelease = gate();
  const list = f.catalog.list;
  let reads = 0;
  f.catalog.list = async () => {
    if (++reads === 1) { catalogEntered.resolve(); await catalogRelease.promise; }
    return list();
  };
  const restoring = assert.rejects(f.host.restoreProjectSession(f.ref.projectPath), /no longer active/);
  await catalogEntered.promise;
  f.catalog.mutateMetadataSource = async () => { archiveEntered.resolve(); await archiveRelease.promise; };
  const archival = f.handlers.get(CH_SESSION_ARCHIVE)!(f.event, f.ref.projectPath, sessionId, source);
  await archiveEntered.promise;
  catalogRelease.resolve();
  await new Promise<void>(resolve => setImmediate(resolve));
  archiveRelease.resolve();
  await Promise.all([restoring, archival]);
  assert.equal(f.settings.isSessionArchived(f.ref), true);
  assert.equal(f.bridge.get(), undefined);
  assert.equal(f.settings.getPublic().activeSession?.sessionId, otherId);
});
