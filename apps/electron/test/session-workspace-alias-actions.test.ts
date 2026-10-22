import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, statSync, utimesSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test, type TestContext } from 'node:test';
import {
  CH_SESSION_ARCHIVE, CH_SESSION_ARCHIVE_PREFLIGHT, CH_SESSION_RENAME, CH_SESSION_TOUCH,
  HostController, type WorkspaceDialogs,
} from '../src/main/host';
import { DiagnosticBuffer } from '../src/main/host-utils';
import type { SessionRef } from '../src/main/bridgeTypes';
import { ProjectSessionCatalog, type ProjectSessionCatalogRow } from '../src/main/session-catalog';
import { SettingsStore } from '../src/main/settings';

const sessionId = '11111111-2222-4333-8444-555555555555';
const replacementId = '22222222-3333-4444-8555-666666666666';

function fixture(t: TestContext, options: { running?: boolean; savedSource?: boolean } = {}) {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-alias-actions-')));
  const alias = join(root, 'A'), source = join(root, 'B'), store = join(root, 'transcripts');
  mkdirSync(alias); mkdirSync(source); mkdirSync(store);
  const settings = new SettingsStore(join(root, 'settings'));
  settings.addProject(alias);
  if (options.savedSource !== false) settings.addProject(source);
  const ref = Object.freeze({ projectPath: alias, sessionId });
  const sourceRef = { projectPath: source, sessionId };
  if (options.savedSource !== false) settings.setActiveSession(sourceRef);
  const path = join(store, `${sessionId}.jsonl`), untouchedPath = join(alias, 'unrelated.jsonl');
  const original = `${JSON.stringify({ type: 'user', sessionId, cwd: source, message: { role: 'user', content: 'Saved chat' } })}\n`;
  writeFileSync(path, original); writeFileSync(untouchedPath, 'A remains unchanged\n');
  utimesSync(path, new Date('2020-01-01T00:00:00Z'), new Date('2020-01-01T00:00:00Z'));
  const calls = {
    find: [] as SessionRef[], background: [] as SessionRef[], started: [] as SessionRef[],
    select: [] as (SessionRef & { scope: string })[], transcriptPaths: [] as (string | undefined)[],
    closed: [] as SessionRef[], replacements: [] as string[], trust: [] as string[], events: [] as string[],
  };
  const runtime = {
    projectPath: source, sessionId, connectionState: { status: 'connected' }, hasActiveWork: false,
    async getWorkspaceState() { return { cwd: this.projectPath, additional_directories: [], transcript_path: path }; },
    beginArchive() {
      if (this.hasActiveWork) throw new Error('Wait for active work before archiving this chat.');
      return () => undefined;
    },
    async manageCron(_request: unknown) {
      return [{ id: 'owned', automation: { targetSessionId: sessionId } },
        { id: 'other', automation: { targetSessionId: replacementId } }];
    },
  };
  const runtimes = new Map<string, typeof runtime>();
  if (options.running !== false) runtimes.set(sessionId, runtime);
  const bridge = {
    get: (id: string) => runtimes.get(id),
    isProjectClosing: () => false,
    registerWindow() {}, registerIpc() {},
    async withBackgroundSession(ref: SessionRef, _empty: boolean, _model: string | undefined, operation: (owner: typeof runtime) => Promise<unknown>, transcriptPath?: string) {
      calls.background.push({ ...ref });
      calls.transcriptPaths.push(transcriptPath);
      const existing = runtimes.get(ref.sessionId);
      if (existing) assert.equal(ref.projectPath, existing.projectPath, 'metadata must reuse the authoritative runtime');
      else {
        assert.equal(settings.hasProject(ref.projectPath), true, 'trust must persist before engine startup');
        calls.events.push(`start:${ref.projectPath}`); calls.started.push({ ...ref });
        runtimes.set(ref.sessionId, runtime);
      }
      return operation(runtime);
    },
    async closeSession(ref: SessionRef) {
      assert.equal(ref.projectPath, runtimes.get(ref.sessionId)?.projectPath);
      calls.closed.push({ ...ref }); runtimes.delete(ref.sessionId);
    },
    async newSession(project: string) {
      calls.replacements.push(project);
      return { projectPath: project, sessionId: replacementId };
    },
  };
  let hint = source;
  const row = (): ProjectSessionCatalogRow => {
    const entries = readFileSync(path, 'utf8').trim().split('\n').map((line) => JSON.parse(line));
    return { uuid: sessionId, title: entries.at(-1)?.customTitle ?? 'Saved chat', path,
      modified_rfc3339: statSync(path).mtime.toISOString(), message_count: 1, mode: 'code', empty_session: false,
      workspace_path: hint };
  };
  const catalog = {
    async find(projectPath: string, id: string) { calls.find.push({ projectPath, sessionId: id }); return id === sessionId ? row() : undefined; },
    async select(projectPath: string, id: string, selection: { scope: string }) {
      calls.select.push({ projectPath, sessionId: id, scope: selection.scope });
      return id === sessionId ? row() : undefined;
    },
    async list(_projectPath: string) { return { sessions: [row()] }; },
    invalidate() {},
  };
  const handlers = new Map<string, (...args: any[]) => Promise<any>>();
  const dialogs: WorkspaceDialogs = {
    async pickDirectory() { return undefined; },
    async trustDirectory(directory) { calls.trust.push(directory); calls.events.push(`trust:${directory}`); return true; },
  };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), catalog as any,
    { handle: (name: string, handler: any) => handlers.set(name, handler), removeHandler: (name: string) => handlers.delete(name) } as any,
    undefined, undefined, undefined, undefined, dialogs);
  (host as any).bootstrap = () => ({ settings: settings.getPublic(), projectCatalogs: Object.fromEntries((host as any).catalogs) });
  const frame = { url: 'http://127.0.0.1:4242' };
  const sender = { mainFrame: frame, isDestroyed: () => false, once() {}, send() {} };
  host.registerWindow(sender as any, frame.url); host.registerIpc();
  const invoke = (channel: string, ...extra: unknown[]) => handlers.get(channel)!({ sender, senderFrame: frame }, ref.projectPath, ref.sessionId, ...extra);
  t.after(() => { host.dispose(); rmSync(root, { recursive: true, force: true }); });
  return { root, alias, source, ref, sourceRef, path, untouchedPath, original, settings, runtime, runtimes, calls, catalog, dialogs, host, invoke,
    setHint: (value: string) => { hint = value; } };
}

