import assert from 'node:assert/strict';
import { createServer, type IncomingHttpHeaders } from 'node:http';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { ProviderVaultClient, ProviderVaultError, vaultInstallPlan, vaultUpsertFor } from '../src/main/provider-vault';
import { CH_PROVIDER_VAULT, VAULT_PRESET_IDS, parseProviderVaultCommand } from '../src/shared/providerVault';
import { CH_PROVIDER_ACCOUNTS } from '../src/shared/providerAccounts';
import { HostController } from '../src/main/host';
import { DiagnosticBuffer } from '../src/main/host-utils';
import { SettingsStore } from '../src/main/settings';
import { PROVIDERS } from '../src/shared/providers';
import type { VaultProvider } from '../../../packages/llm-vault/contracts';

const provider = (over: Partial<VaultProvider> = {}): VaultProvider => ({
  id: 'deepseek', presetId: 'deepseek', label: 'DeepSeek', protocol: 'open_ai_chat', desktopType: null, baseUrl: 'https://api.deepseek.com',
  models: [{ id: 'deepseek-flash' }], defaultModel: 'deepseek-flash', keyHint: 'ab12', source: 'web', revision: 1,
  webUsable: true, createdAt: 1, updatedAt: 1, ...over,
});
const account = (uid = 'u1') => {
  const state = { uid, epoch: 0, tokens: [] as boolean[] };
  return { state, snapshot: () => ({ user: state.uid ? { uid: state.uid } : null }), sessionEpoch: () => state.epoch,
    getIdToken: async (force = false) => { state.tokens.push(force); return force ? 'fresh-token' : 'cached-token'; } };
};

async function server(handler: (method: string, url: string, headers: IncomingHttpHeaders, body: string) => { status: number; body: unknown }) {
  const seen: Array<{ method: string; url: string; headers: IncomingHttpHeaders; body: string }> = [];
  const http = createServer((request, response) => {
    let body = '';
    request.on('data', chunk => { body += chunk; });
    request.on('end', () => {
      seen.push({ method: request.method!, url: request.url!, headers: request.headers, body });
      const reply = handler(request.method!, request.url!, request.headers, body);
      response.writeHead(reply.status, { 'Content-Type': 'application/json' }).end(JSON.stringify(reply.body));
    });
  });
  await new Promise<void>(resolve => http.listen(0, '127.0.0.1', resolve));
  const { port } = http.address() as { port: number };
  return { base: `http://127.0.0.1:${port}/v1/llm`, seen, close: () => new Promise<void>(resolve => http.close(() => resolve())) };
}

test('the vault client calls from the main process without an Origin, as the desktop client, with the account token', async () => {
  const vault = await server((method, url) => method === 'GET' ? { status: 200, body: { providers: [provider()] } }
    : url.endsWith('/export') ? { status: 200, body: { providers: [{ ...provider(), apiKey: 'sk-secret' }] } } : { status: 200, body: { provider: provider() } });
  try {
    const client = new ProviderVaultClient(account(), vault.base);
    assert.deepEqual((await client.list('u1')).map(item => item.id), ['deepseek']);
    await client.upsert('u1', { action: 'upsert', id: 'deepseek', presetId: 'deepseek', apiKey: 'sk-new' });
    assert.equal((await client.exportKeys('u1', ['deepseek']))[0]!.apiKey, 'sk-secret');
    await client.remove('u1', 'deepseek', 3);
    assert.deepEqual(vault.seen.map(call => `${call.method} ${call.url}`), ['GET /v1/llm/providers', 'POST /v1/llm/providers', 'POST /v1/llm/providers/export', 'POST /v1/llm/providers']);
    for (const call of vault.seen) {
      assert.equal(call.headers.origin, undefined, 'a request with Origin is treated as a browser and refused an export');
      assert.equal(call.headers['x-lingxi-client'], 'desktop');
      assert.equal(call.headers.authorization, 'Bearer cached-token');
    }
    assert.deepEqual(JSON.parse(vault.seen[2]!.body), { ids: ['deepseek'] });
    assert.deepEqual(JSON.parse(vault.seen[3]!.body), { action: 'delete', id: 'deepseek', expectedRevision: 3 });
  } finally { await vault.close(); }
});

