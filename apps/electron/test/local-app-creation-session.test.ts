import assert from 'node:assert/strict';
import { mkdtempSync, realpathSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { CH_SESSION_NEW, HostController } from '../src/main/host';
import { SessionRuntimeManager } from '../src/main/sessionRuntimeManager';
import { DiagnosticBuffer } from '../src/main/host-utils';
import { SettingsStore } from '../src/main/settings';
import { localAppChatTarget } from '../src/renderer/bridge/localAppChat';
import type { SessionRef } from '../src/shared/settings';

test('repeated Local App creation in one project allocates separate Code sessions and preserves the old draft', async t => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-local-app-session-')));
  const settings = new SettingsStore(join(root, 'settings'));
  await settings.conversationSpaces.setMode('code');
  settings.addProject(root);
  await settings.conversationSpaces.addProject(root, 'code');
  const manager = new SessionRuntimeManager({ launchConfig: ref => ({ workspace: ref.projectPath, trusted: true }) });
  // Keep real session allocation/cache behavior; model boot and source verification have their own tests.
  (manager as any).ensure = async (ref: SessionRef) => ({ ...ref });
  (manager as any).activate = () => {};
  const original = await manager.newSession(root, undefined, { mode: 'code', projectId: root });
  const allocated: string[] = [], completed: string[] = [];
  const handlers = new Map<string, (...args: any[]) => any>();
  const bridge = {
    registerWindow() {}, registerIpc() {}, activateSession() {},
    newSession: manager.newSession.bind(manager),
    get: () => ({}), retainBackgroundSession: () => () => {},
  };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), {} as any,
    { handle: (name: string, handler: any) => handlers.set(name, handler), removeHandler() {} } as any);
  host.attachLocalAppSessions({
    begin: () => ({
      allocate: async ref => { allocated.push(ref.sessionId); },
      recordSource: async () => {},
      finish: async ref => { completed.push(ref.sessionId); },
    }),
    get: async () => { throw new Error('Creation must not recover an existing operation'); },
    recordSource: async () => { throw new Error('Unexpected recovery'); },
    finish: async () => { throw new Error('Unexpected recovery'); },
  });
  (host as any).rememberCreatedSessionSource = async (ref: SessionRef) => {
    const transcriptPath = join(root, `${ref.sessionId}.jsonl`);
    settings.rememberSessionTranscriptSelection(ref, { catalogProjectPath: root, scope: 'current', transcriptPath });
    return transcriptPath;
  };
  (host as any).assertLocalAppSessionRuntime = async () => {};
  (host as any).bootstrap = () => ({ settings: settings.getPublic() });
  t.after(async () => { host.dispose(); await manager.dispose(); rmSync(root, { recursive: true, force: true }); });
  const frame = { url: 'http://127.0.0.1:4242' };
  const sender = { mainFrame: frame, isDestroyed: () => false, once() {}, send() {} };
  host.registerWindow(sender as any, frame.url); host.registerIpc();
  const create = () => localAppChatTarget(undefined, {
    mode: 'code', requiresCodeMessage: 'Code required', hasProject: () => true,
    addProject: async () => ({ path: root }),
    openSession: async () => { throw new Error('Creation must not resume an existing session'); },
    recoverSession: async () => { throw new Error('Unexpected recovery'); },
    newSession: async (path, intent) => (await handlers.get(CH_SESSION_NEW)!({ sender, senderFrame: frame }, path, undefined,
      { kind: 'local-app', intent })).settings.activeSession,
  }, () => true);
  const first = await create(), second = await create();
  assert.ok(first && second);
  assert.equal(new Set([original.sessionId, first.sessionId, second.sessionId]).size, 3);
  assert.deepEqual(allocated, [first.sessionId, second.sessionId]);
  assert.deepEqual(completed, allocated);
  assert.equal(first.projectPath, root);
  assert.equal(second.mode, 'code');
  assert.equal((await manager.newSession(root, undefined, { mode: 'code', projectId: root })).sessionId, original.sessionId);
  assert.equal((await manager.reopenOwnedDraft(first)).sessionId, first.sessionId);
});
