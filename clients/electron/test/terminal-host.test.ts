import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, realpathSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { HostController } from '../src/main/host';
import { DiagnosticBuffer } from '../src/main/host-utils';
import { SettingsStore } from '../src/main/settings';
import { CH_TERMINAL_REQUEST, type TerminalScope, type TerminalSnapshot } from '../src/shared/terminal';

test('terminal IPC validates sender, scope and window attachment without a connected model', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'lingxi-terminal-host-'));
  const project = realpathSync(directory);
  const settings = new SettingsStore(join(directory, 'settings'));
  settings.addProject(project);
  const sessionId = '11111111-2222-4333-8444-555555555555';
  const scope = { projectPath: project, sessionId };
  settings.setActiveSessionDraft(scope);
  const handlers = new Map<string, (...args: any[]) => any>();
  const ipc = { handle: (name: string, fn: (...args: any[]) => any) => handlers.set(name, fn), removeHandler: (name: string) => handlers.delete(name) };
  const bridge = { registerWindow() {}, registerIpc() {}, get: (id: string) => id === sessionId ? { projectPath: project, connectionState: { status: 'idle' } } : { projectPath: '/another' } };
  const rows = new Map<string, TerminalSnapshot>();
  const input: string[] = [];
  const manager = {
    setOutputPaused() {}, onEvent: () => () => {}, get: (id: string) => rows.get(id),
    list: (owner: TerminalScope) => [...rows.values()].filter(row => row.scope.sessionId === owner.sessionId),
    create: async (owner: TerminalScope) => { const row: TerminalSnapshot = { id: 'terminal-a', scope: owner, title: 'test', status: 'running', output: '', sequence: 0, exitCode: null }; rows.set(row.id, row); return row; },
    input: async (_id: string, data: string) => { input.push(data); }, resize: async () => {}, close: async (id: string) => { rows.delete(id); },
  };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), undefined, ipc as any);
  host.attachTerminals(manager as any);
  const makeSender = () => { const frame = { url: 'http://127.0.0.1:4242' }; const sender = { mainFrame: frame, isDestroyed: () => false, once() {}, send() {} }; host.registerWindow(sender as any, frame.url); return { sender, senderFrame: frame }; };
  const event = makeSender(); const second = makeSender();
  host.registerIpc();
  const invoke = (request: object, from = event) => handlers.get(CH_TERMINAL_REQUEST)!(from, request);
  try {
    await assert.rejects(invoke({ kind: 'create', scope }, { ...event, senderFrame: { url: event.senderFrame.url } }), /unauthorized/);
    await assert.rejects(invoke({ kind: 'create', scope: { ...scope, sessionId: '22222222-3333-4444-8555-666666666666' } }), /different project/);
    await assert.rejects(invoke({ kind: 'create', scope: { ...scope, sessionId: '__draft__' } }), /active session/);
    const terminal = await invoke({ kind: 'create', scope });
    assert.equal(terminal.scope.sessionId, sessionId);
    await assert.rejects(invoke({ kind: 'input', terminalId: terminal.id, data: 'pwd\r' }, second), /not attached/);
    await invoke({ kind: 'input', terminalId: terminal.id, data: 'pwd\r' });
    assert.deepEqual(input, ['pwd\r']);
    await assert.rejects(invoke({ kind: 'resize', terminalId: terminal.id, rows: 0, cols: 80 }), /dimensions/);
    await assert.rejects(invoke({ kind: 'input', terminalId: terminal.id, data: 'x'.repeat(65_537) }), /input/);
    await invoke({ kind: 'list', scope }, second);
    await invoke({ kind: 'input', terminalId: terminal.id, data: 'echo hello\r' }, second);
    settings.removeProject(project);
    await assert.rejects(invoke({ kind: 'input', terminalId: terminal.id, data: 'pwd\r' }), /unavailable/);
  } finally { host.dispose(); rmSync(directory, { recursive: true, force: true }); }
});

/**
 * A terminal outlives the turn that opened it, so nothing in the terminal
 * manager notices that its chat or project went away — the host has to say so.
 * Every one of these hooks existed on `TerminalManager` and had no caller:
 * archiving or clearing a chat, or removing a project, left its shells running
 * with no surface that could ever reach them again, and a shell opened in the
 * draft scope was orphaned the moment the draft became a real session.
 */
