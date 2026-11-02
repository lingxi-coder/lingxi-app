import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { appendFileSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { CH_SESSION_METADATA_CAPTURE, CH_SESSION_METADATA_RELEASE, CH_SESSION_RENAME, HostController } from '../src/main/host';
import { DiagnosticBuffer } from '../src/main/host-utils';
import { SettingsStore } from '../src/main/settings';

test('rename retains the captured active writer across the dialog and rejects a replaced source', async () => {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), 'lingxi-rename-')));
  const settings = new SettingsStore(join(directory, 'settings'));
  settings.addProject(directory);
  const sessionId = '11111111-2222-4333-8444-555555555555';
  const path = join(directory, `${sessionId}.jsonl`);
  const original = `${JSON.stringify({ type: 'user', message: { content: 'Hello' } })}\n`;
  writeFileSync(path, original);
  let catalogFailure = false;
  let leases = 0;
  let captures = 0;
  const handlers = new Map<string, (...args: any[]) => any>();
  const runtime = {
    projectPath: directory, sessionMode: 'code', connectionState: { status: 'connected' },
    getWorkspaceState: async () => ({ cwd: directory, additional_directories: [], transcript_path: path }),
    captureSessionMetadataOwner: async (selectedPath: string, mode: string) => {
      assert.equal(selectedPath, path); assert.equal(mode, 'code'); captures++;
      const identity = lstatSync(path, { bigint: true });
      return async (action: string, title?: string) => {
        const current = lstatSync(path, { bigint: true });
        if (current.dev !== identity.dev || current.ino !== identity.ino) throw new Error('captured source changed');
        if (action === 'rename') appendFileSync(path, `${JSON.stringify({ type: 'custom-title', customTitle: title, sessionId })}\n`);
      };
    },
  };
  const bridge = {
    registerWindow() {}, registerIpc() {}, get: () => runtime, isProjectClosing: () => false,
    retainBackgroundSession: () => { leases++; return () => { leases--; }; },
    withBackgroundSession: () => { throw new Error('rename must not start another engine'); },
  };
  const row = () => {
    const records = readFileSync(path, 'utf8').trim().split('\n').map(line => JSON.parse(line));
    return { uuid: sessionId, path, title: records.at(-1)?.customTitle ?? 'Hello', modified_rfc3339: new Date().toISOString(), message_count: 1, mode: 'code', empty_session: false };
  };
  const catalog = {
    list: async () => { if (catalogFailure) throw new Error('catalog temporarily unavailable'); return { sessions: [row()] }; },
    invalidate: () => undefined,
  };
  const ipc = { handle: (name: string, handler: (...args: any[]) => any) => handlers.set(name, handler), removeHandler: (name: string) => handlers.delete(name) };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), catalog as any, ipc as any);
  const frame = { url: 'http://127.0.0.1:4242' };
  const sender = { mainFrame: frame, isDestroyed: () => false, once() {}, send() {} };
  host.registerWindow(sender as any, frame.url);
  host.registerIpc();
  const event = { sender, senderFrame: frame };
  const capture = () => handlers.get(CH_SESSION_METADATA_CAPTURE)!(event, directory, sessionId, { scope: 'current', transcriptPath: path });
  const rename = (title: string, selection?: unknown) => handlers.get(CH_SESSION_RENAME)!(event, directory, sessionId, title, selection);
  try {
    await assert.rejects(rename('without capture'), /Capture the session source/);
    let selected = await capture();
    assert.equal(leases, 1);
    const otherFrame = { url: frame.url };
    const otherSender = { mainFrame: otherFrame, isDestroyed: () => false, once() {}, send() {} };
    host.registerWindow(otherSender as any, otherFrame.url);
    await assert.rejects(handlers.get(CH_SESSION_RENAME)!({ sender: otherSender, senderFrame: otherFrame }, directory, sessionId, 'stolen', selected), /expired or changed/);
    await assert.rejects(rename('changed scope', { ...selected, scope: 'all' }), /expired or changed/);
    assert.equal(readFileSync(path, 'utf8'), original);
    const result = await rename('  Desktop UI优化  ', selected);
    assert.equal(result.sessions[0].title, 'Desktop UI优化');
    assert.equal(leases, 0);
    await assert.rejects(rename('replay', selected), /expired or changed/);
    catalogFailure = true;
    selected = await capture();
    const fallback = await rename('Offline title', selected);
    assert.equal(fallback.sessions[0].title, 'Offline title');
    assert.match(fallback.error, /temporarily unavailable/);
    for (const sibling of [
      { ...row(), path: join(directory, 'sibling.jsonl'), title: 'Other source' },
      { ...row(), mode: 'chat', title: 'Other mode' },
    ]) {
      // The displayed catalog can advance while the dialog retains source A.
      // A successful write to A must not relabel another cached row on error.
      (host as any).catalogs.set(directory, { sessions: [sibling] });
      selected = await capture();
      const result = await rename('Captured source only', selected);
      assert.equal(result.sessions[0].title, sibling.title);
      assert.equal(result.sessions[0].path, sibling.path);
      assert.match(readFileSync(path, 'utf8'), /Captured source only/);
    }
    catalogFailure = false;
    selected = await capture();
    const saved = readFileSync(path, 'utf8');
    await assert.rejects(rename('bad\nname', selected), /printable/);
    assert.equal(readFileSync(path, 'utf8'), saved);
    const old = `${path}.old`;
    renameSync(path, old);
    writeFileSync(path, saved);
    await assert.rejects(rename('wrong inode', selected), /captured source changed/);
    assert.equal(readFileSync(path, 'utf8'), saved);
    assert.equal(readFileSync(old, 'utf8'), saved);
    await handlers.get(CH_SESSION_METADATA_RELEASE)!(event, selected);
    assert.equal(leases, 0);
    selected = await capture();
    assert.ok(captures >= 4);
    settings.conversationSpaces.register({ projectPath: directory, projectId: directory, sessionId, mode: 'code' });
    const moved = join(directory, 'moved'); mkdirSync(moved);
    settings.conversationSpaces.relocate({ projectPath: directory, sessionId }, moved);
    await assert.rejects(rename('wrong owner', selected), /owner changed/);
    assert.equal(readFileSync(path, 'utf8'), saved);
    host.dispose();
    assert.equal(leases, 0, 'dispose releases captured runtime leases');
    assert.ok(!handlers.has(CH_SESSION_METADATA_CAPTURE));
    assert.ok(!handlers.has(CH_SESSION_METADATA_RELEASE));
    assert.ok(!handlers.has(CH_SESSION_RENAME));
  } finally {
    host.dispose();
    rmSync(directory, { recursive: true, force: true });
  }
});

