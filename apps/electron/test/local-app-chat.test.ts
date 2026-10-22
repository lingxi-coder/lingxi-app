import assert from 'node:assert/strict';
import { test } from 'node:test';
import { localAppChatTarget, localAppDraftDisposition, type LocalAppChatNavigation, type LocalAppDraft } from '../src/renderer/bridge/localAppChat';

const source = { projectPath: '/project/a', sessionId: 'chat-a' };
const active = { projectPath: '/project/b', sessionId: 'chat-b' };
const app = { id: 'a12345678901234567890', name: 'Reading garden', sourceSession: source };
function navigation(overrides: Partial<LocalAppChatNavigation> = {}): LocalAppChatNavigation {
  return {
    activeProject: active.projectPath,
    hasProject: () => true,
    openSession: async () => true,
    addProject: async () => { throw new Error('Unexpected project picker'); },
    newSession: async () => { throw new Error('Unexpected new chat'); },
    ...overrides,
  };
}

test('Modify waits for the renderer navigation to apply the source chat before drafting', async () => {
  let visible = active;
  let resolveNavigation!: (opened: boolean) => void;
  let finished = false;
  const pending = localAppChatTarget(app, navigation({
    openSession: async (projectPath, sessionId) => {
      assert.deepEqual({ projectPath, sessionId }, source);
      const opened = await new Promise<boolean>(resolve => { resolveNavigation = resolve; });
      if (opened) visible = source;
      return opened;
    },
  }), () => true).then(target => { finished = true; return target; });
  await Promise.resolve();
  assert.equal(finished, false);
  assert.deepEqual(visible, active);
  resolveNavigation(true);
  assert.deepEqual(await pending, visible);
  assert.deepEqual(visible, source);
});

test('Create without a project asks for one and establishes a new real session', async () => {
  const calls: string[] = [];
  const target = await localAppChatTarget(undefined, navigation({
    activeProject: undefined,
    addProject: async () => { calls.push('pick'); return { path: '/selected' }; },
    newSession: async path => { calls.push(`new:${path}`); return { projectPath: path, sessionId: 'new-chat' }; },
  }), () => true);
  assert.deepEqual(calls, ['pick', 'new:/selected']);
  assert.deepEqual(target, { projectPath: '/selected', sessionId: 'new-chat' });
});

test('canceling the project picker does not create a chat or a draft', async () => {
  assert.equal(await localAppChatTarget(undefined, navigation({ activeProject: undefined, addProject: async () => null }), () => true), undefined);
});

test('an app whose source project was removed gets a fresh chat without mixing another draft', async () => {
  assert.deepEqual(await localAppChatTarget(app, navigation({
    hasProject: () => false,
    openSession: async () => { throw new Error('Removed source must not open'); },
    newSession: async path => ({ projectPath: path, sessionId: 'fresh-chat' }),
  }), () => true), { projectPath: active.projectPath, sessionId: 'fresh-chat' });
});

test('a removed authoring chat gets a fresh session in its project, preserving the previous chat', async () => {
  assert.deepEqual(await localAppChatTarget(app, navigation({
    openSession: async () => false,
    newSession: async path => ({ projectPath: path, sessionId: 'replacement-chat' }),
  }), () => true), { projectPath: source.projectPath, sessionId: 'replacement-chat' });
});

test('a superseded navigation result cannot produce a draft or start another chat', async () => {
  let current = true;
  assert.equal(await localAppChatTarget(app, navigation({ openSession: async () => { current = false; return true; } }), () => current), undefined);
  current = true;
  assert.equal(await localAppChatTarget(undefined, navigation({
    activeProject: undefined,
    addProject: async () => { current = false; return { path: '/selected' }; },
  }), () => current), undefined);
});

test('drafts wait during navigation and apply only to their exact settled session', () => {
  const draft: LocalAppDraft = { ...app, target: source, nonce: 1 };
  assert.equal(localAppDraftDisposition(draft, active, true), 'wait');
  assert.equal(localAppDraftDisposition(draft, source, false), 'apply');
  assert.equal(localAppDraftDisposition(draft, active, false), 'discard');
  assert.equal(localAppDraftDisposition(draft, { ...source, projectPath: '/other' }, false), 'discard');
  assert.equal(localAppDraftDisposition(draft, undefined, false), 'discard');
});