test('the vault client retries once with a fresh token, reports vault codes, and refuses a changed account', async () => {
  let calls = 0;
  const vault = await server((_method, _url, headers) => {
    calls++;
    if (headers.authorization === 'Bearer cached-token') return { status: 401, body: { error: { code: 'INVALID_SESSION' } } };
    return calls === 2 ? { status: 200, body: { providers: [] } } : { status: 401, body: { error: { code: 'REAUTH_REQUIRED' } } };
  });
  try {
    const signedIn = account();
    const client = new ProviderVaultClient(signedIn, vault.base);
    assert.deepEqual(await client.list('u1'), []);
    assert.deepEqual(signedIn.state.tokens, [false, true]);
    await assert.rejects(client.exportKeys('u1', ['deepseek']), (error: unknown) => error instanceof ProviderVaultError && error.code === 'REAUTH_REQUIRED');
    await assert.rejects(client.list('someone-else'), (error: unknown) => error instanceof ProviderVaultError && error.code === 'INVALID_SESSION');
    signedIn.state.epoch++;
    const before = calls;
    const switched = new ProviderVaultClient({ ...signedIn, getIdToken: async () => { signedIn.state.uid = 'u2'; return 'fresh-token'; } }, vault.base);
    await assert.rejects(switched.list('u1'), (error: unknown) => error instanceof ProviderVaultError && error.code === 'INVALID_SESSION');
    assert.equal(calls, before, 'nothing is sent once the account changed');
  } finally { await vault.close(); }
  await assert.rejects(new ProviderVaultClient(account(), 'http://127.0.0.1:9/v1/llm').list('u1'), (error: unknown) => error instanceof ProviderVaultError && error.code === 'NETWORK');
});

test('a key saved on this Mac maps to a vault record: built-in presets by id, custom providers by their protocol', () => {
  assert.deepEqual(vaultUpsertFor('deepseek', undefined, 'sk-1'), { action: 'upsert', id: 'deepseek', presetId: 'deepseek', label: 'DeepSeek', apiKey: 'sk-1' });
  // The Desktop default model is an engine alias ('deepseek/deepseek-flash'); the preset carries the wire id instead.
  assert.equal(vaultUpsertFor('deepseek', undefined, 'sk-1')?.defaultModel, undefined);
  assert.equal(vaultUpsertFor('github-copilot', undefined, 'token'), null);
  assert.deepEqual(vaultUpsertFor('my-proxy', { type: 'openai', baseUrl: ' https://llm.example.com/v1 ', models: [{ id: 'm1 ' }, 'm2', { id: 'm1' }] }, 'sk-2'), {
    action: 'upsert', id: 'my-proxy', label: 'my-proxy', protocol: 'open_ai_chat', desktopType: 'openai', baseUrl: 'https://llm.example.com/v1',
    models: [{ id: 'm1' }, { id: 'm2' }], defaultModel: 'm1', apiKey: 'sk-2',
  });
  assert.equal(vaultUpsertFor('my-azure', { type: 'azure-openai', baseUrl: 'https://x.openai.azure.com', models: [] }, 'k')?.protocol, 'other');
  assert.equal(vaultUpsertFor('local', { type: 'openai', baseUrl: 'http://localhost:11434/v1', models: [{ id: 'llama' }] }, 'k'), null);
  assert.equal(vaultUpsertFor('odd', { type: 'mystery', baseUrl: 'https://x.example.com' }, 'k'), null);
  for (const id of VAULT_PRESET_IDS) if (PROVIDERS.some(entry => entry.id === id)) assert.ok(vaultUpsertFor(id, undefined, 'k'), id);
});

