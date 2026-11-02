import assert from 'node:assert/strict';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, renameSync, rmSync, symlinkSync, unlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { test, type TestContext } from 'node:test';
import type { SessionWorkspaceState, SetWorkingDirectoryRequest, SetWorkingDirectoryResult } from '@lingxi/bridge-client';

import { HostController, resolveSavedConversationTranscript, type WorkspaceDialogs } from '../src/main/host';
import { SessionRuntime } from '../src/main/bridge';
import { SessionRuntimeManager } from '../src/main/sessionRuntimeManager';
import { DiagnosticBuffer } from '../src/main/host-utils';
import type { SessionRef } from '../src/main/bridgeTypes';
import type { ProjectSessionCatalogRow } from '../src/main/session-catalog';
import { SettingsStore } from '../src/main/settings';
import { TerminalManager } from '../src/main/terminal';
import type { SessionCatalogScope, SessionOpenSelection } from '../src/shared/sessionWorkspace';

const sessionId = '11111111-2222-4333-8444-555555555555';
const otherId = '22222222-3333-4444-8555-666666666666';

class WorkspaceRuntime {
  connectionState = { status: 'connected' as const };
  sessionMode: 'chat' | 'code' = 'code';
  cwd: string;
  initialProjectRoot: string;
  transcriptPath: string;
  promptDirectories: string[];
  directories: string[] = [];
  hasConversation: boolean | undefined = true;
  runtimeControls: string | undefined;
  busy = false;
  leased = false;
  reads = 0;
  leases = 0;
  releases = 0;
  changes: SetWorkingDirectoryRequest[] = [];
  additions: string[] = [];
  constructor(readonly projectPath: string, readonly sessionId: string) {
    this.cwd = projectPath;
    this.initialProjectRoot = projectPath;
    this.transcriptPath = join(projectPath, `${sessionId}.jsonl`);
    this.promptDirectories = [projectPath];
  }
  get hasActiveWork(): boolean { return this.busy || this.leased; }
  beginWorkspaceChange(): () => void {
    if (this.busy || this.leased) throw new Error('Wait for active work before changing this chat’s directory.');
    this.leased = true; this.leases++;
    let released = false;
    return () => { if (!released) { released = true; this.leased = false; this.releases++; } };
  }
  async getWorkspaceState(): Promise<SessionWorkspaceState & { has_conversation?: boolean }> {
    this.reads++;
    return { cwd: this.cwd, initial_project_root: this.initialProjectRoot,
      transcript_path: this.transcriptPath, prompt_directories: [...this.promptDirectories], additional_directories: [...this.directories],
      ...(this.hasConversation === undefined ? {} : { has_conversation: this.hasConversation }),
      ...(this.runtimeControls ? { runtime_controls: this.runtimeControls } : {}) };
  }
  async setWorkingDirectory(request: SetWorkingDirectoryRequest): Promise<SetWorkingDirectoryResult> {
    assert.equal(this.leased, true);
    this.changes.push({ ...request });
    const cwd = realpathSync.native(resolve(this.cwd, request.path));
    const changed = cwd !== this.cwd;
    this.cwd = cwd;
    if (changed) {
      this.promptDirectories = [cwd];
      if (this.hasConversation === false) this.initialProjectRoot = cwd;
    }
    return { status: 'ok', cwd, changed, transcript_relocated: changed };
  }
  async addWorkingDirectory(path: string): Promise<SessionWorkspaceState> {
    assert.equal(this.leased, true);
    this.additions.push(path);
    const canonical = realpathSync.native(resolve(this.cwd, path));
    if (!this.directories.includes(canonical)) this.directories.push(canonical);
    if (!this.promptDirectories.includes(canonical)) this.promptDirectories.push(canonical);
    return this.getWorkspaceState();
  }
  rememberTranscriptSelection(path: string): void { this.transcriptPath = path; }
}

function hostFixture(t: TestContext) {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-workspace-host-')));
  const source = join(root, 'source'), target = join(root, 'target'), userData = join(root, 'settings');
  mkdirSync(source); mkdirSync(target);
  const settings = new SettingsStore(userData);
  const ref = { projectPath: source, sessionId };
  settings.conversationSpaces.setMode('code');
  settings.addProject(source); settings.activateProject(source); settings.setActiveSession(ref);
  const original = new WorkspaceRuntime(source, sessionId);
  writeFileSync(original.transcriptPath, '{}\n');
  const runtimes = new Map<string, WorkspaceRuntime>([[sessionId, original]]);
  const grants = new Map<string, readonly string[]>();
  const projectRoots = new Map<string, string>();
  const promptDirectories = new Map<string, readonly string[]>();
  const calls = {
    open: [] as Array<{ ref: SessionRef; empty: boolean; model?: string }>, close: [] as SessionRef[],
    openArguments: [] as unknown[][],
    picks: [] as string[], trust: [] as string[], find: [] as SessionRef[],
    select: [] as Array<{ ref: SessionRef; selection: SessionOpenSelection }>,
    list: [] as Array<{ path: string; scope: SessionCatalogScope }>, invalidations: 0,
    terminals: [] as Array<{ before: SessionRef; after: SessionRef }>,
  };
  const bridge = {
    get: (id: string) => runtimes.get(id),
    require: (requested: SessionRef) => {
      const runtime = runtimes.get(requested.sessionId);
      if (!runtime || runtime.projectPath !== requested.projectPath) throw new Error('session runtime is not open');
      return runtime;
    },
    isProjectClosing: () => false,
    hasActiveWork: (projectPath: string) => [...runtimes.values()].some((runtime) => runtime.projectPath === projectPath && runtime.hasActiveWork),
    setSessionDirectoryGrants: (id: string, paths: readonly string[]) => { grants.set(id, [...paths]); },
    setSessionProjectRoot: (id: string, projectPath: string) => { projectRoots.set(id, projectPath); },
    setSessionPromptDirectories: (id: string, paths: readonly string[]) => { promptDirectories.set(id, [...paths]); },
    prepareDraftRelocation: (_ref: SessionRef, _projectPath: string) => () => {},
    closeSession: async (requested: SessionRef) => {
      assert.equal(runtimes.get(requested.sessionId)?.leased, false, 'the lease must release immediately before removing the old runtime');
      assert.equal(runtimes.get(requested.sessionId)?.busy, false, 'busy runtimes must remain open');
      calls.close.push({ ...requested }); runtimes.delete(requested.sessionId);
    },
    openSession: async (...args: [requested: SessionRef, empty?: boolean, model?: string, activate?: boolean, transcriptPath?: string, remountControls?: string]) => {
      const [requested, empty = false, model, , transcriptPath] = args;
      calls.open.push({ ref: { projectPath: requested.projectPath, sessionId: requested.sessionId }, empty, model });
      calls.openArguments.push(args);
      const existing = runtimes.get(requested.sessionId);
      if (existing) {
        assert.equal(existing.projectPath, requested.projectPath, 'close the old copy before opening another source directory');
        if (transcriptPath) existing.rememberTranscriptSelection(transcriptPath);
        return existing;
      }
      const runtime = new WorkspaceRuntime(requested.projectPath, requested.sessionId);
      runtime.hasConversation = !empty;
      runtime.directories = [...(grants.get(requested.sessionId) ?? [])];
      runtime.initialProjectRoot = projectRoots.get(requested.sessionId) ?? requested.projectPath;
      runtime.promptDirectories = [...(promptDirectories.get(requested.sessionId) ?? runtime.promptDirectories)];
      if (transcriptPath) runtime.transcriptPath = transcriptPath;
      runtimes.set(requested.sessionId, runtime);
      return runtime;
    },
  };
  const row = (path: string, id = sessionId): ProjectSessionCatalogRow => ({
    uuid: id, title: 'Saved chat', path: join(path, `${id}.jsonl`), modified_rfc3339: '2026-10-21T00:00:00Z',
    message_count: 4, mode: 'code', empty_session: false, workspace_path: path,
  });
  const catalog = {
    find: async (path: string, id: string): Promise<ProjectSessionCatalogRow | undefined> => { calls.find.push({ projectPath: path, sessionId: id }); return row(path, id); },
    select: async (path: string, id: string, selection: SessionOpenSelection): Promise<ProjectSessionCatalogRow | undefined> => {
      calls.select.push({ ref: { projectPath: path, sessionId: id }, selection: { ...selection } });
      return { ...row(path, id), ...(id === sessionId ? { mode: original.sessionMode } : {}),
        ...(selection.transcriptPath ? { path: selection.transcriptPath } : {}) };
    },
    list: async (path: string, scope: SessionCatalogScope = 'current') => { calls.list.push({ path, scope }); return { sessions: [row(path)] }; },
    invalidate: () => { calls.invalidations++; },
  };
  const dialogs: WorkspaceDialogs = {
    pickDirectory: async (path) => { calls.picks.push(path); return undefined; },
    trustDirectory: async (path) => { calls.trust.push(path); return true; },
  };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), catalog as any,
    { handle() {}, removeHandler() {} } as any, undefined, undefined, undefined, dialogs);
  // The test uses the real orchestration and store; unrelated provider and OS
  // bootstrap services are replaced by their current persisted observations.
  (host as any).bootstrap = () => ({ settings: settings.getPublic(), activeSession: settings.getPublic().activeSession,
    projectCatalogs: Object.fromEntries((host as any).catalogs) });
  const terminals = new TerminalManager();
  const migrateScope = terminals.migrateScope.bind(terminals);
  terminals.migrateScope = (before, after) => {
    calls.terminals.push({ before, after });
    migrateScope(before, after);
  };
  host.attachTerminals(terminals);
  t.after(async () => { host.dispose(); await terminals.dispose(); rmSync(root, { recursive: true, force: true }); });
  return { root, source, target, userData, ref, settings, original, runtimes, grants, projectRoots, promptDirectories, calls, bridge, row, catalog, dialogs, host };
}

