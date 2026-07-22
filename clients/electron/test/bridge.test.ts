import { test } from 'node:test';
import assert from 'node:assert/strict';
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { EventEmitter } from 'node:events';

import { BridgeManager } from '../src/main/bridge';
import { DiagnosticBuffer } from '../src/main/host-utils';

function temporaryDirectory(): string {
  return mkdtempSync(join(tmpdir(), 'lingxi-electron-bridge-test-'));
}

test('restart exposes an explicit restarting state and structured connection diagnostics', async () => {
  const diagnostics = new DiagnosticBuffer();
  const manager = new BridgeManager({
    diagnostics,
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const states: string[] = [];
  (manager as any).registerIpc = () => undefined;
  (manager as any).broadcast = (_channel: string, payload: { status?: string }) => {
    if (payload?.status) states.push(payload.status);
  };
  (manager as any).stopBridge = async () => undefined;
  (manager as any).startInternal = async () => {
    (manager as any).setState({ status: 'connected' });
  };

  await manager.restart();

  assert.deepEqual(states, ['restarting', 'connected']);
  const events = diagnostics.snapshot().map((entry) => JSON.parse(entry.message));
  assert.deepEqual(
    events.filter((entry) => entry.event === 'connection_state').map((entry) => entry.state.status),
    ['restarting', 'connected'],
  );
});

test('lockfile polling ignores invalid candidates until a valid private lockfile appears', async () => {
  const diagnostics = new DiagnosticBuffer();
  const workspace = temporaryDirectory();
  const launchDir = temporaryDirectory();
  const lockfilePath = join(launchDir, 'bridge.lock');
  const manager = new BridgeManager({
    diagnostics,
    lockfileTimeoutMs: 500,
    launchConfig: () => ({ workspace, trusted: true }),
  });
  (manager as any).generation = 1;
  (manager as any).child = { pid: process.pid };

  writeFileSync(lockfilePath, '{"pid":', { mode: 0o600 });
  chmodSync(lockfilePath, 0o600);
  setTimeout(() => {
    writeFileSync(lockfilePath, JSON.stringify({
      pid: process.pid,
      workspaceFolders: [workspace],
      ideName: 'LingXi-Bridge',
      transport: 'ws',
      runningInWindows: false,
      authToken: '0123456789abcdef0123456789abcdef',
    }), { mode: 0o600 });
    chmodSync(lockfilePath, 0o600);
  }, 75);

  try {
    const resolved = await (manager as any).waitForLockfile(launchDir, { workspace, trusted: true }, 1);
    assert.equal(resolved, lockfilePath);
    const ignored = diagnostics.snapshot().map((entry) => JSON.parse(entry.message))
      .find((entry) => entry.event === 'lockfile_ignored');
    assert.equal(ignored?.file, lockfilePath);
  } finally {
    rmSync(workspace, { recursive: true, force: true });
    rmSync(launchDir, { recursive: true, force: true });
  }
});

test('privileged bridge access re-checks current workspace trust before use', () => {
  let trusted = true;
  const client = { sendCommand: () => undefined };
  const manager = new BridgeManager({
    launchConfig: () => ({ workspace: '/workspace', trusted, apiKey: 'credential' }),
  });
  (manager as any).client = client;
  (manager as any).activeWorkspace = '/workspace';
  (manager as any).activeWorkspaceTrusted = true;
  (manager as any).state = { status: 'connected' };

  assert.equal((manager as any).requireClient(), client);
  trusted = false;
  assert.throws(() => (manager as any).requireClient(), /workspace trust is required/);
});

test('clean child exit clears the SIGKILL timer so the process group is not signalled twice', async () => {
  const manager = new BridgeManager({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
    stopTimeoutMs: 40,
  });
  const child = new EventEmitter() as EventEmitter & {
    exitCode: number | null;
    signalCode: NodeJS.Signals | null;
    pid: number;
  };
  child.exitCode = null;
  child.signalCode = null;
  child.pid = 4242;

  const signals: NodeJS.Signals[] = [];
  (manager as any).child = child;
  (manager as any).signalChildTree = (_child: unknown, signal: NodeJS.Signals) => {
    signals.push(signal);
  };

  const stopping = (manager as any).stopBridge();
  setTimeout(() => child.emit('exit', 0, null), 10);
  await stopping;
  await new Promise((resolve) => setTimeout(resolve, 80));

  assert.deepEqual(signals, ['SIGINT']);
});
