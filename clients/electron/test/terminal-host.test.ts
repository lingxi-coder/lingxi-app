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