test('opening a pasted uppercase UUID retains the canonical runtime and persisted owner', async t => {
  const f = hostFixture(t), id = 'abcdef12-abcd-4abc-8abc-abcdef123456';
  f.runtimes.set(id, new WorkspaceRuntime(f.source, id));
  const snapshot = await f.host.openSessionAndActivate({ projectPath: f.source, sessionId: id.toUpperCase() });
  assert.equal(snapshot.activeSession?.sessionId, id);
  assert.equal(f.calls.open.at(-1)?.ref.sessionId, id);
  assert.equal(f.calls.find.length, 0, 'reuse the connected canonical owner instead of opening a duplicate');
});

test('/cd retains the exact source while remounting its owner in the new cwd', async (t) => {
  const f = hostFixture(t), moved = { projectPath: f.target, sessionId };
  const owner = { ...f.ref, mode: 'code' as const, projectId: f.source };
  f.settings.conversationSpaces.register(owner);
  f.original.directories = [f.root];
  f.settings.setSessionModel(f.ref, 'anthropic/session-model');
  f.settings.setSessionPinned({ ...f.ref, title: 'Pinned chat', pinnedAt: '2026-10-21T00:00:00Z' }, true);
  f.settings.update({ sidebar: { organization: 'project', chatSort: 'manual', manualSessionOrder: { [f.source]: [otherId, sessionId] } } });
  (f.host as any).catalogs.set(f.source, { sessions: [f.row(f.source)] });
  const snapshot = await f.host.updateSessionWorkspace(sessionId, { action: 'cd', path: f.target });
  assert.deepEqual(snapshot?.activeSession, { ...moved, mode: 'code', projectId: f.source });
  assert.deepEqual(f.calls.close, [owner]);
  assert.deepEqual(f.calls.open, [{ ref: moved, empty: false, model: 'anthropic/session-model' }]);
  assert.notEqual(f.runtimes.get(sessionId), f.original);
  assert.equal(f.runtimes.get(sessionId)?.projectPath, f.target);
  assert.equal(f.projectRoots.get(sessionId), f.source);
  assert.equal(f.runtimes.get(sessionId)?.initialProjectRoot, f.source);
  assert.deepEqual(f.promptDirectories.get(sessionId), [f.target]);
  assert.deepEqual(f.runtimes.get(sessionId)?.promptDirectories, [f.target]);
  assert.equal(f.calls.openArguments[0]?.[4], f.row(f.source).path, 'the remount must use the original transcript');
  assert.deepEqual(f.runtimes.get(sessionId)?.directories, [f.root]);
  assert.deepEqual(f.calls.terminals, [{ before: owner, after: moved }]);
  assert.equal((f.host as any).catalogs.has(f.source), false);
  assert.equal((f.host as any).catalogs.get(f.target).sessions[0].uuid, sessionId);
  const saved = new SettingsStore(f.userData);
  assert.deepEqual(saved.getPublic().activeSession, { ...moved, mode: 'code', projectId: f.source });
  assert.equal(saved.getPublic().pinnedSessions[0]?.projectPath, f.target);
  assert.equal(saved.getSessionModel(moved), 'anthropic/session-model');
  assert.deepEqual(saved.getSessionTranscriptSelection(moved), {
    ...moved, catalogProjectPath: f.target, scope: 'current', transcriptPath: f.row(f.source).path,
  });
  assert.deepEqual(saved.getPublic().sidebar?.manualSessionOrder, { [f.source]: [otherId, sessionId] });
  assert.equal(f.original.releases, 1);
});

for (const { mode, global } of [{ mode: 'code', global: false }, { mode: 'chat', global: true }] as const) {
  test(`/cd cold resume retains the ${global ? 'Global' : 'project'} ${mode} owner and exact source`, async t => {
    const f = hostFixture(t), selectedPath = f.original.transcriptPath;
    const owner = { ...f.ref, mode, projectId: global ? null : f.source };
    const moved = { ...owner, projectPath: f.target };
    f.original.sessionMode = mode;
    f.settings.conversationSpaces.setMode(mode);
    f.settings.conversationSpaces.register(owner);
    f.settings.setActiveSession(owner);
    f.settings.rememberSessionTranscriptSelection(owner, {
      catalogProjectPath: f.source, scope: 'current', transcriptPath: selectedPath,
    });

    await f.host.updateSessionWorkspace(sessionId, { action: 'cd', path: f.target });
    const saved = new SettingsStore(f.userData);
    assert.deepEqual(saved.conversationSpaces.findBySessionId(sessionId), moved);
    assert.deepEqual(saved.getPublic().activeSession, moved);
    assert.deepEqual(saved.getSessionTranscriptSelection(moved), {
      projectPath: f.target, sessionId, catalogProjectPath: f.target, scope: 'current', transcriptPath: selectedPath,
    });

    // Model a fresh process: only the on-disk settings and a catalog row whose
    // workspace was updated by the native relocation marker survive.
    f.runtimes.clear();
    f.calls.openArguments.length = 0;
    f.catalog.select = async (path, id, selection) => {
      assert.equal(path, f.target);
      assert.equal(id, sessionId);
      assert.deepEqual(selection, { scope: 'current', transcriptPath: selectedPath });
      return { ...f.row(f.target), mode, path: selectedPath, workspace_path: f.target };
    };
    const coldHost = new HostController(saved, f.bridge as any, new DiagnosticBuffer(), f.catalog as any,
      { handle() {}, removeHandler() {} } as any, undefined, undefined, undefined, f.dialogs);
    (coldHost as any).bootstrap = () => ({ settings: saved.getPublic(), activeSession: saved.getPublic().activeSession });
    t.after(() => coldHost.dispose());
    await coldHost.openSessionAndActivate(moved);
    assert.deepEqual(f.calls.openArguments, [[moved, false, undefined, true, selectedPath]]);
    assert.deepEqual(saved.conversationSpaces.findBySessionId(sessionId), moved);
  });
}

test('startup restores the displayed Current alias path in its authoritative workspace', async (t) => {
  const f = hostFixture(t), moved = { projectPath: f.target, sessionId };
  f.runtimes.clear();
  const selected = { ...f.row(f.target), path: join(f.root, 'selected-copy', `${sessionId}.jsonl`) };
  f.catalog.list = async () => ({ sessions: [selected] });
  f.catalog.select = async (path, id, selection) => {
    f.calls.select.push({ ref: { projectPath: path, sessionId: id }, selection });
    return selected;
  };
  const restored = await f.host.restoreProjectSession(f.source);
  assert.deepEqual(restored, { ...moved, mode: 'code', projectId: f.source });
  assert.deepEqual(f.settings.getPublic().activeSession, { ...moved, mode: 'code', projectId: f.source });
  assert.deepEqual(f.calls.select, [{ ref: f.ref,
    selection: { scope: 'current', transcriptPath: selected.path } }]);
  assert.deepEqual(f.calls.openArguments, [[{ ...moved, mode: 'code', projectId: f.source }, false, undefined, true, selected.path]]);
  assert.deepEqual(f.calls.terminals, [{ before: { projectPath: f.source, sessionId: '__draft__' }, after: { ...moved, mode: 'code', projectId: f.source } }]);
  assert.deepEqual(f.calls.find, [], 'a displayed Current row must not use the CLI UUID resolver');
  assert.equal((f.host as any).catalogs.get(f.source).sessions[0].workspace_path, undefined);
});

test('a slow startup restore cannot overwrite a newer queued navigation', async (t) => {
  const f = hostFixture(t);
  f.runtimes.clear();
  f.settings.addProject(f.target);
  let finish!: (value: { sessions: ProjectSessionCatalogRow[] }) => void;
  f.catalog.list = async () => new Promise(resolve => { finish = resolve; });
  const startup = f.host.restoreProjectSession(f.source);
  await new Promise(resolve => setImmediate(resolve));
  const newer = { projectPath: f.target, sessionId: otherId };
  const selected = f.row(f.target, otherId);
  const navigation = f.host.openSessionAndActivate(newer,
    { scope: 'current', transcriptPath: selected.path });
  await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(f.calls.open, [], 'a later navigation waits for the admitted startup operation');
  finish({ sessions: [f.row(f.source)] });
  await startup;
  await navigation;
  assert.deepEqual(f.calls.open.map(call => call.ref), [f.ref, newer]);
  assert.deepEqual(f.settings.getPublic().activeSession, { ...newer, mode: 'code', projectId: f.target });
});

test('/cd opens an unsent draft without resuming its computed but unwritten transcript path', async (t) => {
  const f = hostFixture(t), moved = { projectPath: f.target, sessionId };
  f.settings.setActiveSession(undefined);
  f.settings.setActiveSessionDraft(f.ref);
  f.original.hasConversation = false;
  unlinkSync(f.original.transcriptPath);
  f.catalog.find = async (path, id) => { f.calls.find.push({ projectPath: path, sessionId: id }); return undefined; };
  f.catalog.list = async () => ({ sessions: [] });
  assert.equal(new SettingsStore(f.userData).getPublic().activeSession, undefined, 'the unsent draft must start as volatile state');
  assert.equal(existsSync(f.original.transcriptPath), false);
  const snapshot = await f.host.updateSessionWorkspace(sessionId, { action: 'cd', path: f.target });
  assert.equal(existsSync(f.row(f.target).path), false, 'a relocated writer path does not imply that a JSONL file exists');
  assert.deepEqual(f.calls.openArguments, [[moved, true, undefined, true, undefined, undefined]]);
  assert.deepEqual(f.calls.close, [f.ref]);
  assert.equal(f.projectRoots.get(sessionId), f.target);
  assert.equal(f.runtimes.get(sessionId)?.initialProjectRoot, f.target);
  assert.equal(f.runtimes.get(sessionId)?.hasConversation, false);
  assert.deepEqual(snapshot?.activeSession, moved);
  assert.deepEqual(snapshot?.projectCatalogs[f.target].sessions, []);
  const restarted = new SettingsStore(f.userData);
  assert.equal(restarted.getPublic().activeSession, undefined, 'cold start must not try to resume an unwritten draft UUID');
  assert.equal(restarted.getPublic().activeProject, f.target);
});

