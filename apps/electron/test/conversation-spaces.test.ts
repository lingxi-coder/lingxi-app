import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, renameSync, rmSync, statSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { ConversationSpaceStore } from '../src/main/conversationSpaceStore.js';
import { SettingsStore } from '../src/main/settings.js';

const id = '11111111-2222-4333-8444-555555555555';

test('project membership and independent navigation survive restart without assigning an unprojected chat', async (t) => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'conversation-spaces-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const store = new ConversationSpaceStore(root, join(root, '.lingxi'));
  const project = join(root, 'existing-project');
  await store.addProject(project, 'code');
  await store.addProject(project, 'chat');
  const newProject = join(root, 'new-project');
  await store.addProject(newProject, 'chat');
  const chat = { projectPath: store.globalWorkspaces.chat, projectId: null, sessionId: id, mode: 'chat' as const };
  await store.register(chat);
  await store.setNavigation('chat', null, chat);
  await store.setNavigation('code', project);
  await store.setMode('code');
  const reopenedStore = new ConversationSpaceStore(root, join(root, '.lingxi'));
  const reopened = reopenedStore.snapshot();
  assert.equal(reopened.activeMode, 'code');
  assert.deepEqual(reopened.projects[project], ['code', 'chat']);
  assert.deepEqual(reopened.projects[newProject], ['chat']);
  assert.deepEqual(reopened.navigation.chat, { projectId: null, session: chat });
  assert.deepEqual(reopened.navigation.code, { projectId: project });
  assert.equal(store.globalWorkspaces.chat, join(root, '.lingxi', 'chat'));
  assert.equal(store.globalWorkspaces.code, join(root, '.lingxi', 'code'));
  assert.notEqual(store.globalWorkspaces.chat, store.globalWorkspaces.code);
  assert.notEqual(store.globalWorkspaces.chat, join(root, 'scheduled-workspace'));
});

test('older space configuration is ignored and current data requires an explicit owner mode', async (t) => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'conversation-spaces-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const project = join(root, 'old-project');
  writeFileSync(join(root, 'conversation-spaces.v1.json'), JSON.stringify({
    version: 1, activeMode: 'code', projects: { [project]: ['code'] }, migrationPending: [project],
  }));
  const store = new ConversationSpaceStore(root, join(root, '.lingxi'));
  assert.equal(store.snapshot().activeMode, 'chat');
  assert.deepEqual(store.snapshot().projects, {});
  await assert.rejects(async () => await store.register({ projectPath: project, projectId: project, sessionId: id } as any), /Invalid conversation owner/);
  writeFileSync(join(root, 'conversation-spaces.v2.json'), JSON.stringify({ version: 1 }));
  assert.throws(() => new ConversationSpaceStore(root, join(root, '.lingxi')), /Invalid conversation spaces/);
});

test('a corrupt ownership registry is rejected without overwriting its contents', (t) => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'conversation-corrupt-registry-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const file = join(root, 'conversation-spaces.v2.json');
  const invalid = '{"version":2,"sessions":[';
  writeFileSync(file, invalid);
  assert.throws(() => new ConversationSpaceStore(root, join(root, '.lingxi')));
  assert.equal(readFileSync(file, 'utf8'), invalid);
});

test('a mode workspace cannot redirect through a symlink', { skip: process.platform === 'win32' }, (t) => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'conversation-mode-symlink-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const brand = join(root, '.lingxi'), outside = join(root, 'outside');
  mkdirSync(brand); mkdirSync(outside);
  symlinkSync(outside, join(brand, 'chat'));
  assert.throws(() => new ConversationSpaceStore(root, brand), /real directory/);
});

test('old active, pinned and archived references without current owners do not reappear', (t) => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'conversation-spaces-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const project = join(root, 'old-project');
  const ref = { projectPath: project, sessionId: id };
  const pin = { ...ref, sessionId: '22222222-3333-4444-8555-666666666666' };
  const archive = { ...ref, sessionId: '33333333-4444-4555-8666-777777777777' };
  writeFileSync(join(root, 'settings.v1.json'), JSON.stringify({ version: 1, projects: [project],
    activeProject: project, activeSession: ref, pinnedSessions: [{ ...pin, title: 'old', pinnedAt: '2026-10-01T00:00:00Z' }],
    archivedSessions: [{ ...archive, title: 'old', archivedAt: '2026-10-01T00:00:00Z' }],
    trustedWorkspaces: {}, lastSessionMode: 'code' }));
  const settings = new SettingsStore(root);
  assert.equal(settings.getPublic().activeSession, undefined);
  assert.deepEqual(settings.getPublic().pinnedSessions, []);
  assert.deepEqual(settings.getPublic().archivedSessions, []);
  assert.deepEqual(settings.conversationSpaces.snapshot().projects, {});
  assert.equal(settings.conversationSpaces.snapshot().activeMode, 'chat');
  settings.addProject(project);
  const persisted = JSON.parse(readFileSync(settings.settingsPath, 'utf8'));
  assert.equal('lastSessionMode' in persisted, false);
  assert.equal('activeSession' in persisted, false);
  assert.deepEqual(persisted.pinnedSessions, []);
  assert.deepEqual(persisted.archivedSessions, []);
});