test('alias rename writes the live source transcript without starting, replacing or selecting an engine', async (t) => {
  const f = fixture(t);
  f.runtime.hasActiveWork = true;
  const before = readFileSync(f.settings.settingsPath, 'utf8');
  const result = await f.invoke(CH_SESSION_RENAME, 'Renamed in B');
  assert.equal(result.projectPath, f.alias);
  assert.equal(result.sessions[0].title, 'Renamed in B');
  assert.deepEqual(f.calls.find, []); assert.deepEqual(f.calls.select, []);
  assert.equal(readFileSync(f.path, 'utf8'), f.original + `${JSON.stringify({ type: 'custom-title', customTitle: 'Renamed in B', sessionId })}\n`);
  assert.equal(readFileSync(f.untouchedPath, 'utf8'), 'A remains unchanged\n');
  assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), before);
  assert.equal(f.runtimes.get(sessionId), f.runtime);
  assert.deepEqual(f.calls.background, []); assert.deepEqual(f.calls.closed, []); assert.deepEqual(f.calls.trust, []);
  assert.deepEqual(f.ref, { projectPath: f.alias, sessionId });
  assert.equal((f.host as any).catalogs.get(f.source).sessions[0].title, 'Renamed in B');
});

test('alias touch updates only the source transcript and leaves active work in place', async (t) => {
  const f = fixture(t);
  f.runtime.hasActiveWork = true;
  const untouched = statSync(f.untouchedPath).mtimeMs, before = readFileSync(f.settings.settingsPath, 'utf8');
  const result = await f.invoke(CH_SESSION_TOUCH);
  assert.equal(result.projectPath, f.alias);
  assert.ok(statSync(f.path).mtimeMs > Date.parse('2020-01-01T00:00:00Z'));
  assert.equal(statSync(f.untouchedPath).mtimeMs, untouched);
  assert.equal(readFileSync(f.path, 'utf8'), f.original);
  assert.deepEqual(f.calls.find, []); assert.deepEqual(f.calls.select, []);
  assert.deepEqual(f.calls.background, []); assert.deepEqual(f.calls.closed, []);
  assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), before);
  assert.equal(f.runtimes.get(sessionId), f.runtime);
});