test('/cd rolls back the native directory move if the target already owns a draft', async (t) => {
  const f = hostFixture(t);
  f.settings.setActiveSession(undefined);
  f.settings.setActiveSessionDraft(f.ref);
  f.original.hasConversation = false;
  unlinkSync(f.original.transcriptPath);
  f.bridge.prepareDraftRelocation = () => { throw new Error('The destination project already has an unsent draft.'); };
  await assert.rejects(f.host.updateSessionWorkspace(sessionId, { action: 'cd', path: f.target }), /already has an unsent draft/);
  assert.equal(f.original.cwd, f.source);
  assert.strictEqual(f.runtimes.get(sessionId), f.original);
  assert.deepEqual(f.calls.close, []);
  assert.deepEqual(f.calls.open, []);
  assert.deepEqual(f.settings.getPublic().activeSession, f.ref);
});

test('/cd with history and no catalog row propagates a missing transcript failure instead of silently opening an empty draft', async (t) => {
  for (const hasConversation of [true, undefined]) {
    const f = hostFixture(t);
    f.original.hasConversation = hasConversation;
    unlinkSync(f.original.transcriptPath);
    f.catalog.find = async (path, id) => { f.calls.find.push({ projectPath: path, sessionId: id }); return undefined; };
    const before = readFileSync(f.settings.settingsPath, 'utf8');
    await assert.rejects(f.host.updateSessionWorkspace(sessionId, { action: 'cd', path: f.target }), /source is unavailable/);
    assert.deepEqual(f.calls.openArguments, []);
    assert.deepEqual(f.calls.close, []);
    assert.strictEqual(f.runtimes.get(sessionId), f.original);
    assert.equal(f.original.cwd, f.source);
    assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), before);
    assert.equal(f.projectRoots.get(sessionId), f.source);
    assert.equal(f.original.releases, 1);
  }
});

test('/cd ignores a same-UUID sibling anchor and resumes the engine source', async t => {
  const f = hostFixture(t), owner = { ...f.ref, mode: 'code' as const, projectId: f.source };
  const moved = { ...owner, projectPath: f.target }, source = f.original.transcriptPath;
  f.settings.conversationSpaces.register(owner);
  f.settings.setSessionModel(owner, 'owner/model');
  f.settings.rememberSessionTranscriptSelection(owner, { catalogProjectPath: f.source, scope: 'current', transcriptPath: source });
  f.catalog.find = async () => ({ ...f.row(f.target), empty_session: true, resume_model: 'sibling/model' });
  f.catalog.select = async (path, id, selection) => {
    assert.deepEqual(selection, { scope: 'current', transcriptPath: source });
    return { ...f.row(path, id), path: source, resume_model: 'owner/model' };
  };
  await f.host.updateSessionWorkspace(sessionId, { action: 'cd', path: f.target });
  assert.deepEqual(f.calls.openArguments, [[{ projectPath: f.target, sessionId }, false, 'owner/model', true, source, undefined]]);
  const saved = new SettingsStore(f.userData);
  assert.deepEqual(saved.conversationSpaces.findBySessionId(sessionId), moved);
  assert.equal(saved.getSessionTranscriptSelection(moved)?.transcriptPath, source);
});

test('/cd preserves and resumes a durable empty scheduled anchor', async t => {
  const f = hostFixture(t), owner = { ...f.ref, mode: 'code' as const, projectId: null };
  const moved = { ...owner, projectPath: f.target }, source = f.original.transcriptPath;
  f.original.hasConversation = false;
  writeFileSync(source, `${JSON.stringify({ type: 'fixed-session-mode', sessionId, sessionMode: 'code', instructionEpoch: 0 })}\n${JSON.stringify({ type: 'custom-title', sessionId, mobileEmptySession: 1, cwd: f.source })}\n`);
  f.settings.conversationSpaces.register(owner);
  f.settings.rememberSessionTranscriptSelection(owner, { catalogProjectPath: f.source, scope: 'current', transcriptPath: source });
  f.catalog.select = async (path, id, selection) => {
    assert.deepEqual(selection, { scope: 'current', transcriptPath: source });
    return { ...f.row(path, id), path: source, empty_session: true, workspace_path: f.target };
  };
  await f.host.updateSessionWorkspace(sessionId, { action: 'cd', path: f.target });
  assert.deepEqual(f.calls.openArguments, [[{ projectPath: f.target, sessionId }, false, undefined, true, source, undefined]]);
  const saved = new SettingsStore(f.userData);
  assert.deepEqual(saved.conversationSpaces.findBySessionId(sessionId), moved);
  assert.deepEqual(saved.getSessionTranscriptSelection(moved), {
    projectPath: f.target, sessionId, catalogProjectPath: f.target, scope: 'current', transcriptPath: source,
  });
});

test('/cd rejects a missing or mismatched exact catalog source before changing its owner', async t => {
  for (const conflict of ['missing', 'path', 'mode'] as const) {
    const f = hostFixture(t), owner = { ...f.ref, mode: 'code' as const, projectId: f.source };
    const source = f.original.transcriptPath;
    f.settings.conversationSpaces.register(owner);
    f.settings.rememberSessionTranscriptSelection(owner, { catalogProjectPath: f.source, scope: 'current', transcriptPath: source });
    const before = readFileSync(f.settings.settingsPath, 'utf8');
    f.catalog.select = async (path, id) => conflict === 'missing' ? undefined : {
      ...f.row(path, id), path: conflict === 'path' ? join(f.target, `${sessionId}.jsonl`) : source,
      mode: conflict === 'mode' ? 'chat' : 'code',
    };
    await assert.rejects(f.host.updateSessionWorkspace(sessionId, { action: 'cd', path: f.target }), /source is unavailable|catalog does not match/);
    assert.equal(f.original.cwd, f.source);
    assert.strictEqual(f.runtimes.get(sessionId), f.original);
    assert.deepEqual(f.settings.conversationSpaces.findBySessionId(sessionId), owner);
    assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), before);
    assert.deepEqual(f.calls.close, []);
  }
});

test('/cd rejects a different live source even before a selection was saved', async t => {
  const f = hostFixture(t), owner = { ...f.ref, mode: 'code' as const, projectId: f.source };
  f.settings.conversationSpaces.register(owner);
  const before = readFileSync(f.settings.settingsPath, 'utf8'), source = f.original.transcriptPath;
  const change = f.original.setWorkingDirectory.bind(f.original);
  f.original.setWorkingDirectory = async request => {
    const result = await change(request);
    f.original.transcriptPath = request.path === f.target ? join(f.target, `${sessionId}.jsonl`) : source;
    return result;
  };
  await assert.rejects(f.host.updateSessionWorkspace(sessionId, { action: 'cd', path: f.target }), /source changed/);
  assert.equal(f.original.cwd, f.source);
  assert.equal(f.original.transcriptPath, source);
  assert.deepEqual(f.settings.conversationSpaces.findBySessionId(sessionId), owner);
  assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), before);
  assert.deepEqual(f.calls.close, []);
});

test('/cd never recreates an observed empty anchor that disappeared during the move', async t => {
  const f = hostFixture(t), owner = { ...f.ref, mode: 'code' as const, projectId: null };
  f.original.hasConversation = false;
  f.settings.conversationSpaces.register(owner);
  const before = readFileSync(f.settings.settingsPath, 'utf8');
  const change = f.original.setWorkingDirectory.bind(f.original);
  f.original.setWorkingDirectory = async request => {
    const result = await change(request);
    if (request.path === f.target) unlinkSync(f.original.transcriptPath);
    return result;
  };
  await assert.rejects(f.host.updateSessionWorkspace(sessionId, { action: 'cd', path: f.target }), /source is unavailable/);
  assert.equal(f.original.cwd, f.source);
  assert.deepEqual(f.settings.conversationSpaces.findBySessionId(sessionId), owner);
  assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), before);
  assert.deepEqual(f.calls.open, []);
  assert.deepEqual(f.calls.close, []);
});

test('/cd rejects a same-byte source replacement during exact catalog lookup', async t => {
  const f = hostFixture(t), owner = { ...f.ref, mode: 'code' as const, projectId: f.source };
  f.settings.conversationSpaces.register(owner);
  const source = f.original.transcriptPath, bytes = readFileSync(source);
  const before = readFileSync(f.settings.settingsPath, 'utf8');
  f.catalog.select = async (path, id) => {
    renameSync(source, `${source}.original`);
    writeFileSync(source, bytes);
    return { ...f.row(path, id), path: source };
  };
  await assert.rejects(f.host.updateSessionWorkspace(sessionId, { action: 'cd', path: f.target }), /source changed/);
  assert.equal(f.original.cwd, f.source);
  assert.deepEqual(f.settings.conversationSpaces.findBySessionId(sessionId), owner);
  assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), before);
  assert.deepEqual(f.calls.open, []);
  assert.deepEqual(f.calls.close, []);
});

