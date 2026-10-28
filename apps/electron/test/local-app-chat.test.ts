import assert from 'node:assert/strict';
import { test } from 'node:test';
import { localAppAuthoringPrompt, localAppChatTarget, localAppDraftDisposition, type LocalAppChatNavigation, type LocalAppDraft } from '../src/renderer/bridge/localAppChat';
import type { DesktopAuthoringRef } from '../src/shared/localApps';

const source = { projectPath: '/project/app', sessionId: 'session-app' };
const active = { projectPath: '/project/other', sessionId: 'session-other' };
const binding: DesktopAuthoringRef = {
  workspaceId: 'workspace-app', sourceDirectory: source.projectPath,
  projectRef: { kind: 'desktopPath', path: source.projectPath }, authoringSessionId: source.sessionId,
  appId: 'a12345678901234567890123456789012',
};
const app = { id: binding.appId!, name: 'Reading garden', authoring: binding, authoringAction: 'resumeSession' as const };

function navigation(overrides: Partial<LocalAppChatNavigation> = {}): LocalAppChatNavigation {
  return {
    hasProject: () => true,
    openSession: async () => source,
    addProject: async () => { throw new Error('Unexpected project picker'); },
    newSession: async () => { throw new Error('Unexpected new chat'); },
    bindAuthoring: async () => binding,
    prepareAuthoring: async () => binding,
    ...overrides,
  };
}

test('Modify resumes the exact saved Code session in the dedicated app source project', async () => {
  let created = false, bound = false;
  const target = await localAppChatTarget(app, navigation({
    openSession: async (projectPath, sessionId) => {
      assert.deepEqual({ projectPath, sessionId }, { projectPath: source.projectPath, sessionId: binding.authoringSessionId });
      return source;
    },
    newSession: async () => { created = true; return active; },
    bindAuthoring: async () => { bound = true; return binding; },
  }), () => true);
  assert.deepEqual(target, source);
  assert.equal(created, false);
  assert.equal(bound, false);
});

test('Modify never silently replaces a verified authoring session', async () => {
  let created = false, bound = false;
  await assert.rejects(localAppChatTarget(app, navigation({
    openSession: async () => false,
    newSession: async projectPath => { created = true; return { projectPath, sessionId: 'session-replacement' }; },
    bindAuthoring: async () => { bound = true; return binding; },
  }), () => true), /Could not resume the saved app authoring session/);
  assert.equal(created, false);
  assert.equal(bound, false);
});

test('Create without a saved workspace asks for one and makes a real Code session', async () => {
  const calls: string[] = [];
  const target = await localAppChatTarget(undefined, navigation({
    addProject: async purpose => { assert.equal(purpose, 'local-app'); calls.push('pick'); return { path: '/selected/app' }; },
    newSession: async path => { calls.push(`new:${path}`); return { projectPath: path, sessionId: 'new-session' }; },
    prepareAuthoring: async session => { calls.push(`prepare:${session.projectPath}:${session.sessionId}`); return { ...binding,
      sourceDirectory: session.projectPath, projectRef: { kind: 'desktopPath', path: session.projectPath }, authoringSessionId: session.sessionId, appId: undefined }; },
  }), () => true);
  assert.deepEqual(calls, ['pick', 'new:/selected/app', 'prepare:/selected/app:new-session']);
  assert.deepEqual(target, { projectPath: '/selected/app', sessionId: 'new-session' });
});

test('a legacy source-only binding is rebound in its saved source directory', async () => {
  const legacy = { id: app.id, name: app.name, authoring: { sourceDirectory: source.projectPath }, authoringAction: 'rebindRequired' as const };
  let bound = false;
  const target = await localAppChatTarget(legacy, navigation({
    newSession: async path => ({ projectPath: path, sessionId: 'session-rebound' }),
    bindAuthoring: async (_appId, session) => { bound = true; return { ...binding, authoringSessionId: session.sessionId }; },
  }), () => true);
  assert.deepEqual(target, { projectPath: source.projectPath, sessionId: 'session-rebound' });
  assert.equal(bound, true);
});