test('a vault provider lands on this Mac as a built-in account or a custom provider', () => {
  assert.deepEqual(vaultInstallPlan(provider(), {}), { kind: 'builtin', providerId: 'deepseek', displayName: 'DeepSeek' });
  assert.deepEqual(vaultInstallPlan(provider({ id: 'deepseek-work', label: 'DeepSeek (work)' }), {}), { kind: 'builtin', providerId: 'deepseek', displayName: 'DeepSeek (work)' });
  // A preset Desktop has no built-in provider for becomes a custom provider, its default model first.
  assert.deepEqual(vaultInstallPlan(provider({ id: 'qwen', presetId: 'qwen', label: 'Qwen', baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
    models: [{ id: 'a' }, { id: 'qwen-max' }], defaultModel: 'qwen-max' }), {}), {
    kind: 'custom', providerId: 'qwen', displayName: 'Qwen', profile: { type: 'openai', baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1', models: [{ id: 'qwen-max' }, { id: 'a' }] },
  });
  const custom = provider({ id: 'gw', presetId: null, label: 'Gateway', protocol: 'anthropic_messages', baseUrl: 'https://gw.example.com', models: [], defaultModel: 'claude-x' });
  assert.deepEqual(vaultInstallPlan(custom, {}), { kind: 'custom', providerId: 'gw', displayName: 'Gateway', profile: { type: 'anthropic', baseUrl: 'https://gw.example.com', models: [{ id: 'claude-x' }] } });
  assert.deepEqual(vaultInstallPlan(custom, { gw: { type: 'anthropic', baseUrl: 'https://gw.example.com/' } }), { kind: 'custom', providerId: 'gw', displayName: 'Gateway', profile: null });
  // A provider of this Mac that only shares the name never gets the key: it would send it to its own endpoint.
  for (const local of [{ type: 'anthropic', baseUrl: 'https://other.example.com' }, { type: 'openai', baseUrl: 'https://gw.example.com' }, { type: 'anthropic' }, 'gw']) {
    assert.deepEqual(vaultInstallPlan(custom, { gw: local }), { kind: 'unsupported', code: 'ID_CONFLICT' }, JSON.stringify(local));
  }
  assert.deepEqual(vaultInstallPlan({ ...custom, id: 'openai' }, {}), { kind: 'unsupported', code: 'ID_CONFLICT' });
  assert.deepEqual(vaultInstallPlan({ ...custom, protocol: 'other' }, {}), { kind: 'unsupported', code: 'UNSUPPORTED_PROVIDER' });
  assert.deepEqual(vaultInstallPlan({ ...custom, protocol: 'other', desktopType: 'bedrock-claude' }, {}).kind, 'custom');
  assert.deepEqual(vaultInstallPlan({ ...custom, defaultModel: null }, {}), { kind: 'unsupported', code: 'NO_MODELS' });
});

test('vault IPC commands are validated', () => {
  assert.deepEqual(parseProviderVaultCommand({ action: 'list' }), { action: 'list' });
  assert.deepEqual(parseProviderVaultCommand({ action: 'pull', ids: ['a', 'a', 'b'] }), { action: 'pull', ids: ['a', 'b'] });
  assert.deepEqual(parseProviderVaultCommand({ action: 'remove', id: 'a', expectedRevision: 2 }), { action: 'remove', id: 'a', expectedRevision: 2 });
  for (const input of [null, [], { action: 'export' }, { action: 'pull', ids: [] }, { action: 'pull', ids: ['A'] }, { action: 'pull', ids: Array.from({ length: 21 }, (_, i) => `p${i}`) },
    { action: 'remove', id: '../x' }, { action: 'remove', id: 'a', expectedRevision: -1 }]) assert.throws(() => parseProviderVaultCommand(input), String(JSON.stringify(input)));
});

function hostHarness(options: { recent?: boolean; uid?: string | null } = {}) {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-provider-vault-'));
  const handlers = new Map<string, (...args: any[]) => any>();
  const ipc = { handle: (name: string, handler: (...args: any[]) => any) => handlers.set(name, handler), removeHandler: (name: string) => handlers.delete(name) };
  const applied: unknown[] = [];
  const host = new HostController(new SettingsStore(root), { registerIpc() {}, registerWindow() {}, runtimeSummaries: [], hasActiveWork: () => false, applyProviderSettings: async () => { applied.push(true); } } as any, new DiagnosticBuffer(), undefined, ipc as any);
  const calls: Array<{ method: string; args: unknown[] }> = [];
  const vault = {
    list: async (...args: unknown[]) => { calls.push({ method: 'list', args }); return [provider()]; },
    upsert: async (...args: unknown[]) => { calls.push({ method: 'upsert', args }); if ((args[1] as { id: string }).id === 'broken') throw new ProviderVaultError('PROVIDER_LIMIT', 409); return provider(); },
    remove: async (...args: unknown[]) => { calls.push({ method: 'remove', args }); },
    exportKeys: async (...args: unknown[]) => { calls.push({ method: 'exportKeys', args }); return [{ ...provider(), apiKey: 'sk-from-web' }, { ...provider({ id: 'gw', presetId: null, label: 'Gateway', baseUrl: 'https://gw.example.com', models: [{ id: 'm' }], defaultModel: 'm' }), apiKey: 'sk-gw' }]; },
  };
  const uid = options.uid === undefined ? 'u1' : options.uid;
  host.attachAccount({ snapshot: () => ({ user: uid ? { uid } : null }), sessionEpoch: () => 0, getIdToken: async () => 't', recentlyAuthenticated: () => options.recent ?? true,
    execute: async () => ({}), onChanged: () => () => undefined, dispose() {} } as any, vault as any);
  const settings = { raw: { providers: {} as Record<string, unknown> }, effective: { providers: {} as Record<string, unknown> }, locked: [] };
  const settingWrites: unknown[] = [];
  (host as any).sessionCatalog = { providerSettings: async (patch?: { providers: Record<string, unknown> }, expected?: unknown) => {
    if (patch) { settingWrites.push({ patch, expected }); settings.raw.providers = patch.providers; settings.effective.providers = patch.providers; }
    return structuredClone(settings);
  } };
  const accountCommands: Array<{ command: any; credential?: string }> = [];
  let snapshot = { storeScope: 's', configRevision: 4, accounts: [] as any[], health: [] };
  host.attachProviderAccounts({ cancel() {}, snapshot: async () => snapshot, command: async (command: any, credential?: string) => {
    accountCommands.push({ command, credential });
    if (command.action === 'add') snapshot = { ...snapshot, configRevision: snapshot.configRevision + 1, accounts: [...snapshot.accounts, { accountId: `a${snapshot.accounts.length}`, providerId: command.providerId, displayName: command.displayName, authKind: 'api_key' }] };
    return snapshot;
  } } as any);
  const frame = { url: 'http://127.0.0.1:4242' };
  const sender = { mainFrame: frame, isDestroyed: () => false, once() {}, removeListener() {}, send() {} };
  host.registerWindow(sender as any, frame.url);
  host.registerIpc();
  const invoke = (channel: string, ...args: unknown[]) => handlers.get(channel)!({ sender, senderFrame: frame }, ...args);
  return { host, invoke, calls, accountCommands, settingWrites, applied, settings,
    dispose: () => { host.dispose(); rmSync(root, { recursive: true, force: true }); } };
}

test('an add with cloudSync saves locally first, then stores the key in the vault; a vault failure keeps the local save', async () => {
  const harness = hostHarness();
  try {
    const synced = await harness.invoke(CH_PROVIDER_ACCOUNTS, { action: 'add', expectedRevision: 4, providerId: 'deepseek', executionBindingId: 'deepseek', authKind: 'api_key', displayName: 'DeepSeek', cloudSync: true }, ' sk-local ');
    assert.deepEqual(synced.cloudSync, { ok: true });
    assert.equal('cloudSync' in harness.accountCommands[0]!.command, false, 'the local registry never sees the cloud flag');
    assert.deepEqual(harness.calls.find(call => call.method === 'upsert')!.args, ['u1', { action: 'upsert', id: 'deepseek', presetId: 'deepseek', label: 'DeepSeek', apiKey: 'sk-local' }]);

    harness.settings.effective.providers.broken = { type: 'openai', baseUrl: 'https://broken.example.com', models: [{ id: 'm' }] };
    const failed = await harness.invoke(CH_PROVIDER_ACCOUNTS, { action: 'add', expectedRevision: 5, providerId: 'broken', executionBindingId: 'broken', authKind: 'api_key', displayName: 'broken', cloudSync: true }, 'sk-2');
    assert.deepEqual(failed.cloudSync, { ok: false, code: 'PROVIDER_LIMIT' });
    assert.equal(failed.accounts.length, 2);

    const plain = await harness.invoke(CH_PROVIDER_ACCOUNTS, { action: 'add', expectedRevision: 6, providerId: 'deepseek', executionBindingId: 'deepseek', authKind: 'api_key', displayName: 'Second' }, 'sk-3');
    assert.equal(plain.cloudSync, undefined);
    assert.equal(harness.calls.filter(call => call.method === 'upsert').length, 2);
    await assert.rejects(async () => harness.invoke(CH_PROVIDER_ACCOUNTS, { action: 'test', expectedRevision: 6, accountId: 'a0', cloudSync: true }), /Invalid account operation/);
  } finally { harness.dispose(); }
});

test('a pull asks for a fresh sign-in, then installs built-in and custom providers without the key reaching the renderer', async () => {
  const stale = hostHarness({ recent: false });
  try {
    assert.deepEqual(await stale.invoke(CH_PROVIDER_VAULT, { action: 'pull', ids: ['deepseek'] }), { ok: false, code: 'REAUTH_REQUIRED' });
    assert.equal(stale.calls.length, 0, 'no export is spent on a stale session');
  } finally { stale.dispose(); }
  const signedOut = hostHarness({ uid: null });
  try { assert.deepEqual(await signedOut.invoke(CH_PROVIDER_VAULT, { action: 'list' }), { ok: false, code: 'SIGN_IN_REQUIRED' }); } finally { signedOut.dispose(); }

  const harness = hostHarness();
  try {
    const reply = await harness.invoke(CH_PROVIDER_VAULT, { action: 'pull', ids: ['deepseek', 'gw', 'gone'] });
    assert.ok(reply.ok);
    assert.deepEqual(reply.pulled, [{ id: 'deepseek', ok: true, localProviderId: 'deepseek' }, { id: 'gw', ok: true, localProviderId: 'gw' }, { id: 'gone', ok: false, code: 'PROVIDER_NOT_FOUND' }]);
    assert.ok(!JSON.stringify(reply).includes('sk-'), 'keys stay in the main process');
    assert.deepEqual(harness.settingWrites, [{ patch: { providers: { gw: { type: 'openai', baseUrl: 'https://gw.example.com', models: [{ id: 'm' }] } } }, expected: { providers: {} } }]);
    assert.equal(harness.applied.length, 1);
    assert.deepEqual(harness.accountCommands.map(({ command, credential }) => [command.action, command.providerId, command.displayName, credential]),
      [['add', 'deepseek', 'DeepSeek', 'sk-from-web'], ['add', 'gw', 'Gateway', 'sk-gw']]);

    // Pulling again replaces the keys of the accounts the first pull added.
    await harness.invoke(CH_PROVIDER_VAULT, { action: 'pull', ids: ['deepseek', 'gw'] });
    assert.deepEqual(harness.accountCommands.slice(2).map(({ command }) => [command.action, command.accountId]), [['reconnect', 'a0'], ['reconnect', 'a1']]);
    assert.equal(harness.settingWrites.length, 1, 'an existing custom provider is left as it is');

    assert.ok((await harness.invoke(CH_PROVIDER_VAULT, { action: 'remove', id: 'gw', expectedRevision: 1 })).ok);
    assert.deepEqual(harness.calls.find(call => call.method === 'remove')!.args, ['u1', 'gw', 1]);
    await assert.rejects(async () => harness.invoke(CH_PROVIDER_VAULT, { action: 'export' }), /Invalid provider vault command/);
  } finally { harness.dispose(); }
});

test('the renderer names vault codes in words and remembers the sync choice on this Mac only', async () => {
  const { readVaultSyncPreference, vaultCodeText, writeVaultSyncPreference } = await import('../src/renderer/components/settings/pages/CloudProviders');
  assert.equal(vaultCodeText('PROVIDER_LIMIT'), 'your account already has 20 model services');
  assert.equal(vaultCodeText('NETWORK'), 'check your network and try again');
  assert.equal(vaultCodeText('SOMETHING_NEW'), 'SOMETHING_NEW');
  const values = new Map<string, string>();
  const previous = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: { getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => values.set(key, value) } });
  try {
    assert.equal(readVaultSyncPreference(), false, 'off until chosen');
    writeVaultSyncPreference(true);
    assert.equal(readVaultSyncPreference(), true);
    Object.defineProperty(globalThis, 'localStorage', { configurable: true, get() { throw new Error('blocked'); } });
    assert.equal(readVaultSyncPreference(), false);
    writeVaultSyncPreference(true);
  } finally {
    if (previous) Object.defineProperty(globalThis, 'localStorage', previous); else delete (globalThis as { localStorage?: unknown }).localStorage;
  }
});