test('explicit /add-dir saves session grants without adding a project, changing selection or remounting', async (t) => {
  const f = hostFixture(t), before = f.settings.getPublic(), bytes = readFileSync(f.settings.settingsPath, 'utf8');
  f.original.directories = [f.root];
  const snapshot = await f.host.updateSessionWorkspace(sessionId, { action: 'add-dir', path: f.target });
  assert.deepEqual(snapshot?.settings, before);
  assert.deepEqual(f.grants.get(sessionId), [f.root, f.target]);
  assert.deepEqual(f.promptDirectories.get(sessionId), [f.source, f.target]);
  assert.deepEqual(f.calls.close, []); assert.deepEqual(f.calls.open, []);
  assert.deepEqual(f.original.changes, []);
  assert.equal(f.runtimes.get(sessionId), f.original);
  assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), bytes);
});

test('canceling either native directory picker leaves settings, grants and the active runtime untouched', async (t) => {
  for (const action of ['cd', 'add-dir'] as const) {
    const f = hostFixture(t), bytes = readFileSync(f.settings.settingsPath, 'utf8');
    assert.equal(await f.host.updateSessionWorkspace(sessionId, { action }), null);
    assert.deepEqual(f.calls.picks, [f.source]);
    assert.deepEqual(f.original.changes, []); assert.deepEqual(f.original.additions, []);
    assert.equal(f.grants.size, 0); assert.equal(f.runtimes.get(sessionId), f.original);
    assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), bytes);
    assert.equal(f.original.leased, false); assert.equal(f.original.releases, 1);
  }
});

test('a native no-op keeps the existing runtime and settings file', async (t) => {
  const f = hostFixture(t), bytes = readFileSync(f.settings.settingsPath, 'utf8');
  await f.host.updateSessionWorkspace(sessionId, { action: 'cd', path: '.' });
  assert.deepEqual(f.calls.close, []); assert.deepEqual(f.calls.open, []);
  assert.equal(f.runtimes.get(sessionId), f.original);
  assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), bytes);
});

test('busy work and stale active identities reject before a picker or native mutation runs', async (t) => {
  const f = hostFixture(t);
  f.original.busy = true;
  await assert.rejects(f.host.updateSessionWorkspace(sessionId, { action: 'cd' }), /active work/);
  assert.equal(f.original.reads, 0); assert.deepEqual(f.calls.picks, []);
  f.original.busy = false;
  await assert.rejects(f.host.updateSessionWorkspace(otherId, { action: 'add-dir', path: f.target }), /no longer active/);
  assert.equal(f.original.leases, 0); assert.deepEqual(f.original.changes, []);
});

test('trust approval echoes the displayed canonical directory and re-prompts when a symlink changes target', { skip: process.platform === 'win32' }, async (t) => {
  const f = hostFixture(t), changedTarget = join(f.root, 'changed-target'), alias = join(f.root, 'chosen-directory');
  mkdirSync(changedTarget); symlinkSync(f.target, alias, 'dir');
  f.original.setWorkingDirectory = async (request) => {
    f.original.changes.push({ ...request });
    const actual = realpathSync.native(request.path);
    if (!request.trust_accepted || request.trusted_directory !== actual) return { status: 'needs_trust', directory: actual };
    f.original.cwd = actual;
    f.original.promptDirectories = [actual];
    return { status: 'ok', cwd: actual, changed: true, transcript_relocated: true };
  };
  f.dialogs.trustDirectory = async (path) => {
    f.calls.trust.push(path);
    if (path === f.target) { unlinkSync(alias); symlinkSync(changedTarget, alias, 'dir'); }
    return true;
  };
  await f.host.updateSessionWorkspace(sessionId, { action: 'cd', path: alias });
  assert.deepEqual(f.calls.trust, [f.target, changedTarget]);
  assert.deepEqual(f.original.changes, [
    { path: alias },
    { path: alias, trust_accepted: true, trusted_directory: f.target },
    { path: alias, trust_accepted: true, trusted_directory: changedTarget },
  ]);
  assert.equal(f.settings.getPublic().activeSession?.projectPath, changedTarget);
});

test('/cd into the app\'s own data is refused and rolled back, and adds no project', async (t) => {
  const f = hostFixture(t), bytes = readFileSync(f.settings.settingsPath, 'utf8');
  const inside = join(f.userData, 'bots', 'someone', 'workspace');
  mkdirSync(inside, { recursive: true });
  await assert.rejects(f.host.updateSessionWorkspace(sessionId, { action: 'cd', path: inside }), /belongs to the app's own data/);
  assert.equal(f.original.cwd, f.source, 'the engine is back where it was');
  assert.deepEqual(f.original.changes, [{ path: inside }, { path: f.source, trust_accepted: true, trusted_directory: f.source }]);
  assert.deepEqual(f.settings.getPublic().projects, [f.source]);
  assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), bytes);
  assert.equal(f.runtimes.get(sessionId), f.original);
  assert.deepEqual(f.calls.close, []); assert.deepEqual(f.calls.open, []);
  assert.equal(f.original.releases, 1);
});

test('an engine that asks to trust a folder of the app\'s own data is not asked of the owner', async (t) => {
  const f = hostFixture(t), bytes = readFileSync(f.settings.settingsPath, 'utf8');
  const inside = join(f.userData, 'logs');
  mkdirSync(inside, { recursive: true });
  f.original.setWorkingDirectory = async (request) => {
    f.original.changes.push({ ...request });
    return request.path === f.source ? { status: 'ok', cwd: f.source, changed: false, transcript_relocated: false }
      : { status: 'needs_trust', directory: realpathSync.native(inside) };
  };
  await assert.rejects(f.host.updateSessionWorkspace(sessionId, { action: 'cd', path: inside }), /belongs to the app's own data/);
  assert.deepEqual(f.calls.trust, [], 'the owner was not asked to trust it');
  assert.deepEqual(f.settings.getPublic().projects, [f.source]);
  assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), bytes);
});

test('a session whose folder lies in the app\'s own data is not added as a project by opening it', async (t) => {
  const f = hostFixture(t), inside = join(f.userData, 'bots', 'someone', 'workspace'), id = 'abcdef12-abcd-4abc-8abc-abcdef123456';
  mkdirSync(inside, { recursive: true });
  const selected = { ...f.row(inside, id), workspace_path: inside };
  f.catalog.find = async () => selected;
  f.catalog.select = async () => selected;
  await assert.rejects(f.host.openSessionAndActivate({ projectPath: f.source, sessionId: id }, { scope: 'current', transcriptPath: selected.path }), /belongs to the app's own data/);
  assert.deepEqual(f.calls.trust, []);
  assert.deepEqual(f.settings.getPublic().projects, [f.source]);
});

test('canceling trust or a rejected native cd leaves the old session authoritative', async (t) => {
  for (const cancelTrust of [true, false]) {
    const f = hostFixture(t), bytes = readFileSync(f.settings.settingsPath, 'utf8');
    f.original.setWorkingDirectory = async (request) => {
      f.original.changes.push({ ...request });
      return cancelTrust ? { status: 'needs_trust', directory: f.target }
        : { status: 'rejected', reason: 'missing', message: 'The requested directory is unavailable.' };
    };
    f.dialogs.trustDirectory = async () => false;
    if (cancelTrust) assert.equal(await f.host.updateSessionWorkspace(sessionId, { action: 'cd', path: f.target }), null);
    else await assert.rejects(f.host.updateSessionWorkspace(sessionId, { action: 'cd', path: f.target }), /requested directory is unavailable/);
    assert.equal(f.original.cwd, f.source);
    assert.equal(f.runtimes.get(sessionId), f.original);
    assert.deepEqual(f.calls.close, []); assert.deepEqual(f.calls.open, []);
    assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), bytes);
    assert.equal(f.original.releases, 1);
  }
});

test('a real settings write failure rolls the native cwd back before releasing the lease', async (t) => {
  const f = hostFixture(t), owner = { ...f.ref, mode: 'code' as const, projectId: f.source };
  f.settings.conversationSpaces.register(owner);
  f.settings.rememberSessionTranscriptSelection(owner, {
    catalogProjectPath: f.source, scope: 'current', transcriptPath: f.original.transcriptPath,
  });
  const before = f.settings.getPublic(), bytes = readFileSync(f.settings.settingsPath, 'utf8');
  mkdirSync(`${f.settings.settingsPath}.tmp`);
  await assert.rejects(f.host.updateSessionWorkspace(sessionId, { action: 'cd', path: f.target }), /EISDIR|directory/i);
  assert.equal(f.original.cwd, f.source);
  assert.deepEqual(f.original.changes, [{ path: f.target }, { path: f.source, trust_accepted: true, trusted_directory: f.source }]);
  assert.deepEqual(f.settings.getPublic(), before);
  assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), bytes);
  assert.deepEqual(f.settings.conversationSpaces.findBySessionId(sessionId), owner);
  assert.equal(f.settings.getSessionTranscriptSelection(owner)?.transcriptPath, f.original.transcriptPath);
  assert.equal(f.runtimes.get(sessionId), f.original);
  assert.deepEqual(f.calls.close, []); assert.equal(f.original.releases, 1);
});

