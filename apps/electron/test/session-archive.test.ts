import { test } from 'node:test';
import assert from 'node:assert/strict';
import { lstatSync, mkdtempSync, realpathSync, renameSync, rmSync, writeFileSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { SettingsStore } from '../src/main/settings';
import { CH_SESSION_ARCHIVE, CH_SESSION_ARCHIVE_PREFLIGHT, CH_SESSION_METADATA_CAPTURE, CH_SESSION_METADATA_RELEASE, HostController } from '../src/main/host';
import { DiagnosticBuffer } from '../src/main/host-utils';

const sessionId = '11111111-2222-4333-8444-555555555555';
test('archive persists metadata and unpins without touching the transcript; restore clears marker', () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-archive-'));
  const projectPath = realpathSync.native(dir);
  try {
    const settings = new SettingsStore(dir);
    const ref = { projectPath, sessionId };
    settings.addProject(projectPath);
    settings.conversationSpaces.register({ ...ref, projectId: projectPath, mode: 'code' });
    settings.setActiveSession(ref);
    settings.setSessionPinned({ ...ref, title: 'Saved chat', pinnedAt: new Date().toISOString() }, true);
    const transcript = join(dir, 'transcript.jsonl');
    writeFileSync(transcript, 'original transcript');
    settings.setSessionArchived(ref, true, 'Saved chat');
    const reloaded = new SettingsStore(dir);
    assert.equal(reloaded.isSessionArchived(ref), true);
    assert.equal(reloaded.getPublic().archivedSessions?.[0]?.title, 'Saved chat');
    assert.equal(reloaded.getPublic().activeSession, undefined);
    assert.equal(reloaded.getPublic().pinnedSessions.length, 0);
    assert.equal(readFileSync(transcript, 'utf8'), 'original transcript');
    assert.throws(() => reloaded.setSessionPinned({ ...ref, title: 'Saved chat', pinnedAt: '' }, true), /Restore/);
    reloaded.setSessionArchived(ref, false);
    assert.equal(new SettingsStore(dir).isSessionArchived(ref), false);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('archive pauses only execution-dependent schedules before persistence, and leaves chat accessible on failure', async () => {
  for (const fail of [false, true]) {
    const dir = mkdtempSync(join(tmpdir(), 'lingxi-archive-host-'));
    const projectPath = realpathSync.native(dir);
    try {
      const settings = new SettingsStore(dir);
      settings.addProject(projectPath);
      const ref = { projectPath, sessionId };
      const path = join(projectPath, `${sessionId}.jsonl`);
      const calls: string[] = [];
      const runtime = {
        projectPath, sessionMode: 'code', connectionState: { status: 'connected' },
        beginArchive: () => { calls.push('lock'); return () => { calls.push('unlock'); }; },
      };
      const scheduled = {
        manage: async (scope: string, request: { action: string; id?: string; automation?: { statusReason?: string } }) => {
          assert.equal(scope, projectPath);
          if (request.action !== 'list') {
            assert.equal(request.action, 'pause', 'lifecycle changes must not use partial updates');
            assert.match(request.automation?.statusReason ?? '', /archived/);
          }
          calls.push(request.action + (request.id ? ':' + request.id : ''));
          assert.equal(settings.isSessionArchived(ref), false, 'cleanup must precede archive');
          if (fail && request.id === 'second') throw new Error('fixture stop failure');
          return [{ id: 'first', automation: { status: 'active', runMode: 'selected_session', targetSessionId: sessionId, targetSessionTranscriptPath: path, targetSessionMode: 'code', targetSessionScopeKey: projectPath } }, { id: 'second', automation: { status: 'active', runMode: 'task_session', ownedSessionId: sessionId, ownedSessionTranscriptPath: path, ownedSessionMode: 'code', ownedSessionScopeKey: projectPath } }, { id: 'creator-only', session_id: sessionId }, { id: 'sibling', automation: { status: 'active', runMode: 'selected_session', targetSessionId: sessionId, targetSessionTranscriptPath: `${path}.other`, targetSessionMode: 'code', targetSessionScopeKey: projectPath } }, { id: 'unrelated', automation: { targetSessionId: 'another-session' } }];
        },
      };
      const bridge = {
        get: () => runtime,
        isProjectClosing: () => false,
        withBackgroundSession: async (_ref: unknown, _empty: unknown, _model: unknown, operation: (runtime: any) => Promise<unknown>) => operation(runtime),
        closeSession: async () => { assert.equal(settings.isSessionArchived(ref), true); calls.push('close'); },
      };
      const row = { uuid: sessionId, title: 'Saved chat', modified_rfc3339: '', message_count: 1, mode: 'code', path: '', empty_session: false };
      const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), { list: async () => ({ sessions: [row] }), invalidate: () => undefined } as any);
      host.attachScheduled(scheduled as any);
      const captured = { requested: ref, source: { ref: { ...ref, mode: 'code' }, row, transcriptPath: path }, runtime,
        owner: { ...ref, mode: 'code', projectId: projectPath },
        selection: { scope: 'current', transcriptPath: path, sourceToken: sessionId }, apply: async () => {}, release() {} };
      // Isolate orchestration from unrelated platform bootstrap services.
      (host as any).bootstrap = () => ({ settings: settings.getPublic() });
      if (fail) {
        await assert.rejects((host as any).archiveSessionInternal(ref, captured), /Chat was not archived.*1 scheduled task.*fixture stop failure/);
        assert.equal(settings.isSessionArchived(ref), false);
        assert.ok(!calls.includes('close'));
      } else {
        await (host as any).archiveSessionInternal(ref, captured);
        assert.equal(settings.isSessionArchived(ref), true);
        assert.equal((await (host as any).loadProjectSessions(projectPath)).sessions.length, 0);
      }
      assert.ok(!calls.some((call) => call.startsWith('delete:')));
      assert.ok(!calls.includes('pause:creator-only'));
      assert.ok(!calls.includes('pause:unrelated'));
      assert.ok(!calls.includes('pause:sibling'));
      assert.ok(!calls.includes('delete:prefixed-unrelated'));
      assert.ok(!calls.includes('delete:double-prefixed'));
      assert.deepEqual(calls.slice(0, 4), ['lock', 'list', 'pause:first', 'pause:second']);
      assert.equal(calls.at(-1), 'unlock');
    } finally { rmSync(dir, { recursive: true, force: true }); }
  }
});

test('runtime archive lock rejects active work and blocks racing new prompts and commands', async () => {
  const { SessionRuntime } = await import('../src/main/bridge');
  const runtime = new SessionRuntime({ sessionId, projectPath: '/fixture' } as any);
  (runtime as any).activeTurn = true;
  assert.throws(() => runtime.beginArchive(), /active work/);
  (runtime as any).activeTurn = false;
  (runtime as any).pendingPermissionIds.set(1, { request_id: 1 });
  assert.throws(() => runtime.beginArchive(), /active work/);
  (runtime as any).pendingPermissionIds.clear();
  const release = runtime.beginArchive();
  assert.throws(() => runtime.sendPrompt('race'), /being archived/);
  await assert.rejects(runtime.dispatchCommand({ type: 'cron_manage', request_id: 'race', request: { action: 'list' } }), /being archived/);
  release();
  const releaseAgain = runtime.beginArchive();
  releaseAgain();
});

test('cold archive preflight retains the exact source and uses its project scheduler without starting the chat', async () => {
  const directory = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-archive-capture-')));
  const settings = new SettingsStore(join(directory, 'settings'));
  settings.addProject(directory);
  settings.conversationSpaces.register({ projectPath: directory, projectId: directory, sessionId, mode: 'code' });
  const path = join(directory, `${sessionId}.jsonl`);
  const content = '{"type":"user"}\n';
  writeFileSync(path, content);
  const row = { uuid: sessionId, path, mode: 'code', title: 'Saved', message_count: 1, modified_rfc3339: '', empty_session: false };
  const calls: string[] = [];
  let identity: ReturnType<typeof lstatSync> | undefined;
  let activeRuntime: object | undefined;
  let blockVerify: (() => Promise<void>) | undefined;
  const catalog = {
    select: async (_project: string, _id: string, selection: { transcriptPath: string }) => selection.transcriptPath === path ? row : undefined,
    captureMetadataSource: async (_project: string, _id: string, selected: string) => {
      assert.equal(selected, path); identity = lstatSync(path); return 'opaque-receipt';
    },
    mutateMetadataSource: async (_project: string, receipt: string, action: string) => {
      assert.equal(receipt, 'opaque-receipt'); assert.equal(action, 'verify');
      const current = lstatSync(path);
      if (current.dev !== identity?.dev || current.ino !== identity?.ino) throw new Error('captured source changed');
      await blockVerify?.();
    },
    list: async () => ({ sessions: [row] }), invalidate() {},
  };
  const bridge = { registerWindow() {}, registerIpc() {}, get: () => activeRuntime, isProjectClosing: () => false,
    withBackgroundSession: () => { throw new Error('cold archive must not start the chat'); },
    closeSession: async () => { throw new Error('must not close a new runtime'); } };
  const handlers = new Map<string, (...args: any[]) => any>();
  const ipc = { handle: (name: string, handler: (...args: any[]) => any) => handlers.set(name, handler), removeHandler: (name: string) => handlers.delete(name) };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), catalog as any, ipc as any);
  const frame = { url: 'http://127.0.0.1:4242' };
  const sender = { mainFrame: frame, isDestroyed: () => false, once() {}, send() {} };
  host.registerWindow(sender as any, frame.url); host.registerIpc();
  const event = { sender, senderFrame: frame };
  host.attachScheduled({ dispose() {}, manage: async (scope: string, request: { action: string; id?: string }) => {
    assert.equal(scope, directory); calls.push(`${scope}:${request.action}${request.id ? `:${request.id}` : ''}`);
    return [{ id: 'correct', automation: { status: 'active', runMode: 'selected_session', targetSessionId: sessionId,
      targetSessionTranscriptPath: path, targetSessionMode: 'code', targetSessionScopeKey: directory } },
    { id: 'sibling', automation: { status: 'active', runMode: 'selected_session', targetSessionId: sessionId,
      targetSessionTranscriptPath: `${path}.other`, targetSessionMode: 'code', targetSessionScopeKey: directory } }];
  } } as any);
  (host as any).bootstrap = () => ({ settings: settings.getPublic() });
  try {
    let selection = await handlers.get(CH_SESSION_METADATA_CAPTURE)!(event, directory, sessionId, { scope: 'current', transcriptPath: path });
    const jobs = await handlers.get(CH_SESSION_ARCHIVE_PREFLIGHT)!(event, directory, sessionId, selection);
    assert.deepEqual(jobs.map((job: { id: string }) => job.id), ['correct']);
    let entered!: () => void, resume!: () => void;
    const started = new Promise<void>(resolve => { entered = resolve; });
    const waiting = new Promise<void>(resolve => { resume = resolve; });
    blockVerify = async () => { entered(); await waiting; };
    const racingArchive = handlers.get(CH_SESSION_ARCHIVE)!(event, directory, sessionId, selection);
    await started;
    activeRuntime = { projectPath: directory, sessionMode: 'code' };
    resume();
    await assert.rejects(racingArchive, /runtime owner changed/);
    assert.equal(settings.isSessionArchived({ projectPath: directory, sessionId }), false);
    activeRuntime = undefined; blockVerify = undefined;

    let finalVerifyEntered!: () => void, finishVerify!: () => void;
    const finalVerifyStarted = new Promise<void>(resolve => { finalVerifyEntered = resolve; });
    const finalVerifyWaiting = new Promise<void>(resolve => { finishVerify = resolve; });
    let verificationCount = 0;
    blockVerify = async () => {
      if (++verificationCount === 4) { finalVerifyEntered(); await finalVerifyWaiting; }
    };
    const cancelledArchive = handlers.get(CH_SESSION_ARCHIVE)!(event, directory, sessionId, selection);
    await finalVerifyStarted;
    await handlers.get(CH_SESSION_METADATA_RELEASE)!(event, selection);
    finishVerify();
    await assert.rejects(cancelledArchive, /released/);
    assert.equal(settings.isSessionArchived({ projectPath: directory, sessionId }), false);
    blockVerify = undefined;
    selection = await handlers.get(CH_SESSION_METADATA_CAPTURE)!(event, directory, sessionId, { scope: 'current', transcriptPath: path });

    const old = `${path}.old`;
    renameSync(path, old); writeFileSync(path, content);
    await assert.rejects(handlers.get(CH_SESSION_ARCHIVE)!(event, directory, sessionId, selection), /captured source changed/);
    assert.equal(settings.isSessionArchived({ projectPath: directory, sessionId }), false);
    assert.equal(readFileSync(path, 'utf8'), content); assert.equal(readFileSync(old, 'utf8'), content);
    rmSync(path); renameSync(old, path);
    await handlers.get(CH_SESSION_ARCHIVE)!(event, directory, sessionId, selection);
    assert.equal(settings.isSessionArchived({ projectPath: directory, sessionId }), true);
    assert.ok(calls.includes(`${directory}:pause:correct`));
    assert.ok(!calls.includes(`${directory}:pause:sibling`));
    await assert.rejects(handlers.get(CH_SESSION_ARCHIVE)!(event, directory, sessionId, selection), /expired or changed/);
    await handlers.get(CH_SESSION_METADATA_RELEASE)!(event, selection);
  } finally { host.dispose(); rmSync(directory, { recursive: true, force: true }); }
});