test('the host tells the terminal manager when a chat or project goes away', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'lingxi-terminal-lifecycle-'));
  const project = realpathSync(directory);
  const settings = new SettingsStore(join(directory, 'settings'));
  settings.addProject(project);
  const sessionId = '11111111-2222-4333-8444-555555555555';
  const ref = { projectPath: project, sessionId };
  const calls: string[] = [];
  const manager = {
    setOutputPaused() {}, onEvent: () => () => {},
    closeScope: async (scope: TerminalScope) => { calls.push(`closeScope:${scope.sessionId}`); },
    closeProject: async (path: string) => { calls.push(`closeProject:${path === project}`); },
    migrateScope: (from: TerminalScope, to: TerminalScope) => { calls.push(`migrate:${from.sessionId}->${to.sessionId}`); },
  };
  const replacement = { projectPath: project, sessionId: '22222222-3333-4444-8555-666666666666' };
  const bridge = {
    registerWindow() {}, registerIpc() {}, hasActiveWork: () => false,
    get: () => ({ projectPath: project, connectionState: { status: 'idle' }, pendingInteractions: 0, pendingAskUserQuestions: [] }),
    closeSession: async () => { calls.push('closeSession'); },
    closeProject: async () => { calls.push('closeProject:bridge'); },
    newSession: async () => replacement,
    withBackgroundSession: async (_r: unknown, _e: unknown, _m: unknown, run: (runtime: unknown) => Promise<unknown>) => run({
      projectPath: project, connectionState: { status: 'connected' },
      beginArchive: () => () => undefined,
      manageCron: async () => [],
    }),
  };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer());
  host.attachTerminals(manager as any);
  (host as any).bootstrap = () => ({ settings: settings.getPublic() });
  (host as any).assertSessionBelongsToProject = async () => ({ title: 'chat' });
  (host as any).loadProjectSessions = async () => ({ sessions: [] });
  try {
    // A draft chat's shells follow it into the session it becomes: the scope id
    // is what addresses a shell, and `__draft__` stops addressing anything the
    // moment the engine hands back a real session id.
    (host as any).sessionCatalog = { list: async () => ({ sessions: [] }) };
    const restored = await host.restoreProjectSession(project);
    assert.equal(restored.sessionId, replacement.sessionId);
    assert.deepEqual(calls, [`migrate:__draft__->${replacement.sessionId}`]);
    calls.length = 0;

    settings.setActiveSession(ref);
    await (host as any).clearActiveSession(sessionId);
    assert.ok(calls.includes(`closeScope:${sessionId}`), 'clearing a chat closes its shells');
    assert.ok(calls.indexOf(`closeScope:${sessionId}`) < calls.indexOf('closeSession'),
      'the shells must be closed while the session is still addressable');

    calls.length = 0;
    settings.setActiveSession(ref);
    await (host as any).archiveSessionInternal(ref);
    assert.ok(calls.includes(`closeScope:${sessionId}`), 'archiving a chat closes its shells');

    calls.length = 0;
    await (host as any).removeProjectInternal(project);
    assert.ok(calls.includes('closeProject:true'), 'removing a project closes every shell under it');
    assert.ok(calls.indexOf('closeProject:true') < calls.indexOf('closeProject:bridge'),
      'the shells must be closed before the runtimes they live beside');
  } finally { host.dispose(); rmSync(directory, { recursive: true, force: true }); }
});

test('a shell opened in the draft scope follows the chat the New-chat action creates', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'lingxi-terminal-draft-'));
  const project = realpathSync(directory);
  const settings = new SettingsStore(join(directory, 'settings'));
  settings.addProject(project);
  const created = { projectPath: project, sessionId: '33333333-4444-4555-8666-777777777777' };
  const migrations: string[] = [];
  const handlers = new Map<string, (...args: any[]) => any>();
  const ipc = { handle: (name: string, fn: (...args: any[]) => any) => handlers.set(name, fn), removeHandler: (name: string) => handlers.delete(name) };
  const bridge = { registerWindow() {}, registerIpc() {}, newSession: async () => created, get: () => undefined };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), undefined, ipc as any);
  host.attachTerminals({
    setOutputPaused() {}, onEvent: () => () => {},
    migrateScope: (from: TerminalScope, to: TerminalScope) => { migrations.push(`${from.sessionId}->${to.sessionId}`); },
  } as any);
  (host as any).bootstrap = () => ({ settings: settings.getPublic() });
  const frame = { url: 'http://127.0.0.1:4242' };
  const sender = { mainFrame: frame, isDestroyed: () => false, once() {}, send() {} };
  host.registerWindow(sender as any, frame.url);
  host.registerIpc();
  try {
    await handlers.get('lingxi:session:new')!({ sender, senderFrame: frame }, project, undefined);
    assert.deepEqual(migrations, [`__draft__->${created.sessionId}`]);
    assert.equal(settings.getPublic().activeSession?.sessionId, created.sessionId);
  } finally { host.dispose(); rmSync(directory, { recursive: true, force: true }); }
});