test('alias archive records B ownership and filters the archived UUID from B and the alias list', async (t) => {
  const f = fixture(t);
  await f.invoke(CH_SESSION_ARCHIVE);
  assert.equal(f.settings.isSessionArchived(f.sourceRef), true);
  assert.equal(f.settings.isSessionArchived(f.ref), false);
  assert.deepEqual(f.calls.background, [f.sourceRef]);
  assert.deepEqual(f.calls.closed, [f.sourceRef]);
  assert.deepEqual(f.calls.replacements, [f.source]);
  assert.equal((f.host as any).catalogs.get(f.source).sessions.length, 0);
  assert.equal((f.host as any).catalogs.get(f.alias).sessions.length, 0);
  assert.equal(readFileSync(f.path, 'utf8'), f.original);
  assert.deepEqual(f.ref, { projectPath: f.alias, sessionId });
});

test('cold alias archive trusts and registers the actual source before starting it', async (t) => {
  const f = fixture(t, { running: false, savedSource: false });
  await f.invoke(CH_SESSION_ARCHIVE);
  assert.deepEqual(f.calls.find, []);
  assert.deepEqual(f.calls.select, [{ ...f.ref, scope: 'current' }]);
  assert.deepEqual(f.calls.transcriptPaths, [f.path]);
  assert.deepEqual(f.calls.trust, [f.source]);
  assert.deepEqual(f.calls.events, [`trust:${f.source}`, `start:${f.source}`]);
  assert.deepEqual(f.calls.started, [f.sourceRef]);
  assert.equal(f.settings.hasProject(f.source), true);
  assert.equal(f.settings.isSessionArchived(f.sourceRef), true);
  assert.equal(f.settings.isSessionArchived(f.ref), false);
});

test('alias archive preflight inspects the source runtime and returns only its dependent jobs', async (t) => {
  for (const running of [false, true]) {
    const f = fixture(t, { running, savedSource: running });
    const before = readFileSync(f.settings.settingsPath, 'utf8');
    const result = await f.invoke(CH_SESSION_ARCHIVE_PREFLIGHT);
    assert.deepEqual(result.map((job: { id: string }) => job.id), ['owned']);
    assert.deepEqual(f.calls.background, [f.sourceRef]);
    assert.deepEqual(f.calls.started, running ? [] : [f.sourceRef]);
    assert.deepEqual(f.calls.trust, running ? [] : [f.source]);
    assert.deepEqual(f.calls.transcriptPaths, running ? [undefined] : [f.path]);
    assert.deepEqual(f.calls.closed, []); assert.deepEqual(f.calls.replacements, []);
    assert.equal(f.settings.isSessionArchived(f.sourceRef), false);
    assert.equal(readFileSync(f.path, 'utf8'), f.original);
    if (running) {
      assert.equal(f.runtimes.get(sessionId), f.runtime);
      assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), before);
    }
  }
});

test('canceling source trust leaves archive and preflight data and engines untouched', async (t) => {
  for (const channel of [CH_SESSION_ARCHIVE, CH_SESSION_ARCHIVE_PREFLIGHT]) {
    const f = fixture(t, { running: false, savedSource: false });
    const before = readFileSync(f.settings.settingsPath, 'utf8');
    f.dialogs.trustDirectory = async () => false;
    await assert.rejects(f.invoke(channel), /cancelled/);
    assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), before);
    assert.equal(readFileSync(f.path, 'utf8'), f.original);
    assert.deepEqual(f.calls.background, []); assert.deepEqual(f.calls.started, []);
    assert.deepEqual(f.calls.closed, []); assert.equal(f.runtimes.size, 0);
  }
});

test('missing or rejected source metadata cannot change settings, transcripts or the live engine', async (t) => {
  for (const channel of [CH_SESSION_RENAME, CH_SESSION_TOUCH]) {
    const f = fixture(t, { running: false });
    const before = readFileSync(f.settings.settingsPath, 'utf8'), modified = statSync(f.path).mtimeMs;
    f.catalog.select = async () => { throw new Error('source catalog unavailable'); };
    await assert.rejects(f.invoke(channel, 'Cannot rename'), /source catalog unavailable/);
    assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), before);
    assert.equal(readFileSync(f.path, 'utf8'), f.original);
    assert.equal(statSync(f.path).mtimeMs, modified);
    assert.equal(f.runtimes.size, 0);
    assert.deepEqual(f.calls.closed, []); assert.deepEqual(f.calls.background, []);
  }
});

