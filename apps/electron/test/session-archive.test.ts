import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, realpathSync, rmSync, writeFileSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { SettingsStore } from '../src/main/settings';
import { HostController } from '../src/main/host';
import { DiagnosticBuffer } from '../src/main/host-utils';

const sessionId = '11111111-2222-4333-8444-555555555555';
test('archive persists metadata and unpins without touching the transcript; restore clears marker', () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-archive-'));
  const projectPath = realpathSync.native(dir);
  try {
    const settings = new SettingsStore(dir);
    const ref = { projectPath, sessionId };
    settings.addProject(projectPath);
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
      const calls: string[] = [];
      const runtime = {
        projectPath, connectionState: { status: 'connected' },
        beginArchive: () => { calls.push('lock'); return () => { calls.push('unlock'); }; },
        manageCron: async (request: { action: string; id?: string; automation?: { statusReason?: string } }) => {
          if (request.action !== 'list') {
            assert.equal(request.action, 'pause', 'lifecycle changes must not use partial updates');
            assert.match(request.automation?.statusReason ?? '', /archived/);
          }
          calls.push(request.action + (request.id ? ':' + request.id : ''));
          assert.equal(settings.isSessionArchived(ref), false, 'cleanup must precede archive');
          if (fail && request.id === 'second') throw new Error('fixture stop failure');
          return [{ id: 'first', automation: { status: 'active', runMode: 'selected_session', targetSessionId: sessionId } }, { id: 'second', automation: { status: 'active', runMode: 'task_session', ownedSessionId: sessionId } }, { id: 'creator-only', session_id: sessionId }, { id: 'unrelated', automation: { targetSessionId: 'another-session' } }];
        },
      };
      const bridge = {
        get: () => runtime,
        withBackgroundSession: async (_ref: unknown, _empty: unknown, _model: unknown, operation: (runtime: any) => Promise<unknown>) => operation(runtime),
        closeSession: async () => { assert.equal(settings.isSessionArchived(ref), true); calls.push('close'); },
      };
      const row = { uuid: sessionId, title: 'Saved chat', modified_rfc3339: '', message_count: 1, mode: 'code', path: '', empty_session: false };
      const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), { list: async () => ({ sessions: [row] }), invalidate: () => undefined } as any);
      // Isolate orchestration from unrelated platform bootstrap services.
      (host as any).bootstrap = () => ({ settings: settings.getPublic() });
      if (fail) {
        await assert.rejects((host as any).archiveSessionInternal(ref), /Chat was not archived.*1 scheduled task.*fixture stop failure/);
        assert.equal(settings.isSessionArchived(ref), false);
        assert.ok(!calls.includes('close'));
      } else {
        await (host as any).archiveSessionInternal(ref);
        assert.equal(settings.isSessionArchived(ref), true);
        assert.equal((await (host as any).loadProjectSessions(projectPath)).sessions.length, 0);
      }
      assert.ok(!calls.some((call) => call.startsWith('delete:')));
      assert.ok(!calls.includes('pause:creator-only'));
      assert.ok(!calls.includes('pause:unrelated'));
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

test('opening an archived chat restores it only after the existing session resumes successfully', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-archive-restore-'));
  const projectPath = realpathSync.native(dir);
  try {
    const settings = new SettingsStore(dir);
    const ref = { projectPath, sessionId };
    settings.addProject(projectPath);
    settings.setSessionArchived(ref, true, 'Saved chat');
    let fail = true;
    const row = { uuid: sessionId, empty_session: false };
    const bridge = { get: () => undefined, openSession: async () => { if (fail) throw new Error('resume failed'); } };
    const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), { find: async () => row, list: async () => ({ sessions: [row] }), invalidate: () => undefined } as any);
    (host as any).bootstrap = () => ({ settings: settings.getPublic() });
    await assert.rejects((host as any).openSessionAndActivateInternal(ref), /resume failed/);
    assert.equal(settings.isSessionArchived(ref), true);
    fail = false;
    await (host as any).openSessionAndActivateInternal(ref);
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
    settings.setActiveSession(ref);
    const runtime = { projectPath, connectionState: { status: 'connected' }, beginArchive: () => () => {}, manageCron: async () => [] };
    const bridge = { get: () => runtime, withBackgroundSession: async (_ref: unknown, _empty: unknown, _model: unknown, operation: (runtime: any) => Promise<unknown>) => operation(runtime), closeSession: async () => {}, newSession: async () => { throw new Error('fixture launch failure'); } };
    const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), { list: async () => ({ sessions: [] }), invalidate: () => undefined } as any);
    await assert.rejects((host as any).archiveSessionInternal(ref), /Chat was archived.*fixture launch failure/);
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