test('a native relocation marker failure keeps the source and owner unchanged', async t => {
  const f = hostFixture(t), owner = { ...f.ref, mode: 'code' as const, projectId: f.source };
  f.settings.conversationSpaces.register(owner);
  f.settings.rememberSessionTranscriptSelection(owner, {
    catalogProjectPath: f.source, scope: 'current', transcriptPath: f.original.transcriptPath,
  });
  const before = readFileSync(f.settings.settingsPath, 'utf8');
  f.original.setWorkingDirectory = async request => {
    f.original.changes.push(request);
    throw new Error('could not persist relocation marker');
  };
  await assert.rejects(f.host.updateSessionWorkspace(sessionId, { action: 'cd', path: f.target }), /relocation marker/);
  assert.equal(f.original.cwd, f.source);
  assert.equal(f.original.transcriptPath, f.row(f.source).path);
  assert.deepEqual(f.settings.conversationSpaces.findBySessionId(sessionId), owner);
  assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), before);
  assert.strictEqual(f.runtimes.get(sessionId), f.original);
  assert.deepEqual(f.calls.close, []);
  assert.equal(f.original.releases, 1);
});

test('failed native rollback stops the engine and reports both the persistence and recovery failure', async (t) => {
  const f = hostFixture(t), bytes = readFileSync(f.settings.settingsPath, 'utf8');
  const move = f.original.setWorkingDirectory.bind(f.original);
  f.original.setWorkingDirectory = async (request) => request.path === f.source
    ? { status: 'rejected', reason: 'rollback', message: 'rollback refused' } : move(request);
  mkdirSync(`${f.settings.settingsPath}.tmp`);
  await assert.rejects(f.host.updateSessionWorkspace(sessionId, { action: 'cd', path: f.target }), /engine was stopped.*rollback was rejected/);
  assert.equal(f.runtimes.get(sessionId), undefined);
  assert.deepEqual(f.calls.close, [f.ref]); assert.deepEqual(f.calls.open, []);
  assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), bytes);
  assert.deepEqual(f.settings.getPublic().activeSession, f.ref);
});

test('a lost native response still checks the actual cwd and restores a move that already happened', async (t) => {
  const f = hostFixture(t), bytes = readFileSync(f.settings.settingsPath, 'utf8');
  const move = f.original.setWorkingDirectory.bind(f.original);
  f.original.setWorkingDirectory = async (request) => {
    const result = await move(request);
    if (request.path === f.target) throw new Error('native response timed out');
    return result;
  };
  await assert.rejects(f.host.updateSessionWorkspace(sessionId, { action: 'cd', path: f.target }), /native response timed out/);
  assert.equal(f.original.cwd, f.source);
  assert.equal(f.original.changes.length, 2);
  assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), bytes);
  assert.equal(f.runtimes.get(sessionId), f.original);
});

test('repository/all discoveries and their failures do not overwrite the current host catalog cache', async (t) => {
  const f = hostFixture(t), internal = f.host as any;
  f.catalog.list = async (path, scope = 'current') => {
    if (scope === 'all') throw new Error('expanded catalog temporarily unavailable');
    return { sessions: [{ ...f.row(path, scope === 'repository' ? otherId : sessionId), resume_model: 'private/model' }] };
  };
  const current = await internal.loadProjectSessions(f.source, 'current');
  assert.equal(current.sessions[0].workspace_path, undefined);
  assert.equal(current.sessions[0].resume_model, undefined);
  const cached = internal.catalogs.get(f.source);
  const repository = await internal.loadProjectSessions(f.source, 'repository');
  assert.equal(repository.sessions[0].uuid, otherId);
  assert.equal(internal.catalogs.get(f.source), cached);
  const all = await internal.loadProjectSessions(f.source, 'all');
  assert.match(all.error, /expanded catalog temporarily unavailable/);
  assert.equal(internal.catalogs.get(f.source), cached);
});

test('resuming a source workspace hint uses its real directory and asks for trust before activation', async (t) => {
  const f = hostFixture(t);
  f.runtimes.clear();
  const selected = { ...f.row(f.source), workspace_path: f.target, resume_model: 'anthropic/history-model' };
  f.catalog.find = async () => selected;
  f.catalog.select = async () => selected;
  await f.host.openSessionAndActivate(f.ref, { scope: 'current', transcriptPath: selected.path });
  const moved = { projectPath: f.target, sessionId };
  assert.deepEqual(f.calls.trust, [f.target]);
  assert.deepEqual(f.calls.open, [{ ref: moved, empty: false, model: 'anthropic/history-model' }]);
  assert.deepEqual(new SettingsStore(f.userData).getPublic().activeSession, { ...moved, mode: 'code', projectId: f.source });
});

test('a missing source workspace hint resumes the same UUID from the chosen existing directory', async (t) => {
  const f = hostFixture(t);
  f.runtimes.clear();
  const selected = { ...f.row(f.source), workspace_path: join(f.root, 'removed-worktree') };
  f.catalog.find = async () => selected;
  f.catalog.select = async () => selected;
  await f.host.openSessionAndActivate(f.ref, { scope: 'current', transcriptPath: selected.path });
  assert.deepEqual(f.calls.open, [{ ref: f.ref, empty: false, model: undefined }]);
  assert.deepEqual(f.calls.trust, []);
  assert.deepEqual(f.settings.getPublic().activeSession, { ...f.ref, mode: 'code', projectId: f.source });
});

test('an exact cold selection pins its copy for engine startup and subsequent restoration', async t => {
  const f = hostFixture(t);
  const selected = { ...f.row(f.source), path: join(f.source, 'selected-copy', `${sessionId}.jsonl`) };
  const other = { ...f.row(f.source), path: join(f.source, 'other-copy', `${sessionId}.jsonl`) };
  f.runtimes.clear();
  f.catalog.find = async () => selected;
  f.catalog.select = async (path, id, selection) => {
    f.calls.select.push({ ref: { projectPath: path, sessionId: id }, selection: { ...selection } });
    return selected;
  };
  await f.host.openSessionAndActivate(f.ref, { scope: 'current', transcriptPath: selected.path });
  assert.equal(f.calls.openArguments[0]?.[4], selected.path);
  assert.deepEqual(new SettingsStore(f.userData).getSessionTranscriptSelection(f.ref), {
    ...f.ref, catalogProjectPath: f.source, scope: 'current', transcriptPath: selected.path,
  });

  f.runtimes.clear();
  f.calls.openArguments.length = 0;
  f.calls.select.length = 0;
  let uuidLookups = 0;
  f.catalog.find = async () => { uuidLookups++; return other; };
  const settings = new SettingsStore(f.userData);
  const host = new HostController(settings, f.bridge as any, new DiagnosticBuffer(), f.catalog as any,
    { handle() {}, removeHandler() {} } as any, undefined, undefined, undefined, f.dialogs);
  (host as any).bootstrap = () => ({ activeSession: settings.getPublic().activeSession });
  t.after(() => host.dispose());
  await host.openSessionAndActivate(f.ref);
  assert.equal(uuidLookups, 0, 'cold restoration must not fall back to a different UUID copy');
  assert.equal(f.calls.openArguments[0]?.[4], selected.path);
  assert.deepEqual(f.calls.select, [{ ref: f.ref, selection: { scope: 'current', transcriptPath: selected.path } }]);
});

test('a disconnected writer can reconnect without consulting unavailable catalog metadata', async t => {
  const f = hostFixture(t);
  (f.original as any).connectionState = { status: 'disconnected' };
  f.settings.conversationSpaces.register({ ...f.ref, mode: 'code', projectId: f.source });
  f.catalog.find = async () => { throw new Error('catalog unavailable'); };
  f.catalog.select = async () => { throw new Error('catalog unavailable'); };
  await f.host.openSessionAndActivate(f.ref);
  assert.equal(f.calls.open.length, 1);
  assert.strictEqual(f.runtimes.get(sessionId), f.original);
});

test('a cold open without a selected source rejects before resolving a UUID copy', async t => {
  const f = hostFixture(t);
  const selected = { ...f.row(f.source), path: join(f.source, 'selected-copy', `${sessionId}.jsonl`) };
  const other = { ...f.row(f.source), path: join(f.source, 'other-copy', `${sessionId}.jsonl`) };
  f.runtimes.clear();
  f.catalog.find = async () => selected;
  f.catalog.select = async () => other;
  const before = readFileSync(f.settings.settingsPath, 'utf8');
  await assert.rejects(f.host.openSessionAndActivate(f.ref), /source is unavailable/i);
  assert.deepEqual(f.calls.openArguments, []);
  assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), before);
  assert.equal(f.settings.conversationSpaces.find(f.ref), undefined);
});

test('a saved source whose workspace changes cannot relocate the owner on cold reopen', async t => {
  const f = hostFixture(t);
  const owner = { ...f.ref, mode: 'code' as const, projectId: f.source };
  f.settings.conversationSpaces.register(owner);
  f.settings.rememberSessionTranscriptSelection(owner, {
    catalogProjectPath: f.source, scope: 'current', transcriptPath: f.row(f.source).path,
  });
  const before = readFileSync(f.settings.settingsPath, 'utf8');
  f.runtimes.clear();
  f.catalog.select = async () => ({ ...f.row(f.source), workspace_path: f.target });
  await assert.rejects(f.host.openSessionAndActivate(f.ref), /saved session workspace changed/);
  assert.deepEqual(f.calls.open, []);
  assert.deepEqual(f.settings.conversationSpaces.findBySessionId(sessionId), owner);
  assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), before);
});

