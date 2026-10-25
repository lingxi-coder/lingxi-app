import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import type { BridgeLaunchConfig } from '../src/main/bridgeTypes';
import { SessionRuntimeManager } from '../src/main/sessionRuntimeManager';

test('actual child spawns retain current host Computer Use authority across workspace overlays and restarts', {
  skip: process.platform === 'win32',
}, async t => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-computer-env-'));
  const serverBin = join(root, 'capture-environment');
  writeFileSync(serverBin, `#!/usr/bin/env node
const fs = require('node:fs');
const path = require('node:path');
const directory = process.argv[process.argv.indexOf('--bridge-dir') + 1];
fs.writeFileSync(path.join(directory, 'environment.json'), JSON.stringify({
  policy: process.env.LINGXI_DESKTOP_COMPUTER_POLICY ?? null,
  browser: process.env.LINGXI_DESKTOP_BROWSER_DESCRIPTOR ?? null,
  workspace: process.env.WORKSPACE_VALUE,
}));
setInterval(() => {}, 1000);
`, { mode: 0o700 });
  let authority: BridgeLaunchConfig['computerUseEnvironment'] = {
    LINGXI_DESKTOP_COMPUTER_POLICY: join(root, 'host-policy.json'),
    LINGXI_DESKTOP_BROWSER_DESCRIPTOR: join(root, 'host-browser.json'),
  };
  const manager = new SessionRuntimeManager({
    serverBin, bridgeRoot: join(root, 'runtime'), stopTimeoutMs: 500,
    listProcessCommands: () => [],
    launchConfig: ref => ({ workspace: root, sessionId: ref.sessionId, trusted: true, computerUseEnvironment: authority }),
    workspaceSettings: async () => ({ configHome: root, environment: {
      WORKSPACE_VALUE: 'retained',
      LINGXI_DESKTOP_COMPUTER_POLICY: join(root, 'workspace-policy.json'),
      LINGXI_DESKTOP_BROWSER_DESCRIPTOR: join(root, 'workspace-browser.json'),
    } }),
  });
  t.after(async () => { await manager.dispose(); rmSync(root, { recursive: true, force: true }); });
  const runtime = await manager.ensure({ projectPath: root, sessionId: '11111111-2222-4333-8444-555555555555' }, false);
  const internal = runtime as any;
  // Only the wire handshake is stubbed; launch configuration, spawn and lifecycle
  // are real, and a separate child records the environment it actually received.
  internal.waitForLockfile = async () => '/unused-test-handshake';
  internal.connectBridgeClient = async () => internal.setState({ status: 'connected' });
  async function childEnvironment() {
    const path = join(internal.launchDir, 'environment.json');
    const deadline = Date.now() + 5000;
    for (;;) {
      try { return JSON.parse(readFileSync(path, 'utf8')); }
      catch (error) { if (Date.now() >= deadline) throw error; }
      await new Promise(resolve => setTimeout(resolve, 10));
    }
  }
  await runtime.start();
  assert.deepEqual(await childEnvironment(), {
    policy: authority.LINGXI_DESKTOP_COMPUTER_POLICY,
    browser: authority.LINGXI_DESKTOP_BROWSER_DESCRIPTOR, workspace: 'retained',
  });
  authority = {
    LINGXI_DESKTOP_COMPUTER_POLICY: join(root, 'host-policy.disabled-after-failure'),
    LINGXI_DESKTOP_BROWSER_DESCRIPTOR: join(root, 'host-browser-after-reconnect.json'),
  };
  await runtime.restart();
  assert.deepEqual(await childEnvironment(), {
    policy: authority.LINGXI_DESKTOP_COMPUTER_POLICY,
    browser: authority.LINGXI_DESKTOP_BROWSER_DESCRIPTOR, workspace: 'retained',
  });
  authority = undefined;
  await runtime.restart();
  assert.deepEqual(await childEnvironment(), { policy: null, browser: null, workspace: 'retained' });
});
