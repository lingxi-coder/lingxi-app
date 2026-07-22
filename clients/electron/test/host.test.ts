import { test } from 'node:test';
import assert from 'node:assert/strict';

import { HostController } from '../src/main/host';
import { DiagnosticBuffer } from '../src/main/host-utils';

test('bootstrap surfaces an explicit recovery state when the persisted workspace is missing', () => {
  const diagnostics = new DiagnosticBuffer();
  const workspace = '/missing/workspace';
  const missing = Object.assign(new Error('workspace path is not a directory'), { code: 'ENOENT' });
  const settings = {
    getWorkspace: () => workspace,
    getTrust: () => { throw missing; },
    getPublic: () => ({ version: 1, recentWorkspaces: [workspace] }),
    credentialMetadata: () => ({ configured: false, encryptionAvailable: true }),
  };
  const bridge = {
    connectionState: { status: 'idle' as const },
    runtimeVersions: {
      serverName: 'lingxi-bridge-server/0.9.0',
      serverProtocol: '0.2.0',
      clientProtocol: '1.0.0',
    },
    turnActive: false,
    restart: async () => undefined,
  };
  const host = new HostController(settings as any, bridge as any, diagnostics);

  const bootstrap = (host as any).bootstrap();
  const report = JSON.parse((host as any).diagnosticReport());

  assert.deepEqual(bootstrap.workspace, {
    path: workspace,
    trusted: false,
    recovery: {
      state: 'missing',
      message: `stored workspace is unavailable: ${workspace}`,
    },
  });
  assert.deepEqual(report.workspace.recovery, {
    state: 'missing',
    message: `stored workspace is unavailable: ${workspace}`,
  });
  assert.deepEqual(report.bridgeRuntime, bridge.runtimeVersions);
  assert.match(diagnostics.snapshot()[0]?.message ?? '', /workspace path is not a directory/);
});