test('remote cold attach validates the saved copy and reuses the live writer on later attaches', async t => {
  const f = hostFixture(t);
  const selected = { ...f.row(f.source), path: join(f.source, 'selected-copy', `${sessionId}.jsonl`) };
  const owner = { ...f.ref, mode: 'code' as const, projectId: f.source };
  f.settings.conversationSpaces.register(owner);
  f.settings.relocateSession(f.ref, f.source, { catalogProjectPath: f.source, scope: 'current', transcriptPath: selected.path });
  const settings = new SettingsStore(f.userData);
  let checks = 0;
  f.catalog.select = async (_path, _id, selection) => {
    checks++;
    assert.deepEqual(selection, { scope: 'current', transcriptPath: selected.path });
    return selected;
  };
  const starts: Array<string | undefined> = [], resumes: Array<string | undefined> = [];
  const originalStart = SessionRuntime.prototype.start, originalResume = SessionRuntime.prototype.resumeOwnedSession;
  SessionRuntime.prototype.start = async function () {
    starts.push((this as any).opts.resumeTranscriptPath);
    (this as any).setState({ status: 'connected' });
  };
  SessionRuntime.prototype.resumeOwnedSession = async function (path) { resumes.push(path); };
  const manager = new SessionRuntimeManager({
    launchConfig: ref => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
    resolveSavedTranscript: ref => resolveSavedConversationTranscript(settings, f.catalog as any, ref),
  });
  t.after(async () => {
    SessionRuntime.prototype.start = originalStart;
    SessionRuntime.prototype.resumeOwnedSession = originalResume;
    await manager.dispose();
  });
  const first = await manager.openForRemote(owner);
  assert.deepEqual(starts, [selected.path]);
  assert.deepEqual(resumes, [selected.path]);
  const second = await manager.openForRemote(owner);
  assert.equal(second.runtime, first.runtime);
  assert.equal(checks, 1, 'an already connected owner retains its writer');
  first.release(); second.release();
});

test('a first durable prompt records the live writer source without changing another selected session', async t => {
  const f = hostFixture(t), owner = { ...f.ref, projectId: f.source, mode: 'code' as const };
  f.settings.conversationSpaces.register(owner);
  const other = { projectPath: f.source, sessionId: otherId, projectId: f.source, mode: 'code' as const };
  f.settings.conversationSpaces.register(other); f.settings.setActiveSession(other);
  writeFileSync(f.original.transcriptPath, '{}\n');
  assert.equal(await f.host.rememberCreatedSessionSource(owner, f.original as any, false), f.original.transcriptPath);
  const saved = new SettingsStore(f.userData);
  assert.deepEqual(saved.getSessionTranscriptSelection(owner), { projectPath: f.source, sessionId,
    catalogProjectPath: f.source, scope: 'current', transcriptPath: f.original.transcriptPath });
  assert.deepEqual(saved.getPublic().activeSession, other);
  assert.deepEqual(f.calls.select, [{ ref: f.ref, selection: { scope: 'current', transcriptPath: f.original.transcriptPath } }]);
  assert.deepEqual(f.calls.find, []);
  assert.deepEqual(await resolveSavedConversationTranscript(saved, f.catalog as any, owner), { path: f.original.transcriptPath, mode: 'code' });
});

test('source capture rejects mismatched mode, workspace or writer and preserves the prior source', async t => {
  const f = hostFixture(t), owner = { ...f.ref, projectId: f.source, mode: 'code' as const };
  f.settings.conversationSpaces.register(owner); writeFileSync(f.original.transcriptPath, '{}\n');
  const prior = { catalogProjectPath: f.source, scope: 'current' as const, transcriptPath: f.original.transcriptPath };
  f.settings.rememberSessionTranscriptSelection(owner, prior);
  const before = readFileSync(f.settings.settingsPath, 'utf8');
  for (const conflict of [
    { ...f.row(f.source), mode: 'chat' as const },
    { ...f.row(f.source), uuid: otherId },
    { ...f.row(f.source), workspace_path: f.target },
  ]) {
    f.catalog.select = async () => conflict;
    await assert.rejects(f.host.rememberCreatedSessionSource(owner, f.original as any, false));
    assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), before);
  }
  f.catalog.select = async () => f.row(f.source);
  f.original.cwd = f.target;
  await assert.rejects(f.host.rememberCreatedSessionSource(owner, f.original as any, false), /verified transcript source/);
  assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), before);
});

test('a cold background or remote open without an exact saved source fails before starting an engine', async t => {
  const f = hostFixture(t), owner = { ...f.ref, projectId: f.source, mode: 'code' as const };
  f.settings.conversationSpaces.register(owner);
  let started = 0;
  const originalStart = SessionRuntime.prototype.start;
  SessionRuntime.prototype.start = async function (options) {
    if (options?.reconnectOnly) throw new Error('The session source is unavailable.');
    started++;
  };
  const manager = new SessionRuntimeManager({
    launchConfig: ref => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
    resolveSavedTranscript: ref => resolveSavedConversationTranscript(f.settings, f.catalog as any, ref),
  });
  t.after(async () => { SessionRuntime.prototype.start = originalStart; await manager.dispose(); });
  await assert.rejects(manager.openForRemote(owner), /source is unavailable/i);
  await assert.rejects(manager.withBackgroundSession(owner, false, undefined, async () => undefined), /source is unavailable/i);
  const disconnected = new SessionRuntime({ sessionId, projectPath: f.source,
    launchConfig: () => ({ workspace: f.source, sessionId, trusted: true }) }, 'code');
  (manager as any).runtimes.set(sessionId, disconnected);
  await assert.rejects(manager.openForRemote(owner), /source is unavailable/i);
  assert.equal(started, 0);
});

test('a new scheduled anchor records its confirmed source before the binding operation proceeds', async t => {
  const f = hostFixture(t), owner = { ...f.ref, projectId: f.source, mode: 'code' as const };
  let finish!: (path: string) => void, entered!: () => void;
  const waiting = new Promise<string>(resolve => { finish = resolve; });
  const captureEntered = new Promise<void>(resolve => { entered = resolve; });
  let executed = false;
  const originalStart = SessionRuntime.prototype.start;
  SessionRuntime.prototype.start = async function () {};
  const manager = new SessionRuntimeManager({
    launchConfig: ref => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
    captureSessionSource: async (_ref, _runtime, scheduled) => { assert.equal(scheduled, true); entered(); return waiting; },
  });
  t.after(async () => { SessionRuntime.prototype.start = originalStart; await manager.dispose(); });
  const run = manager.withBackgroundSession(owner, true, undefined, async () => { executed = true; }, undefined, true);
  await captureEntered; assert.equal(executed, false);
  finish(join(f.source, `${sessionId}.jsonl`)); await run;
  assert.equal(executed, true);
});

test('a managed scheduled anchor records its exact source and Global ownership across restart', async t => {
  const f = hostFixture(t), workspace = join(f.root, 'scheduled-run'); mkdirSync(workspace);
  const ref = { projectPath: workspace, sessionId: otherId, mode: 'code' as const };
  f.settings.registerScheduledSession(ref, 'task', join(workspace, 'memory.md'));
  const runtime = new WorkspaceRuntime(workspace, otherId);
  f.runtimes.set(otherId, runtime); writeFileSync(runtime.transcriptPath, '{}\n');
  await f.host.rememberCreatedSessionSource(ref, runtime as any, true);
  const saved = new SettingsStore(f.userData);
  assert.deepEqual(saved.conversationSpaces.find(ref), { ...ref, projectId: null });
  assert.deepEqual(saved.getSessionTranscriptSelection(ref), { projectPath: workspace, sessionId: otherId,
    catalogProjectPath: workspace, scope: 'current', transcriptPath: runtime.transcriptPath });
  assert.deepEqual(await resolveSavedConversationTranscript(saved, f.catalog as any, ref), { path: runtime.transcriptPath, mode: 'code' });
});

test('a transient source recording failure does not interrupt an admitted prompt and is retried', async t => {
  const f = hostFixture(t), owner = { ...f.ref, projectId: f.source, mode: 'code' as const };
  let attempts = 0, commits = 0;
  const manager = new SessionRuntimeManager({
    launchConfig: ref => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
    onFirstPromptSent: () => { commits++; },
    captureSessionSource: async () => {
      if (++attempts === 1) throw new Error('catalog not visible yet');
      return join(f.source, `${sessionId}.jsonl`);
    },
  });
  (manager as any).runtimes.set(sessionId, f.original);
  t.after(async () => { (manager as any).runtimes.delete(sessionId); await manager.dispose(); });
  const callbacks = (manager as any).runtimeOptions(owner);
  assert.equal(callbacks.onFirstPromptSent(), true);
  assert.equal(commits, 1);
  await (manager as any).sourceCapturePending.get(sessionId);
  assert.equal(attempts, 2);
  assert.equal((manager as any).sessionTranscriptPaths.get(sessionId), join(f.source, `${sessionId}.jsonl`));
  assert.equal(f.original.busy, false);
});

test('a saved remote source rejects a conflicting catalog owner before starting an engine', async t => {
  const f = hostFixture(t);
  const selected = f.row(f.source);
  const owner = { ...f.ref, mode: 'code' as const, projectId: f.source };
  f.settings.conversationSpaces.register(owner);
  f.settings.relocateSession(f.ref, f.source, { catalogProjectPath: f.source, scope: 'current', transcriptPath: selected.path });
  const settings = new SettingsStore(f.userData);
  let started = 0;
  const originalStart = SessionRuntime.prototype.start;
  SessionRuntime.prototype.start = async function () { started++; };
  const manager = new SessionRuntimeManager({
    launchConfig: ref => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
    resolveSavedTranscript: ref => resolveSavedConversationTranscript(settings, f.catalog as any, ref),
  });
  t.after(async () => { SessionRuntime.prototype.start = originalStart; await manager.dispose(); });
  for (const conflict of [
    { ...selected, mode: 'chat' as const },
    { ...selected, uuid: otherId },
    { ...selected, path: join(f.source, 'other-copy', `${sessionId}.jsonl`) },
    { ...selected, workspace_path: f.target },
  ]) {
    f.catalog.select = async () => conflict;
    await assert.rejects(manager.openForRemote(owner), /saved session (selection|workspace)/i);
  }
  f.catalog.select = async () => selected;
  await assert.rejects(manager.openForRemote({ ...owner, projectPath: f.target }), /saved session owner/i);
  await assert.rejects(manager.openForRemote({ ...owner, projectId: f.target }), /saved session owner/i);
  await assert.rejects(manager.openForRemote({ ...owner, mode: 'chat' }), /saved session owner/i);
  assert.equal(started, 0);
});