test('archive does not copy the title of a same-UUID sibling source', async () => {
  const directory = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-archive-title-')));
  const settings = new SettingsStore(directory); settings.addProject(directory);
  const ref = { projectPath: directory, sessionId };
  const path = join(directory, `${sessionId}.jsonl`);
  const bridge = { get: () => undefined, isProjectClosing: () => false };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), { list: async () => ({ sessions: [] }), invalidate() {} } as any);
  host.attachScheduled({ dispose() {}, manage: async () => [] } as any);
  (host as any).catalogs.set(directory, { sessions: [{ uuid: sessionId, path: `${path}.other`, mode: 'code', title: 'Sibling title' }] });
  (host as any).bootstrap = () => ({ settings: settings.getPublic() });
  const captured = { requested: ref, source: { ref: { ...ref, mode: 'code' }, transcriptPath: path },
    owner: { ...ref, mode: 'code', projectId: directory }, selection: { scope: 'current', transcriptPath: path, sourceToken: sessionId },
    apply: async () => {}, release() {} };
  try {
    await (host as any).archiveSessionInternal(ref, captured);
    assert.equal(settings.getPublic().archivedSessions?.[0]?.title, undefined);
  } finally { host.dispose(); rmSync(directory, { recursive: true, force: true }); }
});

test('Global archive uses the Global scheduler and fails closed when it is unavailable', async () => {
  const directory = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-archive-global-')));
  const settings = new SettingsStore(directory);
  const projectPath = settings.conversationSpaces.globalWorkspaces.chat;
  const ref = { projectPath, sessionId };
  settings.conversationSpaces.register({ ...ref, projectId: null, mode: 'chat' });
  const path = join(projectPath, `${sessionId}.jsonl`);
  const captured = { requested: ref, source: { ref: { ...ref, mode: 'chat' }, transcriptPath: path },
    owner: { ...ref, mode: 'chat', projectId: null },
    selection: { scope: 'current', transcriptPath: path, sourceToken: sessionId }, apply: async () => {}, release() {} };
  const bridge = { get: () => undefined, isProjectClosing: () => false, closeSession: async () => {},
    withBackgroundSession: () => { throw new Error('must not start the chat'); } };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), { list: async () => ({ sessions: [] }), invalidate() {} } as any);
  (host as any).bootstrap = () => ({ settings: settings.getPublic() });
  try {
    await assert.rejects((host as any).archiveSessionInternal(ref, captured), /Scheduled tasks are unavailable/);
    assert.equal(settings.isSessionArchived(ref), false);
    const calls: string[] = [];
    host.attachScheduled({ dispose() {}, manage: async (scope: string, request: { action: string; id?: string }, mode: string) => {
      calls.push(`${scope}:${request.action}:${mode}`);
      return [{ id: 'global-task', automation: { status: 'active', runMode: 'task_session', ownedSessionId: sessionId,
        ownedSessionTranscriptPath: path, ownedSessionMode: 'chat', ownedSessionScopeKey: 'global' } }];
    } } as any);
    await (host as any).archiveSessionInternal(ref, captured);
    assert.deepEqual(calls, ['global:list:chat', 'global:pause:chat']);
    assert.equal(settings.isSessionArchived(ref), true);
  } finally { host.dispose(); rmSync(directory, { recursive: true, force: true }); }
});

