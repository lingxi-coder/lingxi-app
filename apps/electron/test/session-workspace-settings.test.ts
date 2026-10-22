import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test, type TestContext } from 'node:test';

import { SettingsStore } from '../src/main/settings';

const sessionId = '11111111-2222-4333-8444-555555555555';
const otherId = '22222222-3333-4444-8555-666666666666';

function settingsFixture(t: TestContext) {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-workspace-settings-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const userData = join(root, 'settings'), old = join(root, 'old'), target = join(root, 'next');
  mkdirSync(old); mkdirSync(target);
  const settings = new SettingsStore(userData);
  settings.addProject(old); settings.addProject(target); settings.activateProject(old);
  const ref = { projectPath: old, sessionId }, moved = { projectPath: target, sessionId };
  settings.setActiveSession(ref);
  return { root, userData, old, target, settings, ref, moved };
}

test('relocation atomically moves active identity, model, pin and manual order while preserving other sessions', (t) => {
  const f = settingsFixture(t);
  f.settings.setSessionModel(f.ref, 'anthropic/selected-model');
  f.settings.setSessionModel({ projectPath: f.old, sessionId: otherId }, 'openai/other-model');
  const pinnedAt = '2026-10-21T00:00:00Z';
  f.settings.setSessionPinned({ ...f.ref, title: 'Pinned chat', pinnedAt }, true);
  f.settings.update({ sidebar: { organization: 'project', chatSort: 'manual', manualSessionOrder: {
    [f.old]: [otherId, sessionId], [f.target]: [otherId],
  } } });
  f.settings.relocateSession(f.ref, f.target);
  const saved = new SettingsStore(f.userData);
  const state = saved.getPublic();
  assert.deepEqual(state.activeSession, f.moved);
  assert.equal(state.activeProject, f.target);
  assert.equal(saved.getSessionModel(f.ref), undefined);
  assert.equal(saved.getSessionModel(f.moved), 'anthropic/selected-model');
  assert.equal(saved.getSessionModel({ projectPath: f.old, sessionId: otherId }), 'openai/other-model');
  assert.deepEqual(state.pinnedSessions, [{ ...f.moved, title: 'Pinned chat', pinnedAt }]);
  assert.deepEqual(state.sidebar?.manualSessionOrder, { [f.old]: [otherId], [f.target]: [otherId, sessionId] });
});

test('relocation carries archived metadata to the same UUID in the new directory', (t) => {
  const f = settingsFixture(t);
  f.settings.setSessionArchived(f.ref, true, 'Archived chat');
  const before = f.settings.getPublic().archivedSessions?.[0];
  f.settings.relocateSession(f.ref, f.target);
  assert.equal(f.settings.isSessionArchived(f.ref), false);
  assert.equal(f.settings.isSessionArchived(f.moved), true);
  const saved = new SettingsStore(f.userData);
  assert.deepEqual(saved.getPublic().archivedSessions, [{ ...before, projectPath: f.target }]);
});

test('relocation carries Remote Control enablement to the new directory', (t) => {
  const f = settingsFixture(t);
  const other = { projectPath: f.old, sessionId: otherId };
  f.settings.setRemoteSession(f.ref, true);
  f.settings.setRemoteSession(other, true);
  f.settings.relocateSession(f.ref, f.target);
  const saved = new SettingsStore(f.userData);
  assert.equal(saved.isRemoteSession(sessionId), true);
  assert.deepEqual(saved.getRemoteSessions(), [f.moved, other]);
});

test('relocation carries a volatile draft without overwriting the last durable session', (t) => {
  const f = settingsFixture(t);
  f.settings.setActiveSession({ projectPath: f.old, sessionId: otherId });
  f.settings.setActiveSessionDraft(f.ref);
  f.settings.relocateSession(f.ref, f.target);
  assert.deepEqual(f.settings.getPublic().activeSession, f.moved);
  const saved = new SettingsStore(f.userData);
  assert.deepEqual(saved.getPublic().activeSession, { projectPath: f.old, sessionId: otherId });
  assert.equal(saved.getPublic().activeProject, f.target);
});

test('moving an unsent first draft persists its directory without a nonexistent resume target', (t) => {
  const f = settingsFixture(t);
  f.settings.setActiveSession(undefined);
  f.settings.setActiveSessionDraft(f.ref);
  f.settings.relocateSession(f.ref, f.target);
  assert.deepEqual(f.settings.getPublic().activeSession, f.moved);
  const saved = new SettingsStore(f.userData);
  assert.equal(saved.getPublic().activeSession, undefined);
  assert.equal(saved.getPublic().activeProject, f.target);
});

test('a failed relocation write preserves the previous file, memory, pin and volatile active draft', (t) => {
  const f = settingsFixture(t);
  f.settings.setSessionModel(f.ref, 'anthropic/selected-model');
  f.settings.setSessionPinned({ ...f.ref, title: 'Pinned chat', pinnedAt: '2026-10-21T00:00:00Z' }, true);
  f.settings.setActiveSessionDraft(f.ref);
  const before = f.settings.getPublic(), bytes = readFileSync(f.settings.settingsPath, 'utf8');
  mkdirSync(`${f.settings.settingsPath}.tmp`);
  assert.throws(() => f.settings.relocateSession(f.ref, f.target), /EISDIR|illegal operation|directory/i);
  assert.deepEqual(f.settings.getPublic(), before);
  assert.equal(readFileSync(f.settings.settingsPath, 'utf8'), bytes);
  assert.equal(f.settings.getSessionModel(f.ref), 'anthropic/selected-model');
  assert.equal(f.settings.getSessionModel(f.moved), undefined);
});
