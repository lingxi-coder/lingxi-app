import { test } from 'node:test';
import assert from 'node:assert/strict';
import { providerAccountSummary } from '../src/shared/providerAccounts';
import { fixtureAccountSnapshot } from './fixtures/provider-accounts-mock';

test('configuration summaries group OAuth and API key accounts without claiming model permissions', () => {
  const snapshot = fixtureAccountSnapshot(['openai-chatgpt', 'openai', 'anthropic']);
  snapshot.modelEntitlements = [{ accountId: snapshot.accounts[0].accountId, identityGeneration: 1, modelId: 'restricted', status: 'forbidden', observedAtMs: 1 }];
  snapshot.health[1].status = 'needs_auth';
  assert.deepEqual(providerAccountSummary(snapshot, 'openai'), { status: 'configured', total: 2, enabled: 2 });
  assert.deepEqual(providerAccountSummary(snapshot, 'openai-chatgpt'), providerAccountSummary(snapshot, 'openai'));
  assert.deepEqual(providerAccountSummary(snapshot, 'deepseek'), { status: 'unconfigured', total: 0, enabled: 0 });
  snapshot.accounts[0].enabled = false;
  assert.deepEqual(providerAccountSummary(snapshot, 'openai'), { status: 'needs_auth', total: 2, enabled: 1 });
  snapshot.accounts[1].enabled = false;
  assert.deepEqual(providerAccountSummary(snapshot, 'openai'), { status: 'disabled', total: 2, enabled: 0 });
});

test('unknown and stale identity health cannot appear configured or unconfigured', () => {
  const snapshot = fixtureAccountSnapshot(['openai-chatgpt']);
  snapshot.accounts[0].identityGeneration++;
  assert.equal(providerAccountSummary(snapshot, 'openai').status, 'unverified');
  snapshot.health = [];
  assert.equal(providerAccountSummary(snapshot, 'openai').status, 'unverified');
});

test('limits and storage failures remain configured accounts needing attention', () => {
  const snapshot = fixtureAccountSnapshot(['openai-chatgpt', 'openai']);
  snapshot.health[0].status = 'cooling_down'; snapshot.health[1].status = 'quota_exhausted';
  assert.deepEqual(providerAccountSummary(snapshot, 'openai'), { status: 'temporarily_unavailable', total: 2, enabled: 2 });
  snapshot.health[1].status = 'storage_unavailable';
  assert.equal(providerAccountSummary(snapshot, 'openai').status, 'storage_unavailable');
  snapshot.health[1].status = 'ready';
  assert.equal(providerAccountSummary(snapshot, 'openai').status, 'configured');
});
