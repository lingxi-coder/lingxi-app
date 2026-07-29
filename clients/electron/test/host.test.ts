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

test('bootstrap treats a credential already supplied to the running engine as configured', () => {
  const diagnostics = new DiagnosticBuffer();
  const settings = {
    getWorkspace: () => '/workspace',
    getTrust: () => ({ trusted: true, fingerprint: 'fingerprint' }),
    getPublic: () => ({ version: 1, recentWorkspaces: ['/workspace'] }),
    credentialMetadata: () => ({ configured: false, encryptionAvailable: false }),
    providerCredentialMetadataFor: (providerIds: readonly string[]) => providerIds.map((providerId) => ({
      providerId,
      configured: false,
      encryptionAvailable: false,
    })),
  };
  const bridge = {
    connectionState: { status: 'connected' as const },
    activeCredentialProviderIds: ['deepseek'],
    turnActive: false,
  };
  const host = new HostController(settings as any, bridge as any, diagnostics);

  const bootstrap = (host as any).bootstrap();
  const deepseek = bootstrap.providerCredentials.find((entry: { providerId: string }) => entry.providerId === 'deepseek');

  assert.deepEqual(deepseek, {
    providerId: 'deepseek',
    configured: true,
    encryptionAvailable: false,
    runtimeOnly: true,
  });
});

test('bootstrap reports CLI/TUI credentials discovered by the shared engine store as persisted', () => {
  const diagnostics = new DiagnosticBuffer();
  const settings = {
    getWorkspace: () => '/workspace',
    getTrust: () => ({ trusted: true, fingerprint: 'fingerprint' }),
    getPublic: () => ({ version: 1, recentWorkspaces: ['/workspace'] }),
    credentialMetadata: () => ({ configured: false, encryptionAvailable: false }),
    providerCredentialMetadataFor: (providerIds: readonly string[]) => providerIds.map((providerId) => ({
      providerId,
      configured: false,
      encryptionAvailable: false,
    })),
  };
  const bridge = {
    connectionState: { status: 'connected' as const },
    activeCredentialProviderIds: ['deepseek'],
    persistedCredentialProviderIds: ['deepseek'],
    providerCredentialStorageEncrypted: true,
    turnActive: false,
  };
  const host = new HostController(settings as any, bridge as any, diagnostics);

  const bootstrap = (host as any).bootstrap();
  const deepseek = bootstrap.providerCredentials.find((entry: { providerId: string }) => entry.providerId === 'deepseek');

  assert.deepEqual(deepseek, {
    providerId: 'deepseek',
    configured: true,
    encryptionAvailable: true,
  });
});

test('bootstrap replays pending AskUserQuestion requests after a renderer reload', () => {
  const diagnostics = new DiagnosticBuffer();
  const settings = {
    getWorkspace: () => '/workspace',
    getTrust: () => ({ trusted: true, fingerprint: 'fingerprint' }),
    getPublic: () => ({ version: 1, recentWorkspaces: ['/workspace'] }),
    credentialMetadata: () => ({ configured: false, encryptionAvailable: false }),
    providerCredentialMetadataFor: () => [],
  };
  const bridge = {
    connectionState: { status: 'connected' as const },
    pendingAskUserQuestions: [{
      request_id: 7,
      questions: [{
        question: 'Choose a mode',
        header: 'Mode',
        options: [{ label: 'Safe', description: 'Keep safeguards enabled' }],
        multi_select: false,
      }],
      timeout_secs: 60,
    }],
    turnActive: false,
  };
  const host = new HostController(settings as any, bridge as any, diagnostics);

  const bootstrap = (host as any).bootstrap();

  assert.deepEqual(bootstrap.pendingAskUserQuestions, bridge.pendingAskUserQuestions);
});