test('alias archive rejects active source work without replacing or closing the engine', async (t) => {
  const f = fixture(t);
  f.runtime.hasActiveWork = true;
  const before = readFileSync(f.settings.settingsPath, 'utf8');
  await assert.rejects(f.invoke(CH_SESSION_ARCHIVE), /active work/);
  assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), before);
  assert.equal(readFileSync(f.path, 'utf8'), f.original);
  assert.equal(f.runtimes.get(sessionId), f.runtime);
  assert.deepEqual(f.calls.closed, []); assert.deepEqual(f.calls.replacements, []);
});

test('metadata source resolution does not broaden terminal compound-scope validation', async (t) => {
  const f = fixture(t);
  await assert.rejects((f.host as any).assertSessionBelongsToProject(f.ref), /different project/);
  await assert.rejects((f.host as any).validateSessionScope(f.ref), /different project/);
  assert.equal(f.runtimes.get(sessionId), f.runtime);
});

test('cold metadata keeps the stored transcript authoritative when the workspace hint disappeared', async (t) => {
  const f = fixture(t, { running: false, savedSource: false });
  f.setHint(join(f.root, 'removed-worktree'));
  const before = readFileSync(f.settings.settingsPath, 'utf8');
  await f.invoke(CH_SESSION_RENAME, 'Stored source remains usable');
  assert.match(readFileSync(f.path, 'utf8'), /Stored source remains usable/);
  assert.deepEqual(f.calls.background, []); assert.deepEqual(f.calls.trust, []);
  assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), before);
});

test('live metadata follows the exact active copy even when its logical workspace has a different copy', async (t) => {
  const f = fixture(t);
  f.runtime.projectPath = f.alias;
  f.runtime.hasActiveWork = true;
  const otherPath = join(f.alias, `${sessionId}.jsonl`);
  writeFileSync(otherPath, f.original.replace('Saved chat', 'Unselected A chat'));
  const bytes = readFileSync(otherPath), modified = statSync(otherPath).mtimeMs;
  const settings = readFileSync(f.settings.settingsPath, 'utf8');
  f.catalog.find = async () => { throw new Error('must not resolve a live selected UUID again'); };
  f.catalog.select = async () => { throw new Error('must not discover a live selected UUID again'); };
  await f.invoke(CH_SESSION_RENAME, 'Selected other copy');
  await f.invoke(CH_SESSION_TOUCH);
  assert.match(readFileSync(f.path, 'utf8'), /Selected other copy/);
  assert.deepEqual(readFileSync(otherPath), bytes);
  assert.equal(statSync(otherPath).mtimeMs, modified);
  assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), settings);
  assert.equal(f.runtimes.get(sessionId), f.runtime);
  assert.deepEqual(f.calls.background, []); assert.deepEqual(f.calls.closed, []);
});

test('live metadata rejects invalid or non-file writer targets without modifying either copy', async (t) => {
  for (const channel of [CH_SESSION_RENAME, CH_SESSION_TOUCH]) {
    const f = fixture(t);
    const directory = join(f.root, `${sessionId}.jsonl`);
    mkdirSync(directory);
    for (const path of ['relative.jsonl', join(f.root, `${replacementId}.jsonl`), directory]) {
      f.runtime.getWorkspaceState = async () => ({ cwd: f.source, additional_directories: [], transcript_path: path });
      const modified = statSync(f.path).mtimeMs;
      await assert.rejects(f.invoke(channel, 'Rejected title'), /authoritative transcript|not a file/);
      assert.equal(readFileSync(f.path, 'utf8'), f.original);
      assert.equal(statSync(f.path).mtimeMs, modified);
      assert.equal(f.runtimes.get(sessionId), f.runtime);
    }
  }
});

test('a disconnected owner cannot fall back to a different catalog copy for metadata actions', async (t) => {
  for (const channel of [CH_SESSION_RENAME, CH_SESSION_TOUCH, CH_SESSION_ARCHIVE, CH_SESSION_ARCHIVE_PREFLIGHT]) {
    const f = fixture(t);
    f.runtime.connectionState.status = 'disconnected';
    const settings = readFileSync(f.settings.settingsPath, 'utf8'), modified = statSync(f.path).mtimeMs;
    await assert.rejects(f.invoke(channel, 'Must keep the selected copy'), /Reconnect this chat/);
    assert.deepEqual(f.calls.select, []); assert.deepEqual(f.calls.find, []);
    assert.deepEqual(f.calls.background, []); assert.deepEqual(f.calls.closed, []);
    assert.equal(f.runtimes.get(sessionId), f.runtime);
    assert.equal(readFileSync(f.path, 'utf8'), f.original);
    assert.equal(statSync(f.path).mtimeMs, modified);
    assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), settings);
  }
});

