import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { CH_SESSION_RENAME, HostController } from '../src/main/host';
import { DiagnosticBuffer } from '../src/main/host-utils';
import { SettingsStore } from '../src/main/settings';

test('rename persists and returns a title without starting or dispatching to an engine', async () => {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), 'lingxi-rename-')));
  const settings = new SettingsStore(join(directory, 'settings'));
  settings.addProject(directory);
  const sessionId = '11111111-2222-4333-8444-555555555555';
  const path = join(directory, `${sessionId}.jsonl`);
  const original = `${JSON.stringify({ type: 'user', message: { content: 'Hello' } })}\n`;
  writeFileSync(path, original);
  let catalogFailure = false;
  const handlers = new Map<string, (...args: any[]) => any>();
  const bridge = {
    registerWindow() {}, registerIpc() {},
    get: () => ({ projectPath: directory, connectionState: { status: 'connected' } }),
    withBackgroundSession: () => { throw new Error('rename must not wait on engine readiness'); },
  };
  const row = () => {
    const records = readFileSync(path, 'utf8').trim().split('\n').map(line => JSON.parse(line));
    return { uuid: sessionId, path, title: records.at(-1)?.customTitle ?? 'Hello', modified_rfc3339: new Date().toISOString(), message_count: 1, mode: 'code', empty_session: false };
  };
  const catalog = {
    find: async (_project: string, id: string) => id === sessionId ? row() : undefined,
    list: async () => {
      if (catalogFailure) throw new Error('catalog temporarily unavailable');
      return { sessions: [row()] };
    },
    invalidate: () => undefined,
  };
  const ipc = { handle: (name: string, handler: (...args: any[]) => any) => handlers.set(name, handler), removeHandler: (name: string) => handlers.delete(name) };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), catalog as any, ipc as any);
  const frame = { url: 'http://127.0.0.1:4242' };
  const sender = { mainFrame: frame, isDestroyed: () => false, once() {}, send() {} };
  host.registerWindow(sender as any, frame.url);
  host.registerIpc();
  const invoke = (title: string) => handlers.get(CH_SESSION_RENAME)!({ sender, senderFrame: frame }, directory, sessionId, title);
  try {
    const result = await invoke('  Desktop UI优化  ');
    assert.equal(result.sessions[0].title, 'Desktop UI优化');
    assert.equal(readFileSync(path, 'utf8'), original + `${JSON.stringify({ type: 'custom-title', customTitle: 'Desktop UI优化', sessionId })}\n`);
    await invoke('Second title');
    assert.equal(row().title, 'Second title');
    catalogFailure = true;
    const fallback = await invoke('Offline title');
    assert.equal(fallback.sessions[0].title, 'Offline title');
    assert.match(fallback.error, /temporarily unavailable/);
    const fallbackAgain = await invoke('Offline title again');
    assert.equal(fallbackAgain.sessions[0].title, 'Offline title again');
    catalogFailure = false;
    const saved = readFileSync(path, 'utf8');
    await assert.rejects(invoke('bad\nname'), /printable/);
    assert.equal(readFileSync(path, 'utf8'), saved);
    rmSync(path);
    await assert.rejects(invoke('Missing session'), /ENOENT/);
  } finally {
    host.dispose();
    rmSync(directory, { recursive: true, force: true });
  }
});