test('owner cannot switch mode or project and workspace relocation keeps both', async (t) => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'conversation-spaces-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const store = new ConversationSpaceStore(root, join(root, '.lingxi'));
  const chat = { projectPath: store.globalWorkspaces.chat, projectId: null, sessionId: id, mode: 'chat' as const };
  await store.register(chat);
  await store.setNavigation('chat', null, chat);
  await assert.rejects(async () => await store.register({ ...chat, mode: 'code' }), /ownership cannot change/);
  await assert.rejects(async () => await store.register({ ...chat, projectId: join(root, 'project') }), /ownership cannot change/);
  await assert.rejects(async () => await store.register({ ...chat, projectPath: join(root, 'other-workspace'), mode: 'code' }), /ownership cannot change/);
  assert.equal(store.findBySessionId(id)?.mode, 'chat');
  const moved = await store.relocate(chat, join(root, 'other-workspace'));
  assert.deepEqual(moved, { ...chat, projectPath: join(root, 'other-workspace') });
  assert.deepEqual(new ConversationSpaceStore(root, join(root, '.lingxi')).snapshot().navigation.chat.session, moved);
});

test('a persisted UUID cannot name two owners in different workspaces', async (t) => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'conversation-spaces-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const store = new ConversationSpaceStore(root, join(root, '.lingxi'));
  await store.register({ projectPath: store.globalWorkspaces.chat, projectId: null, sessionId: id, mode: 'chat' });
  const file = join(root, 'conversation-spaces.v2.json');
  const contents = JSON.parse(readFileSync(file, 'utf8'));
  contents.sessions.push({ projectPath: join(root, 'other'), projectId: null, sessionId: id, mode: 'code' });
  writeFileSync(file, JSON.stringify(contents));
  assert.throws(() => new ConversationSpaceStore(root, join(root, '.lingxi')), /Duplicate conversation owner/);
  assert.equal(readFileSync(file, 'utf8'), JSON.stringify(contents));
});

test('Global workspace uses the native workspace root without reading config-home environment', (t) => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'conversation-brand-root-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const workspaceRoot = join(root, '.atlas-workspaces');
  const store = new ConversationSpaceStore(root, workspaceRoot);
  assert.equal(store.globalWorkspaces.chat, join(workspaceRoot, 'chat'));
  assert.equal(store.globalWorkspaces.code, join(workspaceRoot, 'code'));
  assert.throws(() => new ConversationSpaceStore(root, 'relative'), /absolute path/);
});

function deferred<T = void>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(yes => { resolve = yes; });
  return { promise, resolve };
}

test('identical navigation performs no file replacement or write even when staging is unavailable', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'conversation-navigation-noop-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const store = new ConversationSpaceStore(root);
  const owned = { projectPath: store.globalWorkspaces.chat, projectId: null, sessionId: id, mode: 'chat' as const };
  await store.register(owned); await store.setNavigation('chat', null, owned);
  const file = join(root, 'conversation-spaces.v2.json');
  const before = readFileSync(file, 'utf8');
  mkdirSync(`${file}.tmp`);
  await store.setNavigation('chat', null, structuredClone(owned));
  await store.setMode('chat');
  assert.equal(readFileSync(file, 'utf8'), before);
  assert.deepEqual(new ConversationSpaceStore(root).snapshot().navigation.chat.session, owned);
});

test('queued owner and navigation transactions validate committed state and preserve concurrent additions', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'conversation-navigation-queue-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const store = new ConversationSpaceStore(root);
  const first = { projectPath: store.globalWorkspaces.chat, projectId: null, sessionId: id, mode: 'chat' as const };
  const second = { projectPath: store.globalWorkspaces.code, projectId: null, sessionId: '22222222-3333-4444-8555-666666666666', mode: 'code' as const };
  const entered = deferred(), release = deferred();
  const stage = (store as any).stage.bind(store);
  let firstWrite = true;
  (store as any).stage = async (value: unknown) => {
    if (firstWrite) { firstWrite = false; entered.resolve(); await release.promise; }
    return stage(value);
  };
  const registering = store.register(first);
  await entered.promise;
  const navigating = store.setNavigation('chat', null, first);
  const adding = store.register(second);
  const conflicting = store.register({ ...first, mode: 'code' });
  const rejected = assert.rejects(conflicting, /ownership cannot change/);
  assert.equal(store.findBySessionId(id), undefined, 'uncommitted ownership stays private');
  assert.equal(store.snapshot().navigation.chat.session, undefined);
  release.resolve();
  await Promise.all([registering, navigating, adding, rejected]);
  const cold = new ConversationSpaceStore(root).snapshot();
  assert.deepEqual(cold.sessions, [first, second]);
  assert.deepEqual(cold.navigation.chat.session, first);
});

