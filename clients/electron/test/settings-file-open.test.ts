import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { CH_SETTINGS_FILE_OPEN, HostController, openSettingsFile } from '../src/main/host';
import { DiagnosticBuffer } from '../src/main/host-utils';
import { SettingsStore } from '../src/main/settings';

test('settings files open through the OS with errors propagated and paths restricted', async () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-settings-open-'));
  const directory = join(root, '.lingxi');
  mkdirSync(directory);
  const opened: string[] = [];
  const opener = async (path: string) => { opened.push(path); return ''; };
  try {
    for (const name of ['settings.json', 'settings.local.json']) {
      const path = join(directory, name);
      writeFileSync(path, '{}');
      await openSettingsFile(path, opener);
      assert.equal(opened.at(-1), realpathSync.native(path));
    }
    const valid = join(directory, 'settings.json');
    await assert.rejects(openSettingsFile(valid, async () => 'No application is associated'), /No application/);
    await assert.rejects(openSettingsFile(valid, async () => { throw new Error('OS unavailable'); }), /OS unavailable/);
    for (const path of [null, 42, '.lingxi/settings.json', 'https://example.com', valid + '\0', join(root, 'settings.json'), join(directory, 'secret.json')]) {
      await assert.rejects(openSettingsFile(path, opener), /absolute|unsupported/);
    }
    rmSync(valid);
    await assert.rejects(openSettingsFile(valid, opener), /ENOENT/);
    mkdirSync(valid);
    await assert.rejects(openSettingsFile(valid, opener), /regular file/);
    rmSync(valid, { recursive: true });
    if (process.platform !== 'win32') {
      const outside = join(root, 'secret.json');
      writeFileSync(outside, '{}');
      symlinkSync(outside, valid);
      await assert.rejects(openSettingsFile(valid, opener), /regular file/);
    }
    assert.equal(opened.length, 2);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('settings file IPC rejects unknown windows and subframes and unregisters on disposal', async () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-settings-ipc-'));
  const handlers = new Map<string, (...args: any[]) => unknown>();
  const ipc = {
    handle: (channel: string, handler: (...args: any[]) => unknown) => { handlers.set(channel, handler); },
    removeHandler: (channel: string) => { handlers.delete(channel); },
  };
  const bridge = { registerIpc() {}, registerWindow() {} };
  const host = new HostController(new SettingsStore(root), bridge as any, new DiagnosticBuffer(), undefined, ipc as any);
  const frame = { url: 'http://127.0.0.1:4242' };
  const sender = { mainFrame: frame, once() {} };
  try {
    host.registerWindow(sender as any, frame.url);
    host.registerIpc();
    const handler = handlers.get(CH_SETTINGS_FILE_OPEN)!;
    assert.ok(handler);
    await assert.rejects(Promise.resolve(handler({ sender: {}, senderFrame: frame }, 'invalid')), /unauthorized IPC sender/);
    await assert.rejects(Promise.resolve(handler({ sender, senderFrame: { ...frame } }, 'invalid')), /unauthorized IPC sender/);
    await assert.rejects(Promise.resolve(handler({ sender, senderFrame: frame }, 'invalid')), /must be absolute/);
    host.dispose();
    assert.equal(handlers.has(CH_SETTINGS_FILE_OPEN), false);
  } finally {
    host.dispose();
    rmSync(root, { recursive: true, force: true });
  }
});
