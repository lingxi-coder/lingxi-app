import assert from 'node:assert/strict';
import { mkdtempSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test, type TestContext } from 'node:test';
import type { MessageDto } from '@lingxi/bridge-client';
import { SettingsStore } from '../src/main/settings';
import { HostController, CH_SESSION_ARCHIVE, CH_SESSION_METADATA_CAPTURE, CH_SESSION_NEW, CH_SESSION_OPEN } from '../src/main/host';
import type { ResumePreviewListener } from '../src/main/sessionRuntimeManager.js';
import { CH_SESSION_RESUME_PREVIEW } from '../src/shared/sessionHistory.js';
import { CH_CONVERSATION_SPACE } from '../src/shared/conversationSpaces';
import { DiagnosticBuffer } from '../src/main/host-utils';

const sessionId = '11111111-2222-4333-8444-555555555555';
const otherId = '22222222-3333-4444-8555-666666666666';
const previewMessages: MessageDto[] = [{ role: 'user', blocks: [{ type: 'text', text: 'Saved history' }] }];
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
  const sent: Array<{ channel: string; payload: unknown }> = [];
  const sender = { mainFrame: frame, isDestroyed: () => false, once() {}, send(channel: string, payload: unknown) { sent.push({ channel, payload }); } };
  host.registerWindow(sender as any, frame.url); host.registerIpc();
  t.after(() => { host.dispose(); rmSync(directory, { recursive: true, force: true }); });
  return { host, settings, ref, path, catalog, bridge, handlers, sent, event: { sender, senderFrame: frame },
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

test('an exact picker open previews only to its initiating document before committing navigation', async t => {
  const f = await fixture(t);
  const entered = gate(), finish = gate();
  const selection = { scope: 'current', transcriptPath: f.path } as const;
  const open = f.bridge.openSession;
  let preview: ResumePreviewListener | undefined;
  f.bridge.openSession = async (...args: unknown[]) => {
    const runtime = await open();
    preview = args[7] as ResumePreviewListener;
    entered.resolve();
    await finish.promise;
    return runtime;
  };
  const otherMessages: unknown[] = [];
  const otherSender = { ...f.event.sender, send(_channel: string, payload: unknown) { otherMessages.push(payload); } };
  f.host.registerWindow(otherSender as any, f.event.senderFrame.url);
  const handler = f.handlers.get(CH_SESSION_OPEN)!;
  await assert.rejects(handler(f.event, f.ref.projectPath, sessionId, selection, undefined, undefined, undefined, 'invalid request'), /invalid session preview request/);
  const requestId = 'picker-attempt-1';
  const opening = handler(f.event, f.ref.projectPath, sessionId, selection, undefined, undefined, undefined, requestId);
  try {
    await entered.promise;
    assert.equal(f.settings.getPublic().activeSession?.sessionId, otherId);
    preview?.({ type: 'session_resumed', session_id: sessionId, mode: 'code', messages: previewMessages });
    assert.deepEqual(f.sent, [{ channel: CH_SESSION_RESUME_PREVIEW,
      payload: { requestId, session: { ...f.ref, mode: 'code' }, messages: previewMessages } }]);
    assert.deepEqual(otherMessages, []);
    assert.equal(f.settings.getPublic().activeSession?.sessionId, otherId);
  } finally { finish.resolve(); await opening; }
  assert.equal(f.settings.getPublic().activeSession?.sessionId, sessionId);
});

test('a plain cold open previews its remembered exact transcript and original catalog scope', async t => {
  const f = await fixture(t);
  const catalogProjectPath = join(f.ref.projectPath, 'settings');
  f.settings.rememberSessionTranscriptSelection(f.ref, { catalogProjectPath, scope: 'all', transcriptPath: f.path });
  const select = f.catalog.select;
  const reads: unknown[][] = [];
  f.catalog.select = async (...args: unknown[]) => { reads.push(args); return select(); };
  const open = f.bridge.openSession;
  f.bridge.openSession = async (...args: unknown[]) => {
    assert.equal(args[4], f.path);
    const runtime = await open();
    const preview = args[7] as ResumePreviewListener | undefined;
    assert.ok(preview);
    preview({ type: 'session_resumed', session_id: sessionId, mode: 'code', messages: previewMessages });
    assert.equal(f.settings.getPublic().activeSession?.sessionId, otherId);
    return runtime;
  };

  await f.handlers.get(CH_SESSION_OPEN)!(f.event, f.ref.projectPath, sessionId,
    undefined, undefined, undefined, undefined, 'saved-source-open');

  assert.deepEqual(reads, [[catalogProjectPath, sessionId, { scope: 'all', transcriptPath: f.path }]]);
  assert.deepEqual(f.sent, [{ channel: CH_SESSION_RESUME_PREVIEW,
    payload: { requestId: 'saved-source-open', session: { ...f.ref, mode: 'code' }, messages: previewMessages } }]);
  assert.equal(f.settings.getPublic().activeSession?.sessionId, sessionId);
});

for (const timing of ['before capture', 'before delivery']) {
  test(`a plain cold open fences changed remembered scope ${timing}`, async t => {
    const f = await fixture(t);
    const remembered = { catalogProjectPath: f.ref.projectPath, scope: 'all', transcriptPath: f.path } as const;
    f.settings.rememberSessionTranscriptSelection(f.ref, remembered);
    const changeScope = () => {
      f.settings.commitSessionRelocation(f.ref, f.ref.projectPath, { ...remembered, scope: 'repository' });
      f.settings.setActiveSession({ ...f.ref, sessionId: otherId });
    };
    if (timing === 'before capture') {
      const select = f.catalog.select;
      f.catalog.select = async () => { changeScope(); return select(); };
    } else {
      const open = f.bridge.openSession;
      f.bridge.openSession = async (...args: unknown[]) => {
        const runtime = await open();
        const preview = args[7] as ResumePreviewListener | undefined;
        assert.ok(preview);
        changeScope();
        preview({ type: 'session_resumed', session_id: sessionId, mode: 'code', messages: previewMessages });
        return runtime;
      };
    }

    const opening = f.handlers.get(CH_SESSION_OPEN)!(f.event, f.ref.projectPath, sessionId,
      undefined, undefined, undefined, undefined, 'changed-saved-scope');
    if (timing === 'before capture') {
      await assert.rejects(opening, /saved session selection changed/);
      assert.equal(f.bridge.get(), undefined);
    } else await opening;
    assert.equal(f.sent.some(({ channel }) => channel === CH_SESSION_RESUME_PREVIEW), false);
  });
}

test('a picker preview from a superseded navigation is dropped', async t => {
  const f = await fixture(t);
  const entered = gate(), finish = gate();
  const open = f.bridge.openSession;
  let preview: ResumePreviewListener | undefined;
  f.bridge.openSession = async (...args: unknown[]) => {
    const runtime = await open();
    preview = args[7] as ResumePreviewListener;
    entered.resolve();
    await finish.promise;
    return runtime;
  };
  const opening = f.handlers.get(CH_SESSION_OPEN)!(f.event, f.ref.projectPath, sessionId,
    { scope: 'current', transcriptPath: f.path }, undefined, undefined, undefined, 'superseded-picker');
  try {
    await entered.promise;
    await f.handlers.get(CH_SESSION_NEW)!(f.event, f.ref.projectPath);
    assert.ok(preview, 'the exact open requested a preview from its runtime');
    preview({ type: 'session_resumed', session_id: sessionId, mode: 'code', messages: previewMessages });
    assert.equal(f.sent.some(({ channel }) => channel === CH_SESSION_RESUME_PREVIEW), false);
  } finally { finish.resolve(); await opening; }
  assert.equal(f.settings.getPublic().activeSession?.sessionId, otherId);
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

test('a cold notification validates its saved picker source once before switching space', async t => {
  const f = await fixture(t);
  await f.settings.conversationSpaces.setMode('chat');
  const catalogProjectPath = join(f.ref.projectPath, 'settings');
  f.settings.rememberSessionTranscriptSelection(f.ref, { catalogProjectPath, scope: 'all', transcriptPath: f.path });
  const select = f.catalog.select;
  const reads: unknown[][] = [];
  f.catalog.select = async (...args: unknown[]) => {
    reads.push(args);
    assert.equal(f.settings.conversationSpaces.snapshot().activeMode, 'chat');
    return select();
  };

  await f.host.showNotifiedSession(f.ref);

  assert.deepEqual(reads, [[catalogProjectPath, sessionId, { scope: 'all', transcriptPath: f.path }]]);
  assert.equal(f.settings.conversationSpaces.snapshot().activeMode, 'code');
  assert.equal(f.settings.getPublic().activeSession?.sessionId, sessionId);
});

test('a cold notification rolls back when its saved picker source changes during validation', async t => {
  const f = await fixture(t);
  await f.settings.conversationSpaces.setMode('chat');
  const source = { catalogProjectPath: f.ref.projectPath, scope: 'all', transcriptPath: f.path } as const;
  f.settings.rememberSessionTranscriptSelection(f.ref, source);
  const select = f.catalog.select;
  f.catalog.select = async () => {
    f.settings.commitSessionRelocation(f.ref, f.ref.projectPath, { ...source, scope: 'repository' });
    f.settings.setActiveSession({ ...f.ref, sessionId: otherId });
    return select();
  };

  await assert.rejects(f.host.showNotifiedSession(f.ref), /saved session selection changed/);
  assert.equal(f.settings.conversationSpaces.snapshot().activeMode, 'chat');
  assert.equal(f.settings.getPublic().activeSession?.sessionId, otherId);
  assert.equal(f.bridge.get(), undefined);
});

test('a notification rejects a changed workspace owner while switching space', async t => {
  const f = await fixture(t);
  const spaces = f.settings.conversationSpaces;
  await spaces.setMode('chat');
  const saved = { catalogProjectPath: f.ref.projectPath, scope: 'current', transcriptPath: f.path } as const;
  f.settings.rememberSessionTranscriptSelection(f.ref, saved);
  const setMode = spaces.setMode.bind(spaces);
  spaces.setMode = async mode => {
    if (mode === 'code') await spaces.relocate(f.ref, join(f.ref.projectPath, 'settings'));
    await setMode(mode);
  };

  await assert.rejects(f.host.showNotifiedSession(f.ref), /notification session owner changed/);
  assert.equal(f.settings.getSessionTranscriptSelection(f.ref)?.transcriptPath, f.path);
  assert.equal(spaces.snapshot().activeMode, 'chat');
  assert.equal(f.bridge.get(), undefined);
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