test('failed asynchronous persistence keeps committed memory and disk and does not poison later transactions', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'conversation-navigation-failed-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const store = new ConversationSpaceStore(root);
  const owned = { projectPath: store.globalWorkspaces.chat, projectId: null, sessionId: id, mode: 'chat' as const };
  await store.register(owned); await store.setNavigation('chat', null, owned);
  const file = join(root, 'conversation-spaces.v2.json');
  const before = readFileSync(file, 'utf8'), snapshot = store.snapshot();
  mkdirSync(`${file}.tmp`);
  await assert.rejects(store.setMode('code'), /EISDIR|directory|illegal operation/i);
  assert.deepEqual(store.snapshot(), snapshot);
  assert.equal(readFileSync(file, 'utf8'), before);
  rmSync(`${file}.tmp`, { recursive: true });
  await store.setMode('code'); await store.flush();
  assert.equal(new ConversationSpaceStore(root).snapshot().activeMode, 'code');
  if (process.platform !== 'win32') assert.equal(statSync(file).mode & 0o777, 0o600);
});

test('revoking an authority fence during asynchronous selection staging cannot publish owner or navigation', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'conversation-selection-revoked-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const store = new ConversationSpaceStore(root);
  const owned = { projectPath: store.globalWorkspaces.chat, projectId: null, sessionId: id, mode: 'chat' as const };
  await store.register(owned); await store.setNavigation('chat', null, owned);
  const before = store.snapshot(), file = join(root, 'conversation-spaces.v2.json'), bytes = readFileSync(file, 'utf8');
  const entered = deferred(), release = deferred();
  const stage = (store as any).stage.bind(store);
  (store as any).stage = async (value: unknown) => { const result = await stage(value); entered.resolve(); await release.promise; return result; };
  let revoked = false, committed = false;
  const selecting = store.select(owned, join(root, 'relocated'), () => { committed = true; }, async publish => {
    if (revoked) throw new Error('authoring owner replaced');
    publish();
  });
  await entered.promise;
  assert.deepEqual(store.snapshot(), before);
  revoked = true; release.resolve();
  await assert.rejects(selecting, /authoring owner replaced/);
  assert.equal(committed, false);
  assert.deepEqual(store.snapshot(), before);
  assert.equal(readFileSync(file, 'utf8'), bytes);
  assert.deepEqual(new ConversationSpaceStore(root).snapshot(), before);
});

test('a failed synchronous selection commit restores the previous registry before releasing its transaction', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'conversation-selection-rollback-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const store = new ConversationSpaceStore(root);
  const owned = { projectPath: store.globalWorkspaces.chat, projectId: null, sessionId: id, mode: 'chat' as const };
  await store.register(owned); await store.setNavigation('chat', null, owned);
  const before = store.snapshot(), file = join(root, 'conversation-spaces.v2.json'), bytes = readFileSync(file, 'utf8');
  await assert.rejects(store.select(owned, join(root, 'relocated'), () => { throw new Error('settings write failed'); }, async publish => publish()), /settings write failed/);
  assert.deepEqual(store.snapshot(), before);
  assert.equal(readFileSync(file, 'utf8'), bytes);
  await store.setMode('code');
  assert.equal(new ConversationSpaceStore(root).snapshot().activeMode, 'code');
});


test('a failed rollback retains the previous registry for recovery', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'conversation-selection-recovery-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const store = new ConversationSpaceStore(root);
  const owned = { projectPath: store.globalWorkspaces.chat, projectId: null, sessionId: id, mode: 'chat' as const };
  await store.register(owned); await store.setNavigation('chat', null, owned);
  const file = join(root, 'conversation-spaces.v2.json'), before = readFileSync(file, 'utf8');
  await assert.rejects(store.select(owned, join(root, 'relocated'), () => {
    renameSync(file, `${file}.published`); mkdirSync(file);
    throw new Error('settings failed during a filesystem replacement');
  }, async publish => publish()), /previous registry could not be restored/);
  assert.equal(readFileSync(`${file}.rollback`, 'utf8'), before);
});