test('opening an archived chat restores it only after the existing session resumes successfully', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-archive-restore-'));
  const projectPath = realpathSync.native(dir);
  try {
    const settings = new SettingsStore(dir);
    const ref = { projectPath, sessionId };
    settings.addProject(projectPath);
    settings.conversationSpaces.register({ ...ref, projectId: projectPath, mode: 'code' });
    settings.setSessionArchived(ref, true, 'Saved chat');
    let fail = true;
    const row = { uuid: sessionId, mode: 'code', path: join(projectPath, `${sessionId}.jsonl`), empty_session: false };
    const bridge = { get: () => undefined, isProjectClosing: () => false, openSession: async () => { if (fail) throw new Error('resume failed'); } };
    const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), { find: async () => row, select: async () => row, list: async () => ({ sessions: [row] }), invalidate: () => undefined } as any);
    (host as any).bootstrap = () => ({ settings: settings.getPublic() });
    await assert.rejects((host as any).openSessionAndActivateInternal(ref, { scope: 'current', transcriptPath: row.path }), /resume failed/);
    assert.equal(settings.isSessionArchived(ref), true);
    fail = false;
    await (host as any).openSessionAndActivateInternal(ref, { scope: 'current', transcriptPath: row.path });
    assert.equal(new SettingsStore(dir).isSessionArchived(ref), false);
    assert.equal(settings.getPublic().activeSession?.sessionId, sessionId);
    assert.equal((host as any).catalogs.get(projectPath).sessions[0].uuid, sessionId);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('failure to create the replacement keeps the durable archive and cleared active selection coherent', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-archive-next-'));
  const projectPath = realpathSync.native(dir);
  try {
    const settings = new SettingsStore(dir);
    const ref = { projectPath, sessionId };
    settings.addProject(projectPath);
    settings.conversationSpaces.register({ ...ref, projectId: projectPath, mode: 'code' });
    settings.setActiveSession(ref);
    settings.conversationSpaces.setMode('code');
    const runtime = { projectPath, sessionMode: 'code', connectionState: { status: 'connected' }, beginArchive: () => () => {}, manageCron: async () => [] };
    const bridge = { get: () => runtime, isProjectClosing: () => false, withBackgroundSession: async (_ref: unknown, _empty: unknown, _model: unknown, operation: (runtime: any) => Promise<unknown>) => operation(runtime), closeSession: async () => {}, newSession: async () => { throw new Error('fixture launch failure'); } };
    const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), { list: async () => ({ sessions: [] }), invalidate: () => undefined } as any);
    host.attachScheduled({ manage: async () => [] } as any);
    const path = join(projectPath, `${sessionId}.jsonl`);
    const captured = { requested: ref, source: { ref: { ...ref, mode: 'code' }, transcriptPath: path }, runtime,
      owner: { ...ref, mode: 'code', projectId: projectPath },
      selection: { scope: 'current', transcriptPath: path, sourceToken: sessionId }, apply: async () => {}, release() {} };
    await assert.rejects((host as any).archiveSessionInternal(ref, captured), /Chat was archived.*fixture launch failure/);
    assert.equal(settings.getPublic().activeSession, undefined);
    assert.equal(new SettingsStore(dir).isSessionArchived(ref), true);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('removing a project pauses active schedules with a reason before removing its scope', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-remove-scheduled-project-'));
  const project = realpathSync.native(dir);
  try {
    const settings = new SettingsStore(dir);
    settings.addProject(project);
    const calls: string[] = [];
    const bridge = {
      hasActiveWork: () => false,
      isProjectClosing: () => false,
      closeProject: async () => { calls.push('close'); assert.equal(settings.hasProject(project), true); },
    };
    const host = new HostController(settings, bridge as any, new DiagnosticBuffer());
    (host as any).bootstrap = () => ({ settings: settings.getPublic() });
    (host as any).scheduled = { manage: async (scope: string, request: { action: string; id?: string; automation?: { statusReason?: string } }) => {
      assert.equal(scope, project);
      assert.equal(settings.hasProject(project), true);
      calls.push(request.action);
      if (request.action === 'list') return [{ id: 'active', automation: { status: 'active' } }, { id: 'completed', automation: { status: 'completed' } }];
      assert.equal(request.action, 'pause');
      assert.equal(request.id, 'active');
      assert.match(request.automation?.statusReason ?? '', /Project was removed/);
      return [];
    } };
    await (host as any).removeProjectInternal(project);
    assert.equal(settings.hasProject(project), false);
    assert.deepEqual(calls, ['list', 'pause', 'close']);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
