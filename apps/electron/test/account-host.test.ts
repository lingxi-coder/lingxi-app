import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { HostController } from '../src/main/host';
import { DiagnosticBuffer } from '../src/main/host-utils';
import { SettingsStore } from '../src/main/settings';
import { CH_ACCOUNT, CH_ACCOUNT_CHANGED, type AccountState } from '../src/shared/account';

test('account IPC allows only a registered main frame, broadcasts profiles, and unregisters on dispose', async () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-account-ipc-'));
  const handlers = new Map<string, (...args: any[]) => any>();
  const ipc = { handle: (name: string, handler: (...args: any[]) => any) => handlers.set(name, handler), removeHandler: (name: string) => handlers.delete(name) };
  const host = new HostController(new SettingsStore(root), { registerIpc() {}, registerWindow() {} } as any, new DiagnosticBuffer(), undefined, ipc as any);
  let emit!: (state: AccountState) => void;
  let requests = 0, disposed = false, unsubscribed = false;
  const profile: AccountState = { user: { uid: 'u1', email: 'user@example.com', displayName: null, emailVerified: true }, busy: false, error: null, persistence: 'secure' };
  host.attachAccount({ execute: async () => { requests++; return profile; }, onChanged: (callback: typeof emit) => { emit = callback; return () => { unsubscribed = true; }; }, dispose: () => { disposed = true; } } as any);
  const frame = { url: 'http://127.0.0.1:4242' };
  const sent: unknown[] = [];
  const sender = { mainFrame: frame, isDestroyed: () => false, once() {}, removeListener() {}, send: (...args: unknown[]) => sent.push(args) };
  host.registerWindow(sender as any, frame.url);
  host.registerIpc();
  const invoke = handlers.get(CH_ACCOUNT)!;
  try {
    assert.throws(() => invoke({ sender, senderFrame: { url: frame.url } }, { type: 'status' }), /unauthorized/);
    assert.throws(() => invoke({ sender: { mainFrame: frame }, senderFrame: frame }, { type: 'status' }), /unauthorized/);
    assert.equal(requests, 0);
    assert.deepEqual(await invoke({ sender, senderFrame: frame }, { type: 'status' }), profile);
    emit(profile);
    assert.deepEqual(sent, [[CH_ACCOUNT_CHANGED, profile]]);
    frame.url = 'https://attacker.example';
    assert.throws(() => invoke({ sender, senderFrame: frame }, { type: 'status' }), /unauthorized/);
  } finally { host.dispose(); rmSync(root, { recursive: true, force: true }); }
  assert.ok(disposed && unsubscribed);
  assert.ok(!handlers.has(CH_ACCOUNT));
});