test('renderer navigation during source capture releases the lease and returns no token', async () => {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), 'lingxi-metadata-navigation-')));
  const settings = new SettingsStore(join(directory, 'settings')); settings.addProject(directory);
  const sessionId = '11111111-2222-4333-8444-555555555555';
  const path = join(directory, `${sessionId}.jsonl`); writeFileSync(path, '{}\n');
  let finish!: () => void;
  const pending = new Promise<void>(resolve => { finish = resolve; });
  let entered!: () => void;
  const started = new Promise<void>(resolve => { entered = resolve; });
  let leases = 0;
  const runtime = { projectPath: directory, sessionMode: 'code', connectionState: { status: 'connected' },
    getWorkspaceState: async () => ({ transcript_path: path }),
    captureSessionMetadataOwner: async () => { entered(); await pending; return async () => {}; } };
  const bridge = { registerWindow() {}, registerIpc() {}, get: () => runtime, isProjectClosing: () => false,
    retainBackgroundSession: () => { leases++; return () => { leases--; }; } };
  const handlers = new Map<string, (...args: any[]) => any>();
  const ipc = { handle: (name: string, handler: (...args: any[]) => any) => handlers.set(name, handler), removeHandler: (name: string) => handlers.delete(name) };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), undefined, ipc as any);
  const frame = { url: 'http://127.0.0.1:4242' };
  const sender = Object.assign(new EventEmitter(), { mainFrame: frame, isDestroyed: () => false, send() {} });
  host.registerWindow(sender as any, frame.url); host.registerIpc();
  try {
    const capture = handlers.get(CH_SESSION_METADATA_CAPTURE)!({ sender, senderFrame: frame }, directory, sessionId, { scope: 'current', transcriptPath: path });
    await started;
    assert.equal(leases, 1);
    sender.emit('did-start-navigation', {}, frame.url, false, true);
    finish();
    await assert.rejects(capture, /owner changed during capture/);
    assert.equal(leases, 0);
    assert.equal((host as any).metadataSources.size, 0);
  } finally { host.dispose(); rmSync(directory, { recursive: true, force: true }); }
});

test('dispose during an internal source capture releases the lease and publishes no token', async () => {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), 'lingxi-metadata-dispose-')));
  const settings = new SettingsStore(join(directory, 'settings')); settings.addProject(directory);
  const sessionId = '11111111-2222-4333-8444-555555555555';
  const path = join(directory, `${sessionId}.jsonl`); writeFileSync(path, '{}\n');
  let finish!: () => void, entered!: () => void;
  const pending = new Promise<void>(resolve => { finish = resolve; });
  const started = new Promise<void>(resolve => { entered = resolve; });
  let leases = 0;
  const runtime = { projectPath: directory, sessionMode: 'code', connectionState: { status: 'connected' },
    getWorkspaceState: async () => ({ transcript_path: path }),
    captureSessionMetadataOwner: async () => { entered(); await pending; return async () => {}; } };
  const bridge = { get: () => runtime, isProjectClosing: () => false,
    retainBackgroundSession: () => { leases++; return () => { leases--; }; } };
  const ipc = { removeHandler() {} };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), undefined, ipc as any);
  try {
    const capture = (host as any).prepareMetadataSource({ projectPath: directory, sessionId });
    await started;
    assert.equal(leases, 1);
    host.dispose();
    finish();
    await assert.rejects(capture, /owner changed during capture/);
    assert.equal(leases, 0);
    assert.equal((host as any).metadataSources.size, 0);
  } finally { host.dispose(); rmSync(directory, { recursive: true, force: true }); }
});