test('background cold opens use the saved source while new anchors do not consult it', async t => {
  const f = hostFixture(t);
  const selected = f.row(f.source);
  const owner = { ...f.ref, mode: 'code' as const, projectId: f.source };
  f.settings.conversationSpaces.register(owner);
  f.settings.relocateSession(f.ref, f.source, { catalogProjectPath: f.source, scope: 'current', transcriptPath: selected.path });
  const settings = new SettingsStore(f.userData);
  let checks = 0;
  f.catalog.select = async () => { checks++; return selected; };
  const starts: Array<string | undefined> = [], resumes: Array<string | undefined> = [];
  const originalStart = SessionRuntime.prototype.start, originalResume = SessionRuntime.prototype.resumeOwnedSession;
  SessionRuntime.prototype.start = async function () { starts.push((this as any).opts.resumeTranscriptPath); };
  SessionRuntime.prototype.resumeOwnedSession = async function (path) { resumes.push(path); };
  const manager = new SessionRuntimeManager({
    launchConfig: ref => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
    resolveSavedTranscript: ref => resolveSavedConversationTranscript(settings, f.catalog as any, ref),
  });
  t.after(async () => {
    SessionRuntime.prototype.start = originalStart;
    SessionRuntime.prototype.resumeOwnedSession = originalResume;
    await manager.dispose();
  });
  await manager.withBackgroundSession(owner, false, undefined, async () => undefined);
  await manager.withBackgroundSession({ ...owner, sessionId: otherId }, true, undefined, async () => undefined,
    undefined, true);
  assert.equal(checks, 1);
  assert.deepEqual(starts, [selected.path, undefined]);
  assert.deepEqual(resumes, [selected.path]);
});

test('an old project reference still opens an already-remounted UUID in its authoritative runtime directory', async (t) => {
  const f = hostFixture(t), moved = { projectPath: f.target, sessionId };
  await f.host.updateSessionWorkspace(sessionId, { action: 'cd', path: f.target });
  f.calls.open.length = 0; f.calls.find.length = 0;
  await f.host.openSessionAndActivate(f.ref);
  assert.deepEqual(f.calls.open, [{ ref: moved, empty: false, model: undefined }]);
  assert.deepEqual(f.calls.find, []);
  assert.deepEqual(f.settings.getPublic().activeSession, { ...moved, mode: 'code', projectId: f.source });
});

for (const scope of ['repository', 'all'] as const) {
  test(`${scope} selection revalidates a fresh copy instead of reusing the cached current copy of the same UUID`, async (t) => {
    const f = hostFixture(t), selected = { ...f.row(f.target), resume_model: 'anthropic/selected-copy' };
    const cached = await (f.host as any).loadProjectSessions(f.source, 'current');
    assert.equal(cached.sessions[0].path, f.original.transcriptPath);
    const select = f.catalog.select;
    f.catalog.select = async (...args) => { await select(...args); return selected; };
    const selection = { scope, transcriptPath: selected.path };
    const snapshot = await f.host.openSessionAndActivate(f.ref, selection);
    const restored = { projectPath: f.target, sessionId };
    assert.deepEqual(f.calls.select, [{ ref: f.ref, selection }]);
    assert.deepEqual(f.calls.close, [f.ref]);
    assert.deepEqual(f.calls.open, [{ ref: restored, empty: false, model: 'anthropic/selected-copy' }]);
    assert.equal(f.calls.openArguments[0]?.[3] ?? true, true);
    assert.equal(f.calls.openArguments[0]?.[4], selected.path);
    assert.notEqual(f.runtimes.get(sessionId), f.original);
    assert.equal(f.runtimes.get(sessionId)?.transcriptPath, selected.path);
    assert.deepEqual(snapshot.activeSession, { ...restored, mode: 'code', projectId: f.source });
    assert.deepEqual(f.calls.trust, [f.target]);
  });
}

test('a selected path that no longer matches fresh metadata rejects before closing or opening a runtime', async (t) => {
  const f = hostFixture(t), bytes = readFileSync(f.settings.settingsPath, 'utf8');
  const select = f.catalog.select;
  f.catalog.select = async (...args) => { await select(...args); return f.row(f.source); };
  const selection = { scope: 'repository' as const, transcriptPath: f.row(f.target).path };
  await assert.rejects(f.host.openSessionAndActivate(f.ref, selection), /selection.*changed|selected.*(match|path)|transcript.*(match|path)/i);
  assert.deepEqual(f.calls.select, [{ ref: f.ref, selection }]);
  assert.deepEqual(f.calls.close, []); assert.deepEqual(f.calls.open, []);
  assert.deepEqual(f.calls.trust, []);
  assert.equal(f.runtimes.get(sessionId), f.original);
  assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), bytes);
});

test('a busy current copy blocks switching to an exact selected copy with the same UUID', async (t) => {
  const f = hostFixture(t), selected = f.row(f.target), bytes = readFileSync(f.settings.settingsPath, 'utf8');
  const select = f.catalog.select;
  f.catalog.select = async (...args) => { await select(...args); return selected; };
  f.original.busy = true;
  await assert.rejects(f.host.openSessionAndActivate(f.ref, { scope: 'repository', transcriptPath: selected.path }), /active work|busy|pending interactions/i);
  assert.deepEqual(f.calls.close, []); assert.deepEqual(f.calls.open, []);
  assert.deepEqual(f.calls.trust, []);
  assert.equal(f.runtimes.get(sessionId), f.original);
  assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), bytes);
});

test('an exact selection already open at the same source and transcript reuses its runtime', async (t) => {
  const f = hostFixture(t), selected = f.row(f.source), selection = { scope: 'repository' as const, transcriptPath: selected.path };
  const snapshot = await f.host.openSessionAndActivate(f.ref, selection);
  assert.deepEqual(f.calls.select, [{ ref: f.ref, selection }]);
  assert.deepEqual(f.calls.close, []);
  assert.equal(f.runtimes.get(sessionId), f.original);
  assert.deepEqual(snapshot.activeSession, { ...f.ref, mode: 'code', projectId: f.source });
  assert.deepEqual(f.calls.trust, []);
});

test('an exact selection of a different transcript in the same source still replaces the idle copy', async (t) => {
  const f = hostFixture(t), selected = { ...f.row(f.source), path: join(f.source, 'other-copy', `${sessionId}.jsonl`) };
  const select = f.catalog.select;
  f.catalog.select = async (...args) => { await select(...args); return selected; };
  await f.host.openSessionAndActivate(f.ref, { scope: 'repository', transcriptPath: selected.path });
  assert.deepEqual(f.calls.close, [f.ref]);
  assert.equal(f.calls.openArguments[0]?.[4], selected.path);
  assert.notEqual(f.runtimes.get(sessionId), f.original);
  assert.equal(f.runtimes.get(sessionId)?.transcriptPath, selected.path);
});

test('a selected transcript keeps its exact path when its missing source directory falls back to the current directory', async (t) => {
  const f = hostFixture(t), selected = { ...f.row(f.target), workspace_path: join(f.root, 'removed-worktree') };
  f.runtimes.clear();
  const select = f.catalog.select;
  f.catalog.select = async (...args) => { await select(...args); return selected; };
  await f.host.openSessionAndActivate(f.ref, { scope: 'repository', transcriptPath: selected.path });
  assert.deepEqual(f.calls.close, []);
  assert.deepEqual(f.calls.open, [{ ref: f.ref, empty: false, model: undefined }]);
  assert.equal(f.calls.openArguments[0]?.[4], selected.path);
  assert.deepEqual(f.settings.getPublic().activeSession, { ...f.ref, mode: 'code', projectId: f.source });
  assert.deepEqual(f.calls.trust, []);
});

test('canceling trust for a selected copy leaves the currently open copy in place', async (t) => {
  const f = hostFixture(t), selected = f.row(f.target), bytes = readFileSync(f.settings.settingsPath, 'utf8');
  const select = f.catalog.select;
  f.catalog.select = async (...args) => { await select(...args); return selected; };
  f.dialogs.trustDirectory = async (path) => { f.calls.trust.push(path); return false; };
  await assert.rejects(f.host.openSessionAndActivate(f.ref, { scope: 'repository', transcriptPath: selected.path }), /cancelled/i);
  assert.deepEqual(f.calls.trust, [f.target]);
  assert.deepEqual(f.calls.close, []); assert.deepEqual(f.calls.open, []);
  assert.equal(f.runtimes.get(sessionId), f.original);
  assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), bytes);
});

