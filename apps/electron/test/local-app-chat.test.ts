import assert from 'node:assert/strict';
import { test } from 'node:test';
import { LocalAppAuthoringError, localAppAuthoringPrompt, localAppChatTarget, localAppDraftDisposition, type LocalAppChatNavigation, type LocalAppDraft } from '../src/renderer/bridge/localAppChat';
import type { DesktopAuthoringRef } from '../src/shared/localApps';
import { localAppAuthoringErrorPresentation, localAppErrorMessage, localAppToolRequiresCode } from '../src/renderer/bridge/localAppErrors';

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
    mode: 'code', requiresCodeMessage: 'Switch to Code',
    hasProject: () => true,
    openSession: async () => source,
    addProject: async () => { throw new Error('Unexpected project picker'); },
    newSession: async () => { throw new Error('Unexpected new chat'); },
    recoverSession: async () => { throw new Error('Unexpected recovery'); },
    ...overrides,
  };
}

test('Chat rejects Local App creation without switching mode or touching the project', async () => {
  const calls: string[] = [];
  await assert.rejects(localAppChatTarget(undefined, navigation({
    mode: 'chat',
    addProject: async () => { calls.push('project'); return { path: source.projectPath }; },
    newSession: async () => { calls.push('session'); return source; },
  }), () => true), /Switch to Code/);
  assert.deepEqual(calls, []);
});

test('the stable SDK development refusal is localized only for a failed LocalApp tool', () => {
  const reason = 'local_app_development_requires_code: Code required';
  assert.match(localAppErrorMessage(new Error(reason)), /Switch to Code/);
  assert.equal(localAppErrorMessage(new Error('another failure')), 'another failure');
  assert.equal(localAppToolRequiresCode('LocalApp', 'error', reason), true);
  assert.equal(localAppToolRequiresCode('LocalApp', 'error', `Error: ${reason}`), true);
  assert.equal(localAppToolRequiresCode('Read', 'error', reason), false);
  assert.equal(localAppToolRequiresCode('LocalApp', 'done', reason), false);
  for (const other of ['PathBlocked /tmp/local_app_development_requires_code/file',
    'The source says "local_app_development_requires_code"', 'local_app_development_requires_code_extra']) {
    assert.equal(localAppErrorMessage(other), other);
    assert.equal(localAppToolRequiresCode('LocalApp', 'error', other), false);
  }
});

test('Modify resumes the exact saved Code session in the dedicated app source project', async () => {
  let created = false, bound = false;
  const target = await localAppChatTarget(app, navigation({
    openSession: async (projectPath, sessionId) => {
      assert.deepEqual({ projectPath, sessionId }, { projectPath: source.projectPath, sessionId: binding.authoringSessionId });
      return source;
    },
    newSession: async () => { created = true; return active; },
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
  }), () => true), /Could not resume the saved app authoring session/);
  assert.equal(created, false);
  assert.equal(bound, false);
});

test('Create without a saved workspace asks for one and makes a real Code session', async () => {
  const calls: string[] = [];
  const target = await localAppChatTarget(undefined, navigation({
    addProject: async purpose => { assert.equal(purpose, 'local-app'); calls.push('pick'); return { path: '/selected/app' }; },
    newSession: async (path, intent) => { calls.push(`new:${path}`); assert.deepEqual(intent, { name: 'app', setCurrent: false }); return { projectPath: path, sessionId: 'new-session' }; },
  }), () => true);
  assert.deepEqual(calls, ['pick', 'new:/selected/app']);
  assert.deepEqual(target, { projectPath: '/selected/app', sessionId: 'new-session' });
});

test('authoring startup errors retain the selected project and retry guidance without exposing host errors', async () => {
  const projectPath = '/fixture/workspace/directory/App';
  for (const [message, diagnostic] of [
    ['ENOSPC: secret-host-token', 'Disk full; task paused'],
    ['unknown failure: secret-host-token', ''],
    ['Codex authentication unavailable. Sign in again. secret-host-token', 'Codex sign-in required'],
    ['cancel the active turn before secret-host-token', 'Turn in progress'],
    ['unexpected result: invalid type: newtype struct secret-host-token', 'Couldn’t read the response'],
  ]) {
    const cause = new Error(message);
    await assert.rejects(localAppChatTarget(undefined, navigation({
      addProject: async () => ({ path: projectPath }),
      newSession: async () => { throw cause; },
    }), () => true), (error: unknown) => {
      assert.ok(error instanceof LocalAppAuthoringError);
      assert.equal(error.cause, cause);
      assert.equal(error.projectPath, projectPath);
      const presentation = localAppAuthoringErrorPresentation(error);
      assert.match(presentation.detail, /authoring chat/);
      assert.ok(presentation.detail.includes(projectPath));
      assert.match(presentation.detail, /Choose that source folder to retry/);
      if (diagnostic) assert.ok(presentation.detail.includes(diagnostic));
      assert.doesNotMatch(JSON.stringify(presentation), /secret-host-token/);
      assert.equal(presentation.technicalDetail, undefined);
      return true;
    });
  }
});

test('an unconfirmed source-only binding is registered without changing the current owner', async () => {
  const unconfirmed = { id: app.id, name: app.name, authoring: { sourceDirectory: source.projectPath }, authoringAction: 'rebindRequired' as const };
  let bound = false;
  const target = await localAppChatTarget(unconfirmed, navigation({
    newSession: async (path, intent) => { assert.equal(intent.setCurrent, false); assert.equal(intent.appId, app.id); bound = true; return { projectPath: path, sessionId: 'session-rebound' }; },
  }), () => true);
  assert.deepEqual(target, { projectPath: source.projectPath, sessionId: 'session-rebound' });
  assert.equal(bound, true);
});

test('an explicitly restored source becomes current after its new session is registered', async () => {
  const restored = { id: app.id, name: app.name, authoring: { sourceDirectory: source.projectPath },
    authoringAction: 'rebindRequired' as const, authoringSetCurrent: true, authoringRequestId: 'copy-operation' };
  let registered = false;
  const target = await localAppChatTarget(restored, navigation({
    newSession: async (projectPath, intent) => {
      assert.equal(intent.setCurrent, true); assert.equal(intent.sourceRequestId, 'copy-operation'); registered = true;
      return { projectPath, sessionId: 'restored-session' };
    },
  }), () => true);
  assert.equal(target?.sessionId, 'restored-session');
  assert.equal(registered, true);
});

test('a removed saved workspace must be re-added exactly instead of using the active project', async () => {
  const calls: string[] = [];
  const target = await localAppChatTarget(app, navigation({
    hasProject: () => false,
    addProject: async purpose => { calls.push(`pick:${purpose}`); return { path: source.projectPath }; },
    newSession: async path => { calls.push(`new:${path}`); return { projectPath: path, sessionId: 'recovered-session' }; },
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
    newSession: async (projectPath, intent) => { assert.equal(intent.setCurrent, true); assert.equal(intent.appId, app.id); calls.push(`new:${projectPath}`); return { projectPath, sessionId: 'restored-session' }; },
  }), () => true);
  assert.deepEqual(target, { projectPath: '/restored/app-source', sessionId: 'restored-session' });
  assert.deepEqual(calls, [`locate:local-app`, 'new:/restored/app-source']);
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