test('a removed saved workspace must be re-added exactly instead of using the active project', async () => {
  const calls: string[] = [];
  const target = await localAppChatTarget(app, navigation({
    hasProject: () => false,
    addProject: async purpose => { calls.push(`pick:${purpose}`); return { path: source.projectPath }; },
    newSession: async path => { calls.push(`new:${path}`); return { projectPath: path, sessionId: 'recovered-session' }; },
    bindAuthoring: async (_appId, session) => ({ ...binding, authoringSessionId: session.sessionId }),
  }), () => true);
  assert.deepEqual(calls, ['pick:local-app', `new:${source.projectPath}`]);
  assert.deepEqual(target, { projectPath: source.projectPath, sessionId: 'recovered-session' });
  await assert.rejects(localAppChatTarget(app, navigation({
    hasProject: () => false, addProject: async () => ({ path: '/unrelated/project' }),
  }), () => true), /Add the saved app workspace/);
});

test('a missing source can be explicitly located and rebound before creating an edit chat', async () => {
  const missing = { ...app, authoringSourceMissing: true };
  const calls: string[] = [];
  const target = await localAppChatTarget(missing, navigation({
    hasProject: path => path === '/restored/app-source',
    addProject: async purpose => { calls.push(`locate:${purpose}`); return { path: '/restored/app-source' }; },
    newSession: async projectPath => { calls.push(`new:${projectPath}`); return { projectPath, sessionId: 'restored-session' }; },
    bindAuthoring: async (appId, session) => { calls.push(`bind:${appId}:${session.sessionId}`); return { ...binding, sourceDirectory: session.projectPath,
      projectRef: { kind: 'desktopPath', path: session.projectPath }, authoringSessionId: session.sessionId }; },
  }), () => true);
  assert.deepEqual(target, { projectPath: '/restored/app-source', sessionId: 'restored-session' });
  assert.deepEqual(calls, [`locate:local-app`, 'new:/restored/app-source', `bind:${app.id}:restored-session`]);
});

test('canceling source recovery never creates a chat or draft', async () => {
  const missing = { ...app, authoringSourceMissing: true };
  let created = false;
  const target = await localAppChatTarget(missing, navigation({
    addProject: async purpose => { assert.equal(purpose, 'local-app'); return null; },
    newSession: async () => { created = true; return active; },
  }), () => true);
  assert.equal(target, undefined);
  assert.equal(created, false);
  assert.equal(await localAppChatTarget(app, navigation({ hasProject: () => false, addProject: async () => null }), () => true), undefined);
});

test('a superseded navigation cannot produce a draft or start a replacement chat', async () => {
  let current = true, created = false;
  const pending = localAppChatTarget(app, navigation({
    openSession: async () => { current = false; return source; },
    newSession: async () => { created = true; return active; },
  }), () => current);
  assert.equal(await pending, undefined);
  assert.equal(created, false);
});

test('drafts apply only to the exact settled Code session', () => {
  const draft: LocalAppDraft = { id: app.id, name: app.name, target: source, nonce: 1 };
  assert.equal(localAppDraftDisposition(draft, active, true), 'wait');
  assert.equal(localAppDraftDisposition(draft, source, false), 'apply');
  assert.equal(localAppDraftDisposition(draft, active, false), 'discard');
  assert.equal(localAppDraftDisposition(draft, { ...source, projectPath: '/other' }, false), 'discard');
  assert.equal(localAppDraftDisposition(draft, undefined, false), 'discard');
});

test('both authoring entries ask for read-only Agent recovery and an explicit new attempt', () => {
  const create = localAppAuthoringPrompt({ id: '', name: '' });
  const modify = localAppAuthoringPrompt({ id: app.id, name: app.name });
  assert.match(create, /创建一个本地应用/);
  assert.match(modify, new RegExp(`LocalApp id: ${app.id}`));
  for (const prompt of [create, modify]) {
    assert.match(prompt, /runId、threadId/);
    assert.match(prompt, /只读方式重连同一 run 和 thread/);
    assert.match(prompt, /不要自动提交、重发或重放工具/);
    assert.match(prompt, /仅在宿主确认时显示正在运行/);
    assert.match(prompt, /中断或未知状态/);
    assert.match(prompt, /用户明确发起并使用新的 runId/);
    assert.match(prompt, /不要承诺无限期运行/);
  }
});
