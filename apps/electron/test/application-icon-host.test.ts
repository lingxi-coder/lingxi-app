import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { applicationIcon } from '../src/main/applicationIcon';
import { HostController } from '../src/main/host';
import { DiagnosticBuffer } from '../src/main/host-utils';
import { SettingsStore } from '../src/main/settings';
import { CH_APPLICATION_ICON } from '../src/shared/applicationIcon';

test('native application lookup returns a bounded PNG and missing apps fall back', { skip: process.platform !== 'darwin' }, async () => {
  const [first, second] = await Promise.all([applicationIcon('com.apple.TextEdit'), applicationIcon('com.apple.TextEdit')]);
  assert.ok(first?.startsWith('data:image/png;base64,'));
  assert.equal(first, second);
  const png = Buffer.from(first.split(',')[1]!, 'base64');
  assert.ok(png.readUInt32BE(16) >= 40 && png.readUInt32BE(16) <= 80);
  assert.ok(png.readUInt32BE(20) >= 40 && png.readUInt32BE(20) <= 80);
  assert.ok(png.byteLength < 128 * 1024);
  assert.equal(await applicationIcon('com.lingxi.nonexistent-icon-test'), null);
  assert.equal(await applicationIcon('/System/Applications/TextEdit.app'), null);
});

test('application icon IPC rejects unregistered and child frames and unregisters on disposal', async () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-icon-ipc-'));
  const handlers = new Map<string, (...args: any[]) => any>();
  const ipc = { handle: (name: string, handler: (...args: any[]) => any) => handlers.set(name, handler), removeHandler: (name: string) => handlers.delete(name) };
  const host = new HostController(new SettingsStore(root), { registerIpc() {}, registerWindow() {} } as any, new DiagnosticBuffer(), undefined, ipc as any);
  const frame = { url: 'http://127.0.0.1:4242' };
  const sender = { mainFrame: frame, isDestroyed: () => false, once() {}, removeListener() {}, send() {} };
  host.registerWindow(sender as any, frame.url);
  host.registerIpc();
  const invoke = handlers.get(CH_APPLICATION_ICON)!;
  try {
    await assert.rejects(invoke({ sender, senderFrame: { url: frame.url } }, 'com.apple.TextEdit'), /unauthorized/);
    await assert.rejects(invoke({ sender: { mainFrame: frame }, senderFrame: frame }, 'com.apple.TextEdit'), /unauthorized/);
    assert.equal(await invoke({ sender, senderFrame: frame }, '/Applications/TextEdit.app'), null);
  } finally { host.dispose(); rmSync(root, { recursive: true, force: true }); }
  assert.ok(!handlers.has(CH_APPLICATION_ICON));
});
