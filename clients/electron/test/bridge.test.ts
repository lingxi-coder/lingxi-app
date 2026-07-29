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

test('restart surfaces launch failures instead of remaining stuck in restarting', async () => {
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
    throw new Error('macOS login keychain is locked or access is denied (deepseek)');
  };

  await assert.rejects(manager.restart(), /login keychain is locked/);

  assert.deepEqual(states, ['restarting', 'error']);
  assert.deepEqual(manager.connectionState, {
    status: 'error',
    message: 'macOS login keychain is locked or access is denied (deepseek)',
  });
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
    launchConfig: () => ({ workspace: '/workspace', trusted }),
  });
  (manager as any).client = client;
  (manager as any).activeWorkspace = '/workspace';
  (manager as any).activeWorkspaceTrusted = true;
  (manager as any).state = { status: 'connected' };

  assert.equal((manager as any).requireClient(), client);
  trusted = false;
  assert.throws(() => (manager as any).requireClient(), /workspace trust is required/);
});

test('computer access requests broadcast to renderers and are tracked as pending', async () => {
  const manager = new BridgeManager({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const broadcasts: Array<{ channel: string; payload: unknown }> = [];
  (manager as any).broadcast = (channel: string, payload: unknown) => broadcasts.push({ channel, payload });
  const handlers = new Map<string, (...args: unknown[]) => void>();
  const fakeClient = {
    on: (event: string, handler: (...args: unknown[]) => void) => { handlers.set(event, handler); return fakeClient; },
  };

  (manager as any).wireClient(fakeClient, 0);
  const request = {
    request_id: 5,
    reason: 'automate chat',
    apps: [{ label: 'Slack' }],
    tier: 'full',
    clipboard_read: false,
    clipboard_write: false,
    system_key_combos: false,
  };
  handlers.get('computerAccess')!(request);

  assert.deepEqual(broadcasts, [{ channel: 'lingxi:computerAccess', payload: request }]);
  assert.ok((manager as any).pendingComputerAccessIds.has(5));

  // stopBridge (restart/disconnect) clears pending computer access ids, just
  // like it clears pending permission ids.
  (manager as any).child = null;
  await (manager as any).stopBridge();
  assert.equal((manager as any).pendingComputerAccessIds.size, 0);
});

test('AskUserQuestion events are tracked and cleared across disconnect', async () => {
  const manager = new BridgeManager({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const broadcasts: Array<{ channel: string; payload: unknown }> = [];
  (manager as any).broadcast = (channel: string, payload: unknown) => broadcasts.push({ channel, payload });
  const handlers = new Map<string, (...args: unknown[]) => void>();
  const fakeClient = {
    on: (event: string, handler: (...args: unknown[]) => void) => { handlers.set(event, handler); return fakeClient; },
  };
  const event = {
    type: 'ask_user_question',
    request: {
      request_id: 7,
      questions: [{
        question: 'Choose a mode',
        header: 'Mode',
        options: [{ label: 'Safe', description: 'Keep safeguards enabled' }],
        multi_select: false,
      }],
    },
  };

  (manager as any).wireClient(fakeClient, 0);
  handlers.get('event')!(event);

  assert.deepEqual(broadcasts, [{ channel: 'lingxi:event', payload: event }]);
  assert.ok((manager as any).pendingAskUserQuestionIds.has(7));
  assert.deepEqual((manager as any).pendingAskUserQuestionRequests.get(7)?.request, event.request);
  assert.deepEqual(manager.pendingAskUserQuestions, [event.request]);

  (manager as any).child = null;
  await (manager as any).stopBridge();
  assert.equal((manager as any).pendingAskUserQuestionIds.size, 0);
  assert.equal((manager as any).pendingAskUserQuestionRequests.size, 0);
});

test('AskUserQuestion resolved events clear replay state before a renderer reload', () => {
  const manager = new BridgeManager({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const handlers = new Map<string, (...args: unknown[]) => void>();
  const fakeClient = {
    on: (event: string, handler: (...args: unknown[]) => void) => { handlers.set(event, handler); return fakeClient; },
  };

  (manager as any).wireClient(fakeClient, 0);
  handlers.get('event')!({
    type: 'ask_user_question',
    request: {
      request_id: 11,
      questions: [{
        question: 'Choose a mode',
        header: 'Mode',
        options: [{ label: 'Safe', description: 'Keep safeguards enabled' }],
        multi_select: false,
      }],
      timeout_secs: 60,
    },
  });
  handlers.get('event')!({
    type: 'ask_user_question_resolved',
    request_id: 11,
  });

  assert.equal((manager as any).pendingAskUserQuestionIds.has(11), false);
  assert.equal((manager as any).pendingAskUserQuestionRequests.has(11), false);
});

test('AskUserQuestion broker resolution clears replay state without a renderer answer', async () => {
  const manager = new BridgeManager({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const handlers = new Map<string, (...args: unknown[]) => void>();
  const fakeClient = {
    on: (event: string, handler: (...args: unknown[]) => void) => { handlers.set(event, handler); return fakeClient; },
  };

  (manager as any).broadcast = () => undefined;
  (manager as any).wireClient(fakeClient, 0);
  handlers.get('event')!({
    type: 'ask_user_question',
    request: {
      request_id: 12,
      timeout_secs: 0,
      questions: [{
        question: 'Choose a mode',
        header: 'Mode',
        options: [
          { label: 'Safe', description: 'Keep safeguards enabled' },
          { label: 'Fast', description: 'Move quicker' },
        ],
        multi_select: false,
      }],
    },
  });
  handlers.get('event')!({
    type: 'ask_user_question_resolved',
    request_id: 12,
  });

  assert.equal((manager as any).pendingAskUserQuestionIds.has(12), false);
  assert.equal((manager as any).pendingAskUserQuestionRequests.has(12), false);
});

test('registerWindow replays pending AskUserQuestion requests to a reloaded renderer', () => {
  const manager = new BridgeManager({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const handlers = new Map<string, (...args: unknown[]) => void>();
  const fakeClient = {
    on: (event: string, handler: (...args: unknown[]) => void) => { handlers.set(event, handler); return fakeClient; },
  };
  const sent: Array<{ channel: string; payload: unknown }> = [];
  const webContents = {
    send: (channel: string, payload: unknown) => sent.push({ channel, payload }),
    once: (_event: string, _handler: () => void) => undefined,
    isDestroyed: () => false,
  };
  const event = {
    type: 'ask_user_question',
    request: {
      request_id: 13,
      questions: [{
        question: 'Choose a mode',
        header: 'Mode',
        options: [{ label: 'Safe', description: 'Keep safeguards enabled' }],
        multi_select: false,
      }],
    },
  };

  (manager as any).wireClient(fakeClient, 0);
  handlers.get('event')!(event);

  manager.registerWindow(webContents as any, 'app://desktop/index.html');

  assert.deepEqual(sent, [{ channel: 'lingxi:event', payload: event }]);
});

test('provider credential status is sourced from the engine secure store', async () => {
  const commands: Array<Record<string, unknown>> = [];
  const manager = new BridgeManager({
    providerIds: ['deepseek'],
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  (manager as any).client = { sendCommand: (command: Record<string, unknown>) => commands.push(command) };
  (manager as any).state = { status: 'connected' };
  (manager as any).broadcast = () => undefined;

  const pending = manager.listProviderCredentials(['deepseek']);
  const command = commands[0]!;
  assert.equal(command['type'], 'list_provider_credentials');
  assert.deepEqual(command['provider_ids'], ['deepseek']);

  (manager as any).handleProviderCredentialStatus({
    type: 'provider_credential_status',
    operation_id: command['operation_id'],
    configured_provider_ids: ['deepseek'],
    storage_encrypted: true,
  });

  await pending;
  assert.deepEqual(manager.persistedCredentialProviderIds, ['deepseek']);
  assert.deepEqual(manager.activeCredentialProviderIds, ['deepseek']);
  assert.equal(manager.providerCredentialStorageEncrypted, true);
});

test('provider credential writes cross only the authenticated bridge command path', async () => {
  const commands: Array<Record<string, unknown>> = [];
  const manager = new BridgeManager({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  (manager as any).client = { sendCommand: (command: Record<string, unknown>) => commands.push(command) };

  const pending = manager.setProviderCredential('deepseek', 'sk-test-secret');
  const command = commands[0]!;
  assert.equal(command['type'], 'set_provider_credential');
  assert.equal(command['provider_id'], 'deepseek');
  assert.equal(command['credential'], 'sk-test-secret');

  (manager as any).handleProviderCredentialStatus({
    type: 'provider_credential_status',
    operation_id: command['operation_id'],
    configured_provider_ids: ['deepseek'],
    storage_encrypted: true,
  });
  await pending;
});

test('partial credential read failures preserve unavailable providers and publish successful status', async () => {
  const commands: Array<Record<string, unknown>> = [];
  const broadcasts: unknown[] = [];
  const manager = new BridgeManager({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  (manager as any).client = { sendCommand: (command: Record<string, unknown>) => commands.push(command) };
  (manager as any).state = { status: 'connected' };
  (manager as any).persistedCredentialProviders.add('openrouter');
  (manager as any).broadcast = (_channel: string, payload: unknown) => broadcasts.push(payload);

  const pending = manager.listProviderCredentials(['deepseek', 'openrouter']);
  const command = commands[0]!;
  (manager as any).handleProviderCredentialStatus({
    type: 'provider_credential_status',
    operation_id: command['operation_id'],
    configured_provider_ids: ['deepseek'],
    unavailable_provider_ids: ['openrouter'],
    storage_encrypted: true,
    error: 'openrouter: macOS login keychain is locked',
  });

  await assert.rejects(pending, /login keychain is locked/);
  assert.deepEqual([...manager.persistedCredentialProviderIds].sort(), ['deepseek', 'openrouter']);
  assert.equal(broadcasts.length, 1, 'successful partial status must reach the renderer');
});

test('legacy desktop credentials migrate only when the shared store has no value', async () => {
  const migrated: string[] = [];
  const writes: Array<[string, string]> = [];
  const manager = new BridgeManager({
    providerIds: ['deepseek', 'openrouter'],
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
    onProviderCredentialMigrated: (providerId) => migrated.push(providerId),
  });
  (manager as any).generation = 7;
  (manager as any).listProviderCredentials = async () => {
    (manager as any).persistedCredentialProviders.add('openrouter');
  };
  (manager as any).setProviderCredential = async (providerId: string, credential: string) => {
    writes.push([providerId, credential]);
    (manager as any).persistedCredentialProviders.add(providerId);
  };

  await (manager as any).refreshAndMigrateProviderCredentials({
    workspace: '/workspace',
    trusted: true,
    providerCredentialsToMigrate: {
      deepseek: 'sk-legacy-deepseek',
      openrouter: 'sk-legacy-openrouter',
    },
  }, 7);

  assert.deepEqual(writes, [['deepseek', 'sk-legacy-deepseek']]);
  assert.deepEqual(migrated, ['deepseek', 'openrouter']);
});

test('legacy credential migration is deferred when the engine store list fails', async () => {
  const migrated: string[] = [];
  const writes: Array<[string, string]> = [];
  const manager = new BridgeManager({
    providerIds: ['deepseek', 'openrouter'],
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
    onProviderCredentialMigrated: (providerId) => migrated.push(providerId),
  });
  (manager as any).generation = 7;
  // The engine-store read FAILS (e.g. keychain unlock timeout). persistedCredentialProviders
  // stays reset-empty, but that emptiness is NOT authoritative — migrating would
  // clobber whatever newer key the user stored via CLI/TUI in the shared store.
  (manager as any).listProviderCredentials = async () => {
    throw new Error('keychain unavailable');
  };
  (manager as any).setProviderCredential = async (providerId: string, credential: string) => {
    writes.push([providerId, credential]);
    (manager as any).persistedCredentialProviders.add(providerId);
  };

  await (manager as any).refreshAndMigrateProviderCredentials({
    workspace: '/workspace',
    trusted: true,
    providerCredentialsToMigrate: {
      deepseek: 'sk-legacy-deepseek',
      openrouter: 'sk-legacy-openrouter',
    },
  }, 7);

  // The whole migration loop is skipped when the persisted-state read is unknown.
  assert.deepEqual(writes, []);
  assert.deepEqual(migrated, []);
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

// ── Bypass Permissions acceptance gate (security #34) ──────────────────────

function trustedManager(confirm?: () => Promise<boolean>): {
  manager: BridgeManager;
  commands: Array<Record<string, unknown>>;
} {
  const commands: Array<Record<string, unknown>> = [];
  const manager = new BridgeManager({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
    accessState: () => ({ workspace: '/workspace', trusted: true }),
    ...(confirm ? { confirmBypassPermissions: confirm } : {}),
  });
  (manager as any).client = {
    sendCommand: (command: Record<string, unknown>) => commands.push(command),
  };
  (manager as any).activeWorkspace = '/workspace';
  return { manager, commands };
}

test('bypassPermissions is refused when the acceptance dialog is declined', async () => {
  let confirmCalls = 0;
  const { manager, commands } = trustedManager(async () => {
    confirmCalls += 1;
    return false; // user picked Cancel
  });
  await assert.rejects(
    (manager as any).dispatchCommand({ type: 'set_permission_mode', mode: 'bypassPermissions' }),
    /not accepted/,
  );
  assert.equal(confirmCalls, 1, 'the acceptance confirmer is consulted');
  assert.equal(commands.length, 0, 'a declined bypass never reaches the engine');
});

test('bypassPermissions reaches the engine only after acceptance', async () => {
  const { manager, commands } = trustedManager(async () => true); // user accepted
  await (manager as any).dispatchCommand({ type: 'set_permission_mode', mode: 'bypassPermissions' });
  assert.equal(commands.length, 1);
  assert.equal(commands[0]!['type'], 'set_permission_mode');
  assert.equal(commands[0]!['mode'], 'bypassPermissions');
});

test('bypassPermissions is refused when no confirmer is wired (never one-click)', async () => {
  const { manager, commands } = trustedManager(); // no confirmBypassPermissions
  await assert.rejects(
    (manager as any).dispatchCommand({ type: 'set_permission_mode', mode: 'bypassPermissions' }),
    /not accepted/,
  );
  assert.equal(commands.length, 0);
});

test('non-bypass permission modes are not gated by the acceptance dialog', async () => {
  let confirmCalls = 0;
  const { manager, commands } = trustedManager(async () => {
    confirmCalls += 1;
    return true;
  });
  await (manager as any).dispatchCommand({ type: 'set_permission_mode', mode: 'acceptEdits' });
  assert.equal(confirmCalls, 0, 'only bypassPermissions consults the confirmer');
  assert.equal(commands.length, 1);
  assert.equal(commands[0]!['mode'], 'acceptEdits');
});
