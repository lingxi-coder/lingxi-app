import assert from 'node:assert/strict';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, unlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { test, type TestContext } from 'node:test';
import type { SessionWorkspaceState, SetWorkingDirectoryRequest, SetWorkingDirectoryResult } from '@lingxi/bridge-client';

import { HostController, type WorkspaceDialogs } from '../src/main/host';
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
      this.transcriptPath = join(cwd, `${this.sessionId}.jsonl`);
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
  settings.addProject(source); settings.activateProject(source); settings.setActiveSession(ref);
  const original = new WorkspaceRuntime(source, sessionId);
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
    closeSession: async (requested: SessionRef) => {
      assert.equal(runtimes.get(requested.sessionId)?.leased, false, 'the lease must release immediately before removing the old runtime');
      assert.equal(runtimes.get(requested.sessionId)?.busy, false, 'busy runtimes must remain open');
      calls.close.push({ ...requested }); runtimes.delete(requested.sessionId);
    },
    openSession: async (...args: [requested: SessionRef, empty?: boolean, model?: string, activate?: boolean, transcriptPath?: string, remountControls?: string]) => {
      const [requested, empty = false, model, , transcriptPath] = args;
      calls.open.push({ ref: { ...requested }, empty, model });
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
      return row(path, id);
    },
    list: async (path: string, scope: SessionCatalogScope = 'current') => { calls.list.push({ path, scope }); return { sessions: [row(path)] }; },
    invalidate: () => { calls.invalidations++; },
  };
  const dialogs: WorkspaceDialogs = {
    pickDirectory: async (path) => { calls.picks.push(path); return undefined; },
    trustDirectory: async (path) => { calls.trust.push(path); return true; },
  };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), catalog as any,
    { handle() {}, removeHandler() {} } as any, undefined, undefined, undefined, undefined, dialogs);
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

test('/cd remounts the same UUID in its new cwd and carries pin, model, order and explicit grants', async (t) => {
  const f = hostFixture(t), moved = { projectPath: f.target, sessionId };
  f.original.directories = [f.root];
  f.settings.setSessionModel(f.ref, 'anthropic/session-model');
  f.settings.setSessionPinned({ ...f.ref, title: 'Pinned chat', pinnedAt: '2026-10-21T00:00:00Z' }, true);
  f.settings.update({ sidebar: { organization: 'project', chatSort: 'manual', manualSessionOrder: { [f.source]: [otherId, sessionId] } } });
  (f.host as any).catalogs.set(f.source, { sessions: [f.row(f.source)] });
  const snapshot = await f.host.updateSessionWorkspace(sessionId, { action: 'cd', path: f.target });
  assert.deepEqual(snapshot?.activeSession, moved);
  assert.deepEqual(f.calls.close, [f.ref]);
  assert.deepEqual(f.calls.open, [{ ref: moved, empty: false, model: 'anthropic/session-model' }]);
  assert.notEqual(f.runtimes.get(sessionId), f.original);
  assert.equal(f.runtimes.get(sessionId)?.projectPath, f.target);
  assert.equal(f.projectRoots.get(sessionId), f.source);
  assert.equal(f.runtimes.get(sessionId)?.initialProjectRoot, f.source);
  assert.deepEqual(f.promptDirectories.get(sessionId), [f.target]);
  assert.deepEqual(f.runtimes.get(sessionId)?.promptDirectories, [f.target]);
  assert.equal(f.calls.openArguments[0]?.[4], f.row(f.target).path, 'the post-cd native transcript path must replace the old copy');
  assert.deepEqual(f.runtimes.get(sessionId)?.directories, [f.root]);
  assert.deepEqual(f.calls.terminals, [{ before: f.ref, after: moved }]);
  assert.equal((f.host as any).catalogs.has(f.source), false);
  assert.equal((f.host as any).catalogs.get(f.target).sessions[0].uuid, sessionId);
  const saved = new SettingsStore(f.userData);
  assert.deepEqual(saved.getPublic().activeSession, moved);
  assert.equal(saved.getPublic().pinnedSessions[0]?.projectPath, f.target);
  assert.equal(saved.getSessionModel(moved), 'anthropic/session-model');
  assert.deepEqual(saved.getPublic().sidebar?.manualSessionOrder, { [f.source]: [otherId], [f.target]: [sessionId] });
  assert.equal(f.original.releases, 1);
});

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
  assert.deepEqual(restored, moved);
  assert.deepEqual(f.settings.getPublic().activeSession, moved);
  assert.deepEqual(f.calls.select, [{ ref: f.ref,
    selection: { scope: 'current', transcriptPath: selected.path } }]);
  assert.deepEqual(f.calls.openArguments, [[moved, false, undefined, true, selected.path]]);
  assert.deepEqual(f.calls.terminals, [{ before: { projectPath: f.source, sessionId: '__draft__' }, after: moved }]);
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
  assert.deepEqual(f.settings.getPublic().activeSession, newer);
});

