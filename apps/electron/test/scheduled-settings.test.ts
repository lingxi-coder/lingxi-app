import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { test, type TestContext } from 'node:test';
import { SettingsStore } from '../src/main/settings';
import { MAX_PROJECTS } from '../src/main/host-utils';

const sessionId = '11111111-2222-4333-8444-555555555555';
async function directory(t: TestContext) {
  const path = await mkdtemp(join(tmpdir(), 'lingxi-scheduled-settings-'));
  t.after(() => rm(path, { recursive: true, force: true }));
  return path;
}

test('managed no-project chat active selection and pin survive restarting with no saved projects', async (t) => {
  const path = await directory(t);
  const settings = new SettingsStore(path);
  const ref = { projectPath: settings.scheduledWorkspace, sessionId };
  settings.activateProject(ref.projectPath);
  settings.setActiveSession(ref);
  settings.setSessionPinned({ ...ref, title: 'Morning brief', pinnedAt: '2026-09-12T10:00:00Z' }, true);
  const reopened = new SettingsStore(path);
  assert.deepEqual(reopened.getPublic().projects, []);
  assert.equal(reopened.getWorkspace(), ref.projectPath);
  assert.deepEqual(reopened.getPublic().activeSession, ref);
  assert.equal(reopened.getPublic().pinnedSessions.length, 1);
  assert.equal(reopened.getPublic().pinnedSessions[0].title, 'Morning brief');
  assert.equal(reopened.isTrustedWorkspace(ref.projectPath), true);
  assert.equal(reopened.hasProject(ref.projectPath), false);
});

test('archiving and restoring a managed chat persist without promoting its scope to a project', async (t) => {
  const path = await directory(t);
  const settings = new SettingsStore(path);
  const ref = { projectPath: settings.scheduledWorkspace, sessionId };
  settings.setActiveSession(ref);
  settings.setSessionPinned({ ...ref, title: 'Dedicated monitor', pinnedAt: '2026-09-12T10:00:00Z' }, true);
  settings.setSessionArchived(ref, true, 'Dedicated monitor');
  const archived = new SettingsStore(path);
  assert.equal(archived.isSessionArchived(ref), true);
  assert.equal(archived.getPublic().activeSession, undefined);
  assert.deepEqual(archived.getPublic().pinnedSessions, []);
  assert.deepEqual(archived.getPublic().projects, []);
  assert.throws(() => archived.setSessionPinned({ ...ref, title: 'Archived', pinnedAt: '2026-09-12T10:00:00Z' }, true), /Restore/);
  archived.setSessionArchived(ref, false);
  archived.setActiveSession(ref);
  const restored = new SettingsStore(path);
  assert.equal(restored.isSessionArchived(ref), false);
  assert.deepEqual(restored.getPublic().activeSession, ref);
  assert.deepEqual(restored.getPublic().projects, []);
});

test('managed settings restoration preserves a full user project catalog and both pin scopes', async (t) => {
  const path = await directory(t);
  const settings = new SettingsStore(path);
  for (let index = 0; index < MAX_PROJECTS; index++) settings.addProject(join(path, `project-${index}`));
  const projects = settings.getPublic().projects;
  const normal = { projectPath: projects[0], sessionId: '22222222-3333-4444-8555-666666666666' };
  const managed = { projectPath: settings.scheduledWorkspace, sessionId };
  settings.setSessionPinned({ ...normal, title: 'Project chat', pinnedAt: '2026-09-12T10:00:00Z' }, true);
  settings.setSessionPinned({ ...managed, title: 'General chat', pinnedAt: '2026-09-12T11:00:00Z' }, true);
  settings.activateProject(managed.projectPath);
  settings.setActiveSession(managed);
  let reopened = new SettingsStore(path);
  assert.deepEqual(reopened.getPublic().projects, projects);
  assert.equal(reopened.getPublic().pinnedSessions.length, 2);
  assert.deepEqual(reopened.getPublic().activeSession, managed);
  reopened.update({ theme: 'dark' });
  reopened = new SettingsStore(path);
  assert.deepEqual(reopened.getPublic().projects, projects);
  assert.equal(reopened.getPublic().pinnedSessions.length, 2, 'repeated load/save does not duplicate managed pins');
});

test('unregistered arbitrary workspace cannot become active or archived', async (t) => {
  const path = await directory(t);
  const settings = new SettingsStore(path);
  const ref = { projectPath: join(path, 'unregistered'), sessionId };
  assert.throws(() => settings.setActiveSession(ref), /project/);
  assert.throws(() => settings.setSessionArchived(ref, true), /project/);
  assert.equal(settings.isTrustedWorkspace(ref.projectPath), false);
});
