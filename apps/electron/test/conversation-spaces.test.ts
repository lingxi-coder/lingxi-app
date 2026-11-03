import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { ConversationSpaceStore } from '../src/main/conversationSpaceStore.js';
import { SettingsStore } from '../src/main/settings.js';

const id = '11111111-2222-4333-8444-555555555555';

test('project membership and independent navigation survive restart without assigning an unprojected chat', (t) => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'conversation-spaces-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const store = new ConversationSpaceStore(root, join(root, '.lingxi'));
  const project = join(root, 'existing-project');
  store.addProject(project, 'code');
  store.addProject(project, 'chat');
  const newProject = join(root, 'new-project');
  store.addProject(newProject, 'chat');
  const chat = { projectPath: store.globalWorkspaces.chat, projectId: null, sessionId: id, mode: 'chat' as const };
  store.register(chat);
  store.setNavigation('chat', null, chat);
  store.setNavigation('code', project);
  store.setMode('code');
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

test('older space configuration is ignored and current data requires an explicit owner mode', (t) => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'conversation-spaces-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const project = join(root, 'old-project');
  writeFileSync(join(root, 'conversation-spaces.v1.json'), JSON.stringify({
    version: 1, activeMode: 'code', projects: { [project]: ['code'] }, migrationPending: [project],
  }));
  const store = new ConversationSpaceStore(root, join(root, '.lingxi'));
  assert.equal(store.snapshot().activeMode, 'chat');
  assert.deepEqual(store.snapshot().projects, {});
  assert.throws(() => store.register({ projectPath: project, projectId: project, sessionId: id } as any), /Invalid conversation owner/);
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

test('owner cannot switch mode or project and workspace relocation keeps both', (t) => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'conversation-spaces-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const store = new ConversationSpaceStore(root, join(root, '.lingxi'));
  const chat = { projectPath: store.globalWorkspaces.chat, projectId: null, sessionId: id, mode: 'chat' as const };
  store.register(chat);
  store.setNavigation('chat', null, chat);
  assert.throws(() => store.register({ ...chat, mode: 'code' }), /ownership cannot change/);
  assert.throws(() => store.register({ ...chat, projectId: join(root, 'project') }), /ownership cannot change/);
  assert.throws(() => store.register({ ...chat, projectPath: join(root, 'other-workspace'), mode: 'code' }), /ownership cannot change/);
  assert.equal(store.findBySessionId(id)?.mode, 'chat');
  const moved = store.relocate(chat, join(root, 'other-workspace'));
  assert.deepEqual(moved, { ...chat, projectPath: join(root, 'other-workspace') });
  assert.deepEqual(new ConversationSpaceStore(root, join(root, '.lingxi')).snapshot().navigation.chat.session, moved);
});

test('a persisted UUID cannot name two owners in different workspaces', (t) => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'conversation-spaces-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const store = new ConversationSpaceStore(root, join(root, '.lingxi'));
  store.register({ projectPath: store.globalWorkspaces.chat, projectId: null, sessionId: id, mode: 'chat' });
  const file = join(root, 'conversation-spaces.v2.json');
  const contents = JSON.parse(readFileSync(file, 'utf8'));
  contents.sessions.push({ projectPath: join(root, 'other'), projectId: null, sessionId: id, mode: 'code' });
  writeFileSync(file, JSON.stringify(contents));
  assert.throws(() => new ConversationSpaceStore(root, join(root, '.lingxi')), /Duplicate conversation owner/);
  assert.equal(readFileSync(file, 'utf8'), JSON.stringify(contents));
});

test('Global workspace honors the explicit brand home instead of Electron userData', (t) => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'conversation-brand-root-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const previous = process.env.LINGXI_CONFIG_DIR;
  try {
    process.env.LINGXI_CONFIG_DIR = join(root, '.atlas');
    const store = new ConversationSpaceStore(root);
    assert.equal(store.globalWorkspaces.chat, join(root, '.atlas', 'chat'));
    assert.equal(store.globalWorkspaces.code, join(root, '.atlas', 'code'));
    process.env.LINGXI_CONFIG_DIR = 'relative-brand-home';
    assert.throws(() => new ConversationSpaceStore(root), /absolute path/);
  } finally {
    if (previous === undefined) delete process.env.LINGXI_CONFIG_DIR;
    else process.env.LINGXI_CONFIG_DIR = previous;
  }
});