test('/cd opens an unsent draft without resuming its computed but unwritten transcript path', async (t) => {
  const f = hostFixture(t), moved = { projectPath: f.target, sessionId };
  f.settings.setActiveSession(undefined);
  f.settings.setActiveSessionDraft(f.ref);
  f.original.hasConversation = false;
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

test('/cd with history and no catalog row propagates a missing transcript failure instead of silently opening an empty draft', async (t) => {
  for (const hasConversation of [true, undefined]) {
    const f = hostFixture(t), moved = { projectPath: f.target, sessionId }, selectedPath = f.row(f.target).path;
    f.original.hasConversation = hasConversation;
    f.catalog.find = async (path, id) => { f.calls.find.push({ projectPath: path, sessionId: id }); return undefined; };
    const open = f.bridge.openSession;
    f.bridge.openSession = async (...args) => {
      if (args[1] !== true) {
        const [ref, empty = false, model] = args;
        f.calls.open.push({ ref: { ...ref }, empty, model }); f.calls.openArguments.push(args);
        // Exercise a real persistence read error at the manager boundary.
        readFileSync(args[4]!, 'utf8');
        assert.fail('the deliberately missing transcript unexpectedly exists');
      }
      return open(...args);
    };
    await assert.rejects(f.host.updateSessionWorkspace(sessionId, { action: 'cd', path: f.target }), { code: 'ENOENT', path: selectedPath });
    assert.deepEqual(f.calls.openArguments, [[moved, false, undefined, true, selectedPath, undefined]]);
    assert.deepEqual(f.calls.close, [f.ref]);
    assert.equal(f.runtimes.get(sessionId), undefined);
    assert.equal(f.projectRoots.get(sessionId), f.source);
    assert.equal(f.original.releases, 1);
  }
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
    f.original.transcriptPath = join(actual, `${sessionId}.jsonl`);
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
  const f = hostFixture(t), before = f.settings.getPublic(), bytes = readFileSync(f.settings.settingsPath, 'utf8');
  mkdirSync(`${f.settings.settingsPath}.tmp`);
  await assert.rejects(f.host.updateSessionWorkspace(sessionId, { action: 'cd', path: f.target }), /EISDIR|directory/i);
  assert.equal(f.original.cwd, f.source);
  assert.deepEqual(f.original.changes, [{ path: f.target }, { path: f.source, trust_accepted: true, trusted_directory: f.source }]);
  assert.deepEqual(f.settings.getPublic(), before);
  assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), bytes);
  assert.equal(f.runtimes.get(sessionId), f.original);
  assert.deepEqual(f.calls.close, []); assert.equal(f.original.releases, 1);
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
  f.catalog.find = async () => ({ ...f.row(f.source), workspace_path: f.target, resume_model: 'anthropic/history-model' });
  await f.host.openSessionAndActivate(f.ref);
  const moved = { projectPath: f.target, sessionId };
  assert.deepEqual(f.calls.trust, [f.target]);
  assert.deepEqual(f.calls.open, [{ ref: moved, empty: false, model: 'anthropic/history-model' }]);
  assert.deepEqual(new SettingsStore(f.userData).getPublic().activeSession, moved);
});

test('a missing source workspace hint resumes the same UUID from the chosen existing directory', async (t) => {
  const f = hostFixture(t);
  f.runtimes.clear();
  f.catalog.find = async () => ({ ...f.row(f.source), workspace_path: join(f.root, 'removed-worktree') });
  await f.host.openSessionAndActivate(f.ref);
  assert.deepEqual(f.calls.open, [{ ref: f.ref, empty: false, model: undefined }]);
  assert.deepEqual(f.calls.trust, []);
  assert.deepEqual(f.settings.getPublic().activeSession, f.ref);
});

test('an old project reference still opens an already-remounted UUID in its authoritative runtime directory', async (t) => {
  const f = hostFixture(t), moved = { projectPath: f.target, sessionId };
  await f.host.updateSessionWorkspace(sessionId, { action: 'cd', path: f.target });
  f.calls.open.length = 0; f.calls.find.length = 0;
  await f.host.openSessionAndActivate(f.ref);
  assert.deepEqual(f.calls.open, [{ ref: moved, empty: false, model: undefined }]);
  assert.deepEqual(f.calls.find, []);
  assert.deepEqual(f.settings.getPublic().activeSession, moved);
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
    assert.deepEqual(snapshot.activeSession, restored);
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
  assert.deepEqual(snapshot.activeSession, f.ref);
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
  assert.deepEqual(f.settings.getPublic().activeSession, f.ref);
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


test('/cd carries only the current runtime controls to its same-UUID replacement', async (t) => {
  const f = hostFixture(t);
  f.original.runtimeControls = '{"private":"source-runtime-controls"}';
  await f.host.updateSessionWorkspace(sessionId, { action: 'cd', path: f.target });
  assert.equal(f.calls.openArguments[0]?.[5], f.original.runtimeControls);
  assert.equal(readFileSync(f.settings.settingsPath, 'utf8').includes('source-runtime-controls'), false);
});