function scopedCatalog(f: ReturnType<typeof fixture>, globalDuplicates: boolean) {
  const requests: string[][] = [];
  const otherPath = join(f.alias, `${sessionId}.jsonl`);
  if (!globalDuplicates) writeFileSync(otherPath, f.original.replace('Saved chat', 'Old A chat'));
  const row = (path: string, workspace: string): ProjectSessionCatalogRow => ({
    uuid: sessionId, title: readFileSync(path, 'utf8').trim().split('\n').map((line) => JSON.parse(line)).at(-1)?.customTitle ?? 'Saved chat',
    path, workspace_path: workspace, modified_rfc3339: statSync(path).mtime.toISOString(),
    message_count: 1, mode: 'code', empty_session: false,
  });
  const catalog = new ProjectSessionCatalog({ serverBin: '/fixture-bridge', spawnProcess: ((_bin, args) => {
    const argv = [...args as string[]]; requests.push(argv);
    const child = new EventEmitter() as EventEmitter & { stdout: EventEmitter; stderr: EventEmitter; kill(): boolean };
    child.stdout = new EventEmitter(); child.stderr = new EventEmitter(); child.kill = () => true;
    queueMicrotask(() => {
      if (!argv.includes('--session-scope') && globalDuplicates) {
        child.stderr.emit('data', 'session UUID has multiple global copies'); child.emit('close', 1, null);
      } else {
        const selected = argv.includes('--session-scope') ? row(f.path, f.source) : row(otherPath, f.alias);
        child.stdout.emit('data', JSON.stringify({ version: 1, sessions: [selected] })); child.emit('close', 0, null);
      }
    });
    return child;
  }) as any });
  (f.host as any).sessionCatalog = catalog;
  return { catalog, requests, otherPath };
}

test('cold rename and touch use fresh Current alias selection rather than the CLI UUID resolver', async (t) => {
  for (const duplicates of [false, true]) for (const channel of [CH_SESSION_RENAME, CH_SESSION_TOUCH]) {
    const f = fixture(t, { running: false });
    const adapter = scopedCatalog(f, duplicates);
    if (duplicates) await assert.rejects(adapter.catalog.find(f.alias, sessionId), /multiple global copies/);
    else assert.equal((await adapter.catalog.find(f.alias, sessionId))?.path, adapter.otherPath);
    const otherBytes = duplicates ? undefined : readFileSync(adapter.otherPath);
    const modified = statSync(f.path).mtimeMs;
    await f.invoke(channel, 'Newest Current alias');
    assert.equal(adapter.requests[1][adapter.requests[1].indexOf('--session-scope') + 1], 'current');
    assert.equal(adapter.requests[1].includes('--session-id'), true);
    if (channel === CH_SESSION_RENAME) assert.match(readFileSync(f.path, 'utf8'), /Newest Current alias/);
    else assert.ok(statSync(f.path).mtimeMs > modified);
    if (otherBytes) assert.deepEqual(readFileSync(adapter.otherPath), otherBytes);
    assert.deepEqual(f.calls.background, []); assert.deepEqual(f.calls.trust, []);
  }
});

test('cold archive and preflight carry the exact Current alias transcript through background startup', async (t) => {
  for (const channel of [CH_SESSION_ARCHIVE, CH_SESSION_ARCHIVE_PREFLIGHT]) {
    const f = fixture(t, { running: false, savedSource: false });
    const adapter = scopedCatalog(f, true);
    await f.invoke(channel);
    assert.deepEqual(f.calls.transcriptPaths, [f.path]);
    assert.deepEqual(f.calls.started, [f.sourceRef]);
    assert.equal(adapter.requests[0][adapter.requests[0].indexOf('--session-scope') + 1], 'current');
    if (channel === CH_SESSION_ARCHIVE) {
      assert.equal(f.settings.isSessionArchived(f.sourceRef), true);
      assert.equal((f.host as any).catalogs.get(f.alias).sessions.length, 0);
      assert.equal((f.host as any).catalogs.get(f.source).sessions.length, 0);
    }
  }
});
