import { test } from 'node:test';
import assert from 'node:assert/strict';
import { systemClock } from '@lingxi/bridge-client/remote';
import { RemoteCommands } from '../src/main/remoteControl/remoteCommands.ts';
import type { DesktopProviderAccounts } from '../src/main/provider-accounts.ts';
import type { ProviderAccountCommand } from '../src/shared/providerAccounts.ts';

const owner = { account_uid: 'owner', host_id: 'host', device_id: 'phone', device_name: 'Phone', conn_id: 'connection', platform: 'ios' as const };
function fixture() {
  const calls: Array<[ProviderAccountCommand, string | undefined]> = [];
  let allowed = true;
  let failure = false;
  const commands = new RemoteCommands({ allowAlwaysAllow: () => false, echo: () => {}, clock: systemClock,
    authorizeProviderAccounts: viewer => allowed && viewer.account_uid === 'owner',
    providerAccounts: { snapshot: async () => ({ accounts: [{ accountId: 'a', authKind: 'api_key' }] }), command: async (request: ProviderAccountCommand, secret?: string) => {
      calls.push([request, secret]); if (failure) throw new Error('private-key-secret');
      return { storeScope: 'host-account-domain', configRevision: 1, accounts: [], health: [] };
    } } as unknown as DesktopProviderAccounts });
  return { commands, calls, revoke() { allowed = false; }, fail() { failure = true; } };
}

test('provider account owner RPC targets the Host catalog and keeps secrets in the dedicated argument', async () => {
  const f = fixture();
  const response = await f.commands.providerAccounts(owner, { request_json: JSON.stringify({ action: 'add', expectedRevision: 0, providerId: 'openai', executionBindingId: 'openai', authKind: 'api_key', displayName: 'A' }), credential: 'private-key-secret' });
  assert.equal(f.calls[0][1], 'private-key-secret');
  assert.equal(JSON.stringify(f.calls[0][0]).includes('private-key-secret'), false);
  assert.equal(response.snapshot_json.includes('private-key-secret'), false);
  assert.equal(JSON.parse(response.snapshot_json).storeScope, 'host-account-domain');
});

test('unapproved and revoked owners cannot mutate or list the Host account catalog', async () => {
  const f = fixture();
  const request = { request_json: JSON.stringify({ action: 'list', expectedRevision: 0 }) };
  await assert.rejects(f.commands.providerAccounts({ ...owner, account_uid: 'different' }, request), /authenticated Host owner/);
  f.revoke();
  await assert.rejects(f.commands.providerAccounts(owner, request), /authenticated Host owner/);
  assert.deepEqual(f.calls, []);
});

test('remote account requests reject unsupported OAuth ownership and generic commands', async () => {
  const f = fixture();
  for (const request of [ { action: 'begin_oauth', expectedRevision: 0 }, { action: 'add', expectedRevision: 0, authKind: 'oauth' }, { action: 'send_prompt', expectedRevision: 0 }, { action: 'publish_versions', expectedRevision: 0 }, { action: 'publish_credential_version', expectedRevision: 0 } ]) {
    await assert.rejects(f.commands.providerAccounts(owner, { request_json: JSON.stringify(request) }));
  }
  assert.deepEqual(f.calls, []);
});

test('canonical remote enable/key replacement actions use the same scoped Desktop lifecycle service', async () => {
  const f = fixture();
  await f.commands.providerAccounts(owner, { request_json: JSON.stringify({ action: 'set_enabled', expectedRevision: 3, accountId: 'a', enabled: false }) });
  await f.commands.providerAccounts(owner, { request_json: JSON.stringify({ action: 'set_credential', expectedRevision: 4, accountId: 'a' }), credential: 'replacement' });
  assert.equal(f.calls[0][0].action, 'enable');
  assert.equal(f.calls[1][0].action, 'reconnect');
  assert.equal(f.calls[1][0].accountId, 'a');
});

test('account RPC errors never echo helper diagnostics or the credential', async () => {
  const f = fixture(); f.fail();
  await assert.rejects(f.commands.providerAccounts(owner, { request_json: JSON.stringify({ action: 'list', expectedRevision: 0 }), credential: 'private-key-secret' }), error => !String(error).includes('private-key-secret'));
});
