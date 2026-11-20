import assert from 'node:assert/strict';
import { createServer, type IncomingHttpHeaders } from 'node:http';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { ProviderVaultClient, ProviderVaultError, vaultIdForAccount, vaultInstallPlan, vaultUpsertFor } from '../src/main/provider-vault';
import { ProviderVaultLinks } from '../src/main/provider-vault-links';
import { CH_PROVIDER_VAULT, CH_PROVIDER_VAULT_SYNCED, VAULT_PRESET_IDS, parseProviderVaultCommand } from '../src/shared/providerVault';
import { CH_PROVIDER_ACCOUNTS } from '../src/shared/providerAccounts';
import { HostController } from '../src/main/host';
import { DiagnosticBuffer } from '../src/main/host-utils';
import { SettingsStore } from '../src/main/settings';
import { PROVIDERS } from '../src/shared/providers';
import type { VaultProvider } from '../../../packages/llm-vault/contracts';

const provider = (over: Partial<VaultProvider> = {}): VaultProvider => ({
  id: 'deepseek', presetId: 'deepseek', label: 'DeepSeek', protocol: 'open_ai_chat', desktopType: null, baseUrl: 'https://api.deepseek.com',
  models: [{ id: 'deepseek-flash' }], defaultModel: 'deepseek-flash', apiVersion: null, region: null, keyHint: 'ab12', source: 'web', revision: 1,
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

/** Built-in providers as the runtime catalog lists them: the static ones plus those only the catalog has. */
const builtin = (id: string) => PROVIDERS.some(entry => entry.id === id) || id === 'qwen' || id === 'minimax';
const mine = (providerId: string, displayName = providerId, vaultId = providerId) => ({ vaultId, providerId, displayName });

test('a key saved on this Mac maps to a vault record: built-in presets by id, custom providers by their protocol', () => {
  assert.deepEqual(vaultUpsertFor(mine('deepseek', 'DeepSeek'), builtin, undefined, 'sk-1'), { action: 'upsert', id: 'deepseek', presetId: 'deepseek', label: 'DeepSeek', apiKey: 'sk-1' });
  // A second account of the provider has its own record, under its own name.
  assert.deepEqual(vaultUpsertFor(mine('deepseek', 'DeepSeek (work)', 'deepseek.a1'), builtin, undefined, 'sk-2'), { action: 'upsert', id: 'deepseek.a1', presetId: 'deepseek', label: 'DeepSeek (work)', apiKey: 'sk-2' });
  // The Desktop default model is an engine alias ('deepseek/deepseek-flash'); the preset carries the wire id instead.
  assert.equal(vaultUpsertFor(mine('deepseek'), builtin, undefined, 'sk-1')?.defaultModel, undefined);
  assert.equal(vaultUpsertFor(mine('github-copilot'), builtin, undefined, 'token'), null);
  // Qwen and MiniMax are built in through the runtime catalog only: they sync as presets, not as custom providers.
  for (const id of ['qwen', 'minimax']) assert.equal(vaultUpsertFor(mine(id), builtin, undefined, 'k')?.presetId, id);
  assert.deepEqual(vaultUpsertFor(mine('my-proxy'), builtin, { type: 'openai', baseUrl: ' https://llm.example.com/v1 ', models: [{ id: 'm1 ' }, 'm2', { id: 'm1' }] }, 'sk-2'), {
    action: 'upsert', id: 'my-proxy', label: 'my-proxy', protocol: 'open_ai_chat', desktopType: 'openai', baseUrl: 'https://llm.example.com/v1',
    models: [{ id: 'm1' }, { id: 'm2' }], defaultModel: 'm1', apiKey: 'sk-2',
  });
  assert.equal('apiKey' in vaultUpsertFor(mine('my-proxy'), builtin, { type: 'openai', baseUrl: 'https://llm.example.com/v1', models: [] })!, false, 'a change without a key keeps the stored one');
  // What Desktop needs to call Azure and Bedrock again travels with the record.
  assert.deepEqual(vaultUpsertFor(mine('my-azure'), builtin, { type: 'azure-openai', baseUrl: 'https://x.openai.azure.com', apiVersion: '2024-10-21', models: [] }, 'k'),
    { action: 'upsert', id: 'my-azure', label: 'my-azure', protocol: 'other', desktopType: 'azure-openai', baseUrl: 'https://x.openai.azure.com', models: [], apiVersion: '2024-10-21', apiKey: 'k' });
  assert.equal(vaultUpsertFor(mine('my-azure'), builtin, { type: 'azure-openai', baseUrl: 'https://x.openai.azure.com', models: [] }, 'k'), null, 'Azure without its API version');
  assert.deepEqual(vaultUpsertFor(mine('br'), builtin, { type: 'bedrock-claude', region: 'us-east-1', models: [{ id: 'claude' }] }, 'k'),
    { action: 'upsert', id: 'br', label: 'br', protocol: 'other', desktopType: 'bedrock-claude', baseUrl: 'https://bedrock-runtime.us-east-1.amazonaws.com', models: [{ id: 'claude' }], defaultModel: 'claude', region: 'us-east-1', apiKey: 'k' });
  assert.equal(vaultUpsertFor(mine('br'), builtin, { type: 'bedrock-claude', models: [] }, 'k'), null, 'Bedrock without its region');
  // An endpoint the vault refuses is refused here, without a round trip.
  for (const baseUrl of ['http://localhost:11434/v1', 'https://10.0.0.5/v1', 'https://gw.corp.internal', 'https://gw.example.com:8443', 'https://gw.example.com/v1?x=1', 'https://intranet'])
    assert.equal(vaultUpsertFor(mine('local'), builtin, { type: 'openai', baseUrl, models: [{ id: 'llama' }] }, 'k'), null, baseUrl);
  assert.equal(vaultUpsertFor(mine('odd'), builtin, { type: 'mystery', baseUrl: 'https://x.example.com' }, 'k'), null);
  for (const id of VAULT_PRESET_IDS) if (PROVIDERS.some(entry => entry.id === id)) assert.ok(vaultUpsertFor(mine(id), builtin, undefined, 'k'), id);
});

test('each account of this Mac has its own vault record, and keeps it', () => {
  const nobody = () => false;
  assert.equal(vaultIdForAccount('deepseek', 'acct-1', undefined, nobody), 'deepseek', 'the first account takes the provider id');
  assert.equal(vaultIdForAccount('deepseek', 'Acct-2_XY', undefined, id => id === 'deepseek'), 'deepseek.acct2xy', 'a second gets one of its own');
  assert.equal(vaultIdForAccount('deepseek', 'acct-2', 'deepseek.old', nobody), 'deepseek.old', 'a linked account keeps its record');
  assert.equal(vaultIdForAccount('x'.repeat(64), 'abcdefghij', undefined, () => true), `${'x'.repeat(55)}.abcdefgh`);
  assert.equal(vaultIdForAccount('deepseek', '--', undefined, () => true), null);
  const links = new ProviderVaultLinks();
  links.link('u1', 'deepseek', 'a0'); links.link('u1', 'deepseek.a1', 'a1'); links.link('u2', 'deepseek', 'b0');
  assert.equal(links.accountFor('u1', 'deepseek'), 'a0'); assert.equal(links.vaultIdFor('u1', 'a1'), 'deepseek.a1');
  links.link('u1', 'deepseek', 'a1');
  assert.equal(links.vaultIdFor('u1', 'a0'), undefined, 'a record has one account and an account one record');
  assert.equal(links.accountFor('u1', 'deepseek.a1'), undefined);
  links.prune('u1', []);
  assert.equal(links.accountFor('u1', 'deepseek'), undefined);
  assert.equal(links.accountFor('u2', 'deepseek'), 'b0', 'another LingXi account keeps its links');
  const root = mkdtempSync(join(tmpdir(), 'lingxi-vault-links-'));
  try {
    new ProviderVaultLinks(root).link('u1', 'gw', 'a3');
    assert.equal(new ProviderVaultLinks(root).accountFor('u1', 'gw'), 'a3', 'links survive a restart');
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('a vault provider lands on this Mac as a built-in account or a custom provider', () => {
  assert.deepEqual(vaultInstallPlan(provider(), {}, builtin), { kind: 'builtin', providerId: 'deepseek', displayName: 'DeepSeek' });
  assert.deepEqual(vaultInstallPlan(provider({ id: 'deepseek-work', label: 'DeepSeek (work)' }), {}, builtin), { kind: 'builtin', providerId: 'deepseek', displayName: 'DeepSeek (work)' });
  // Qwen is built in through the runtime catalog: never a settings entry that would replace the built-in preset.
  const qwen = provider({ id: 'qwen', presetId: 'qwen', label: 'Qwen', baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1', models: [{ id: 'a' }, { id: 'qwen-max' }], defaultModel: 'qwen-max' });
  assert.deepEqual(vaultInstallPlan(qwen, {}, builtin), { kind: 'builtin', providerId: 'qwen', displayName: 'Qwen' });
  assert.deepEqual(vaultInstallPlan(qwen, {}, id => PROVIDERS.some(entry => entry.id === id)), { kind: 'unsupported', code: 'UNSUPPORTED_PROVIDER' }, 'a runtime without it');
  const custom = provider({ id: 'gw', presetId: null, label: 'Gateway', protocol: 'anthropic_messages', baseUrl: 'https://gw.example.com', models: [], defaultModel: 'claude-x' });
  assert.deepEqual(vaultInstallPlan(custom, {}, builtin), { kind: 'custom', providerId: 'gw', displayName: 'Gateway', profile: { type: 'anthropic', baseUrl: 'https://gw.example.com', models: [{ id: 'claude-x' }] } });
  assert.deepEqual(vaultInstallPlan(custom, { gw: { type: 'anthropic', baseUrl: 'https://gw.example.com/' } }, builtin), { kind: 'custom', providerId: 'gw', displayName: 'Gateway', profile: null });
  // A provider of this Mac that only shares the name never gets the key: it would send it to its own endpoint.
  for (const local of [{ type: 'anthropic', baseUrl: 'https://other.example.com' }, { type: 'openai', baseUrl: 'https://gw.example.com' }, { type: 'anthropic' }, 'gw']) {
    assert.deepEqual(vaultInstallPlan(custom, { gw: local }, builtin), { kind: 'unsupported', code: 'ID_CONFLICT' }, JSON.stringify(local));
  }
  assert.deepEqual(vaultInstallPlan({ ...custom, id: 'openai' }, {}, builtin), { kind: 'unsupported', code: 'ID_CONFLICT' });
  assert.deepEqual(vaultInstallPlan({ ...custom, protocol: 'other' }, {}, builtin), { kind: 'unsupported', code: 'UNSUPPORTED_PROVIDER' });
  // Azure and Bedrock come back with what Desktop needs to call them, or not at all.
  const bedrock = { ...custom, protocol: 'other' as const, desktopType: 'bedrock-claude' as const, baseUrl: 'https://bedrock-runtime.us-east-1.amazonaws.com' };
  assert.deepEqual(vaultInstallPlan(bedrock, {}, builtin), { kind: 'unsupported', code: 'PROVIDER_INCOMPLETE' });
  assert.deepEqual(vaultInstallPlan({ ...bedrock, region: 'us-east-1' }, {}, builtin), { kind: 'custom', providerId: 'gw', displayName: 'Gateway', profile: { type: 'bedrock-claude', baseUrl: 'https://bedrock-runtime.us-east-1.amazonaws.com', models: [{ id: 'claude-x' }], region: 'us-east-1' } });
  assert.deepEqual(vaultInstallPlan({ ...bedrock, region: 'us-east-1' }, { gw: { type: 'bedrock-claude', region: 'us-east-1' } }, builtin).kind, 'custom', 'a region-only Bedrock provider is the same endpoint');
  const azure = { ...custom, protocol: 'other' as const, desktopType: 'azure-openai' as const, baseUrl: 'https://x.openai.azure.com', apiVersion: '2024-10-21' };
  assert.deepEqual(vaultInstallPlan(azure, { gw: { type: 'azure-openai', baseUrl: 'https://x.openai.azure.com', apiVersion: '2024-02-01' } }, builtin), { kind: 'unsupported', code: 'ID_CONFLICT' });
  assert.deepEqual(vaultInstallPlan({ ...custom, defaultModel: null }, {}, builtin), { kind: 'unsupported', code: 'NO_MODELS' });
});

test('vault IPC commands are validated', () => {
  assert.deepEqual(parseProviderVaultCommand({ action: 'list' }), { action: 'list' });
  assert.deepEqual(parseProviderVaultCommand({ action: 'pull', ids: ['a', 'a', 'b'] }), { action: 'pull', ids: ['a', 'b'] });
  assert.deepEqual(parseProviderVaultCommand({ action: 'remove', id: 'a', expectedRevision: 2 }), { action: 'remove', id: 'a', expectedRevision: 2 });
  for (const input of [null, [], { action: 'export' }, { action: 'pull', ids: [] }, { action: 'pull', ids: ['A'] }, { action: 'pull', ids: Array.from({ length: 21 }, (_, i) => `p${i}`) },
    { action: 'remove', id: '../x' }, { action: 'remove', id: 'a', expectedRevision: -1 }]) assert.throws(() => parseProviderVaultCommand(input), String(JSON.stringify(input)));
});

function hostHarness(options: { recent?: boolean; uid?: string | null; listFails?: boolean; accounts?: any[] } = {}) {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-provider-vault-'));
  const handlers = new Map<string, (...args: any[]) => any>();
  const ipc = { handle: (name: string, handler: (...args: any[]) => any) => handlers.set(name, handler), removeHandler: (name: string) => handlers.delete(name) };
  const applied: unknown[] = [];
  const host = new HostController(new SettingsStore(root), { registerIpc() {}, registerWindow() {}, runtimeSummaries: [], hasActiveWork: () => false, applyProviderSettings: async () => { applied.push(true); } } as any, new DiagnosticBuffer(), undefined, ipc as any);
  const calls: Array<{ method: string; args: unknown[] }> = [];
  const vault = {
    list: async (...args: unknown[]) => { calls.push({ method: 'list', args }); if (options.listFails) throw new ProviderVaultError('NETWORK'); return [provider()]; },
    upsert: async (...args: unknown[]) => { calls.push({ method: 'upsert', args }); if ((args[1] as { id: string }).id === 'broken') throw new ProviderVaultError('PROVIDER_LIMIT', 409); return provider(); },
    remove: async (...args: unknown[]) => { calls.push({ method: 'remove', args }); },
    exportKeys: async (...args: unknown[]) => { calls.push({ method: 'exportKeys', args }); return [{ ...provider(), apiKey: 'sk-from-web' }, { ...provider({ id: 'gw', presetId: null, label: 'Gateway', baseUrl: 'https://gw.example.com', models: [{ id: 'm' }], defaultModel: 'm' }), apiKey: 'sk-gw' }]; },
  };
  const uid = options.uid === undefined ? 'u1' : options.uid;
  host.attachAccount({ snapshot: () => ({ user: uid ? { uid } : null }), sessionEpoch: () => 0, getIdToken: async () => 't', recentlyAuthenticated: () => options.recent ?? true,
    execute: async () => ({}), onChanged: () => () => undefined, dispose() {} } as any, vault as any, new ProviderVaultLinks());
  const settings = { raw: { providers: {} as Record<string, unknown> }, effective: { providers: {} as Record<string, unknown> }, locked: [] };
  const settingWrites: unknown[] = [];
  (host as any).sessionCatalog = { providerSettings: async (patch?: { providers: Record<string, unknown> }, expected?: unknown) => {
    if (patch) { settingWrites.push({ patch, expected }); settings.raw.providers = patch.providers; settings.effective.providers = patch.providers; }
    return structuredClone(settings);
  } };
  const accountCommands: Array<{ command: any; credential?: string }> = [];
  let snapshot = { storeScope: 's', configRevision: 4, accounts: [...(options.accounts ?? [])] as any[], health: [] };
  host.attachProviderAccounts({ cancel() {}, snapshot: async () => snapshot, command: async (command: any, credential?: string) => {
    accountCommands.push({ command, credential });
    if (command.action === 'add') snapshot = { ...snapshot, configRevision: snapshot.configRevision + 1, accounts: [...snapshot.accounts, { accountId: `a${snapshot.accounts.length}`, providerId: command.providerId, displayName: command.displayName, authKind: 'api_key' }] };
    return snapshot;
  } } as any);
  const frame = { url: 'http://127.0.0.1:4242' };
  const sent: Array<{ channel: string; value: any }> = [];
  const sender = { mainFrame: frame, isDestroyed: () => false, once() {}, removeListener() {}, send: (channel: string, value: unknown) => sent.push({ channel, value }) };
  host.registerWindow(sender as any, frame.url);
  host.registerIpc();
  const invoke = (channel: string, ...args: unknown[]) => handlers.get(channel)!({ sender, senderFrame: frame }, ...args);
  /** The result event of the cloud sync a reply started. */
  const synced = async (reply: { cloudSync?: { syncId: string } }) => {
    for (let i = 0; i < 50; i++) {
      const event = sent.find(entry => entry.channel === CH_PROVIDER_VAULT_SYNCED && entry.value.syncId === reply.cloudSync?.syncId);
      if (event) { const { syncId: _id, ...result } = event.value; return result; }
      await new Promise(resolve => setImmediate(resolve));
    }
    throw new Error('no sync result');
  };
  return { host, invoke, calls, accountCommands, settingWrites, applied, settings, synced,
    dispose: () => { host.dispose(); rmSync(root, { recursive: true, force: true }); } };
}

test('an add with cloudSync replies once saved locally, then stores the key in the vault; a vault failure keeps the local save', async () => {
  const harness = hostHarness();
  try {
    const first = await harness.invoke(CH_PROVIDER_ACCOUNTS, { action: 'add', expectedRevision: 4, providerId: 'deepseek', executionBindingId: 'deepseek', authKind: 'api_key', displayName: 'DeepSeek', cloudSync: true }, ' sk-local ');
    assert.match(first.cloudSync.syncId, /^[0-9a-f-]{36}$/u, 'the reply only names the sync under way');
    assert.equal(first.accounts.length, 1);
    assert.deepEqual(await harness.synced(first), { ok: true });
    assert.equal('cloudSync' in harness.accountCommands[0]!.command, false, 'the local registry never sees the cloud flag');
    const upserts = () => harness.calls.filter(call => call.method === 'upsert').map(call => call.args);
    assert.deepEqual(upserts()[0], ['u1', { action: 'upsert', id: 'deepseek', presetId: 'deepseek', label: 'DeepSeek', apiKey: 'sk-local' }]);

    // A second account of the provider gets a record of its own, under its name, instead of replacing the first one's key.
    const second = await harness.invoke(CH_PROVIDER_ACCOUNTS, { action: 'add', expectedRevision: 5, providerId: 'deepseek', executionBindingId: 'deepseek', authKind: 'api_key', displayName: 'DeepSeek (work)', cloudSync: true }, 'sk-work');
    assert.deepEqual(await harness.synced(second), { ok: true });
    assert.deepEqual(upserts()[1], ['u1', { action: 'upsert', id: 'deepseek.a1', presetId: 'deepseek', label: 'DeepSeek (work)', apiKey: 'sk-work' }]);
    // A new key for the first account goes to the first record again.
    const replaced = await harness.invoke(CH_PROVIDER_ACCOUNTS, { action: 'reconnect', expectedRevision: 6, accountId: 'a0', providerId: 'deepseek', executionBindingId: 'deepseek', authKind: 'api_key', displayName: 'DeepSeek', cloudSync: true }, 'sk-new');
    assert.deepEqual(await harness.synced(replaced), { ok: true });
    assert.equal((upserts()[2]![1] as { id: string }).id, 'deepseek');

    harness.settings.effective.providers.broken = { type: 'openai', baseUrl: 'https://broken.example.com', models: [{ id: 'm' }] };
    const failed = await harness.invoke(CH_PROVIDER_ACCOUNTS, { action: 'add', expectedRevision: 7, providerId: 'broken', executionBindingId: 'broken', authKind: 'api_key', displayName: 'broken', cloudSync: true }, 'sk-2');
    assert.equal(failed.accounts.length, 3);
    assert.deepEqual(await harness.synced(failed), { ok: false, code: 'PROVIDER_LIMIT' });

    const plain = await harness.invoke(CH_PROVIDER_ACCOUNTS, { action: 'add', expectedRevision: 8, providerId: 'deepseek', executionBindingId: 'deepseek', authKind: 'api_key', displayName: 'Third' }, 'sk-3');
    assert.equal(plain.cloudSync, undefined);
    assert.equal(upserts().length, 4);
    await assert.rejects(async () => harness.invoke(CH_PROVIDER_ACCOUNTS, { action: 'test', expectedRevision: 8, accountId: 'a0', cloudSync: true }), /Invalid account operation/);
  } finally { harness.dispose(); }
});

test('a pull never replaces the key of an account added by hand, and a list failure after it still reports the pull', async () => {
  // A hand-added DeepSeek account with the same name as the vault record.
  const harness = hostHarness({ accounts: [{ accountId: 'mine', providerId: 'deepseek', displayName: 'DeepSeek', authKind: 'api_key' }], listFails: true });
  try {
    const reply = await harness.invoke(CH_PROVIDER_VAULT, { action: 'pull', ids: ['deepseek'] });
    assert.equal(reply.ok, true);
    assert.equal('providers' in reply, false, 'the keys are in; the list that failed is left out');
    assert.deepEqual(reply.pulled[0], { id: 'deepseek', ok: true, localProviderId: 'deepseek' });
    assert.deepEqual(harness.accountCommands[0]!.command.action, 'add', 'a new account beside the hand-added one');
    assert.ok(harness.accountCommands.every(({ command }) => command.accountId !== 'mine'));
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
  assert.equal(vaultCodeText('INSTALL_FAILED'), 'it could not be added to this Mac');
  assert.equal(vaultCodeText('HTTP_503'), 'the LingXi service had a problem; try again later');
  assert.equal(vaultCodeText('SOMETHING_NEW'), 'something went wrong (SOMETHING_NEW)', 'a code without a phrase still says what failed');
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