for (const coldStart of [false, true]) {
  test(`an exact selected transcript survives ${coldStart ? 'a full settings and host reload' : 'eviction and ordinary sidebar reopen'}`, async (t) => {
    const f = hostFixture(t);
    const selected = { ...f.row(f.source), path: join(f.source, 'selected-copy', `${sessionId}.jsonl`) };
    f.catalog.select = async (path, id, selection) => {
      f.calls.select.push({ ref: { projectPath: path, sessionId: id }, selection });
      return selected;
    };
    await f.host.openSessionAndActivate(f.ref, { scope: 'all', transcriptPath: selected.path });
    f.runtimes.clear();
    f.calls.openArguments.length = 0;
    f.calls.find.length = 0;
    f.calls.select.length = 0;
    const settings = coldStart ? new SettingsStore(f.userData) : f.settings;
    const host = coldStart ? new HostController(settings, f.bridge as any, new DiagnosticBuffer(), f.catalog as any,
      { handle() {}, removeHandler() {} } as any, undefined, undefined, undefined, f.dialogs) : f.host;
    if (coldStart) {
      (host as any).bootstrap = () => ({ activeSession: settings.getPublic().activeSession });
      t.after(() => host.dispose());
      await host.restoreProjectSession(f.source, settings.getPublic().activeSession);
    } else await host.openSessionAndActivate(f.ref);
    assert.equal(f.calls.openArguments[0]?.[4], selected.path,
      'a plain UUID resume must not replace the exact picker selection after its engine is gone');
    assert.deepEqual(f.calls.find, []);
    assert.deepEqual(f.calls.select, [{ ref: f.ref, selection: { scope: 'all', transcriptPath: selected.path } }]);
    assert.deepEqual(settings.getPublic().activeSession, { ...f.ref, mode: 'code', projectId: f.source }, 'transcript selection must not change SessionRef identity');
  });
}

test('a missing saved selected transcript cannot silently resume another copy after cold start', async (t) => {
  const f = hostFixture(t);
  const selected = { ...f.row(f.source), path: join(f.source, 'selected-copy', `${sessionId}.jsonl`) };
  f.catalog.select = async () => selected;
  await f.host.openSessionAndActivate(f.ref, { scope: 'all', transcriptPath: selected.path });
  f.runtimes.clear();
  f.calls.openArguments.length = 0;
  f.calls.find.length = 0;
  f.catalog.select = async () => undefined;
  const settings = new SettingsStore(f.userData);
  const host = new HostController(settings, f.bridge as any, new DiagnosticBuffer(), f.catalog as any,
    { handle() {}, removeHandler() {} } as any);
  (host as any).bootstrap = () => ({ activeSession: settings.getPublic().activeSession });
  t.after(() => host.dispose());
  await assert.rejects(host.restoreProjectSession(f.source, settings.getPublic().activeSession), /session.*(changed|project)|selection/i);
  assert.deepEqual(f.calls.openArguments, []);
  assert.deepEqual(f.calls.find, [], 'the UUID resolver has a different copy and must never be used as a fallback');
  assert.deepEqual(settings.getPublic().activeSession, { ...f.ref, mode: 'code', projectId: f.source });
});

test('a cold public open through a symlink revalidates the canonical owner\'s saved transcript', { skip: process.platform === 'win32' }, async (t) => {
  const f = hostFixture(t), alias = join(f.root, 'project-alias');
  symlinkSync(f.source, alias, 'dir');
  const selected = { ...f.row(f.source), path: join(f.source, 'selected-copy', `${sessionId}.jsonl`) };
  f.catalog.select = async (path, id, selection) => {
    f.calls.select.push({ ref: { projectPath: path, sessionId: id }, selection });
    return selected;
  };
  await f.host.openSessionAndActivate(f.ref, { scope: 'all', transcriptPath: selected.path });
  f.runtimes.clear();
  f.calls.select.length = 0;
  f.calls.find.length = 0;
  f.calls.openArguments.length = 0;
  const settings = new SettingsStore(f.userData);
  const host = new HostController(settings, f.bridge as any, new DiagnosticBuffer(), f.catalog as any,
    { handle() {}, removeHandler() {} } as any);
  (host as any).bootstrap = () => ({ activeSession: settings.getPublic().activeSession });
  t.after(() => host.dispose());
  await host.openSessionAndActivate({ projectPath: alias, sessionId });
  assert.deepEqual(f.calls.select, [{ ref: f.ref, selection: { scope: 'all', transcriptPath: selected.path } }]);
  assert.deepEqual(f.calls.find, [], 'the raw alias must not miss the canonical source selection and enter UUID lookup');
  assert.equal(f.calls.openArguments[0]?.[4], selected.path);
  assert.deepEqual(settings.getPublic().activeSession, { ...f.ref, mode: 'code', projectId: f.source });
});

test('cold restore revalidates the original picker scope after the selected source moves navigation to another directory', async (t) => {
  const f = hostFixture(t), moved = { projectPath: f.target, sessionId }, selected = f.row(f.target);
  f.catalog.select = async (path, id, selection) => {
    f.calls.select.push({ ref: { projectPath: path, sessionId: id }, selection });
    assert.equal(path, f.source, 're-query the original alias/picker context, not the new launch directory');
    return selected;
  };
  await f.host.openSessionAndActivate(f.ref, { scope: 'repository', transcriptPath: selected.path });
  f.runtimes.clear();
  f.calls.select.length = 0;
  f.calls.openArguments.length = 0;
  f.settings.removeProject(f.source);
  const settings = new SettingsStore(f.userData);
  const host = new HostController(settings, f.bridge as any, new DiagnosticBuffer(), f.catalog as any,
    { handle() {}, removeHandler() {} } as any, undefined, undefined, undefined, f.dialogs);
  (host as any).bootstrap = () => ({ activeSession: settings.getPublic().activeSession });
  t.after(() => host.dispose());
  await host.restoreProjectSession(f.target, settings.getPublic().activeSession);
  assert.deepEqual(f.calls.select, [{ ref: f.ref, selection: { scope: 'repository', transcriptPath: selected.path } }]);
  assert.equal(f.calls.openArguments[0]?.[4], selected.path);
  assert.deepEqual(settings.getPublic().activeSession, { ...moved, mode: 'code', projectId: f.source });
  assert.equal(settings.hasProject(f.source), false, 'read-only discovery context does not re-trust or register the removed directory');
});


test('/cd carries only the current runtime controls to its same-UUID replacement', async (t) => {
  const f = hostFixture(t);
  f.original.runtimeControls = '{"private":"source-runtime-controls"}';
  await f.host.updateSessionWorkspace(sessionId, { action: 'cd', path: f.target });
  assert.equal(f.calls.openArguments[0]?.[5], f.original.runtimeControls);
  assert.equal(readFileSync(f.settings.settingsPath, 'utf8').includes('source-runtime-controls'), false);
});

test('a failed selected remount leaves the confirmed conversation owner and navigation unchanged', async t => {
  const f = hostFixture(t);
  const owner = { ...f.ref, projectId: f.source, mode: 'code' as const };
  f.settings.conversationSpaces.register(owner);
  f.settings.conversationSpaces.setNavigation('code', f.source, owner);
  const selected = f.row(f.target);
  f.catalog.select = async () => selected;
  f.bridge.openSession = async () => { throw new Error('selected mount failed'); };
  await assert.rejects(f.host.openSessionAndActivate(f.ref, { scope: 'all', transcriptPath: selected.path }), /selected mount failed/);
  const reopened = new SettingsStore(f.userData).conversationSpaces.snapshot();
  assert.deepEqual(reopened.sessions.find((item) => item.sessionId === sessionId), owner);
  assert.deepEqual(reopened.navigation.code, { projectId: f.source, session: owner });
  assert.equal(reopened.sessions.some((item) => item.projectPath === f.target && item.sessionId === sessionId), false);
});

test('a failed first mount does not persist a new conversation owner', async t => {
  const f = hostFixture(t); f.runtimes.clear();
  f.bridge.openSession = async () => { throw new Error('first mount failed'); };
  await assert.rejects(f.host.openSessionAndActivate(f.ref, { scope: 'current', transcriptPath: f.row(f.source).path }), /first mount failed/);
  assert.equal(new SettingsStore(f.userData).conversationSpaces.find(f.ref), undefined);
});

test('/cd keeps the visible draft selection attached to its relocated owner', async t => {
  const f = hostFixture(t);
  const draft = { ...f.ref, mode: 'code' as const, projectId: f.source };
  f.settings.conversationSpaces.register(draft); f.settings.conversationSpaces.setMode('code');
  f.settings.setActiveSession(undefined); f.settings.setActiveSessionDraft(draft);
  (f.host as any).draftSelections.set('code', draft);
  f.original.hasConversation = false;
  unlinkSync(f.original.transcriptPath);
  f.catalog.find = async () => undefined;
  await f.host.updateSessionWorkspace(sessionId, { action: 'cd', path: f.target });
  const moved = { ...draft, projectPath: f.target };
  assert.deepEqual((f.host as any).draftSelections.get('code'), moved);
  assert.deepEqual((f.host as any).conversationSpacesSnapshot().navigation.code.session, moved);
  assert.deepEqual(f.settings.getPublic().activeSession, moved);
});

test('a cold copy in another directory preserves the session UUID owner and project', async t => {
  const f = hostFixture(t);
  const owner = { ...f.ref, projectId: f.source, mode: 'code' as const };
  f.settings.conversationSpaces.register(owner); f.settings.conversationSpaces.setNavigation('code', f.source, owner);
  f.settings.addProject(f.target); f.runtimes.clear();
  await f.host.openSessionAndActivate({ projectPath: f.target, sessionId }, { scope: 'current', transcriptPath: f.row(f.target).path });
  const moved = f.settings.conversationSpaces.find({ projectPath: f.target, sessionId });
  assert.deepEqual(moved, { ...owner, projectPath: f.target });
  assert.equal(f.settings.conversationSpaces.snapshot().sessions.filter((entry) => entry.sessionId === sessionId).length, 1);
});
