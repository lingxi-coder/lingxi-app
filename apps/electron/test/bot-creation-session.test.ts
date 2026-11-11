import assert from 'node:assert/strict';
import { mkdtempSync, realpathSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { CH_SESSION_NEW, HostController } from '../src/main/host';
import { SessionRuntimeManager } from '../src/main/sessionRuntimeManager';
import { DiagnosticBuffer } from '../src/main/host-utils';
import { SettingsStore } from '../src/main/settings';

for (const mode of ['chat', 'code'] as const) {
  test(`each Bot setup opens a separate ${mode} session and preserves the existing draft`, async t => {
    const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-bot-session-')));
    const settings = new SettingsStore(join(root, 'settings'));
    await settings.conversationSpaces.setMode(mode);
    const projectPath = settings.conversationSpaces.globalWorkspaces[mode];
    const manager = new SessionRuntimeManager({ launchConfig: ref => ({ workspace: ref.projectPath, trusted: true }) });
    // Exercise the real allocation/cache policy without booting a model process.
    (manager as any).ensure = async (ref: any) => ({ ...ref });
    (manager as any).activate = () => {};
    const original = await manager.newSession(projectPath, undefined, { mode, projectId: null });
    const handlers = new Map<string, (...args: any[]) => any>();
    const bridge = { registerWindow() {}, registerIpc() {}, newSession: manager.newSession.bind(manager), activateSession() {} };
    const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), {} as any,
      { handle: (name: string, handler: any) => handlers.set(name, handler), removeHandler() {} } as any);
    (host as any).bootstrap = () => ({ settings: settings.getPublic(), activeSession: settings.getPublic().activeSession });
    t.after(async () => { host.dispose(); await manager.dispose(); rmSync(root, { recursive: true, force: true }); });
    const frame = { url: 'http://127.0.0.1:4242' };
    const sender = { mainFrame: frame, isDestroyed: () => false, once() {}, send() {} };
    host.registerWindow(sender as any, frame.url); host.registerIpc();
    const create = () => handlers.get(CH_SESSION_NEW)!({ sender, senderFrame: frame }, null, undefined, { kind: 'create-bot' });
    const first = (await create()).activeSession;
    const second = (await create()).activeSession;
    assert.notEqual(first.sessionId, original.sessionId);
    assert.notEqual(second.sessionId, original.sessionId);
    assert.notEqual(second.sessionId, first.sessionId);
    assert.deepEqual({ mode: first.mode, projectId: first.projectId, projectPath: first.projectPath }, { mode, projectId: null, projectPath });
    assert.equal((await manager.newSession(projectPath, undefined, { mode, projectId: null })).sessionId, original.sessionId);
    assert.equal((await manager.reopenOwnedDraft(first)).sessionId, first.sessionId);
    await assert.rejects(handlers.get(CH_SESSION_NEW)!({ sender, senderFrame: frame }, null, undefined, { kind: 'create-bot', extra: true }), /invalid new session purpose/);
  });
}
