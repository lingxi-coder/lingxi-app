import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  parseModelsInput,
  providersFromSnapshot,
  routingFromSnapshot,
  validateCustomProvider,
} from '../src/renderer/components/settings/pages/CustomProviders';
import {
  apiBaseUrlPatch,
  connectButtonLabel,
  credentialStatusKind,
  initialProviderSelection,
} from '../src/renderer/components/settings/pages/ProviderCredentials';

// ---------------------------------------------------------------------------
// CustomProviders: the write-time gate this task exists for.
// ---------------------------------------------------------------------------

test('a custom provider with no models is refused before it reaches disk', () => {
  const error = validateCustomProvider({ type: 'openai', baseUrl: 'https://x', models: [] });
  assert.ok(error, 'an empty models list must be refused');
  assert.match(
    error ?? '', /models/,
    'the message must name `models` — an absent or empty list is an engine-startup error',
  );
});

test('a custom provider with a model passes (the A/B for the test above)', () => {
  assert.equal(
    validateCustomProvider({ type: 'openai', baseUrl: 'https://x', models: [{ id: 'gpt-x' }] }),
    null,
    'if this also failed, the refusal test above would prove nothing',
  );
});

test('an unsupported provider type is refused and the message lists the supported set', () => {
  const error = validateCustomProvider({ type: 'gopher', baseUrl: 'https://x', models: [{ id: 'm' }] });
  assert.match(error ?? '', /openai/);
  assert.match(error ?? '', /anthropic/);
});

test('a model entry with a blank id is refused even though the list is non-empty', () => {
  const error = validateCustomProvider({ type: 'anthropic', models: [{ id: '  ' }] });
  assert.match(error ?? '', /id/);
});

test('every one of the nine engine-supported provider types passes validation with a model', () => {
  for (const type of ['openai', 'openai-responses', 'anthropic', 'gemini', 'azure-openai', 'bedrock-claude', 'vertex-claude', 'vertex-gemini', 'foundry-claude']) {
    assert.equal(validateCustomProvider({ type, models: [{ id: 'm' }] }), null, `${type} should be accepted`);
  }
});

test('models input parses comma- and newline-separated ids, dropping blanks', () => {
  assert.deepEqual(parseModelsInput('gpt-x, gpt-y,\n , gpt-z'), [{ id: 'gpt-x' }, { id: 'gpt-y' }, { id: 'gpt-z' }]);
  assert.deepEqual(parseModelsInput(''), []);
  assert.deepEqual(parseModelsInput('   '), []);
});

test('providersFromSnapshot reads the effective view and is never a throw', () => {
  assert.deepEqual(providersFromSnapshot(null), {});
  assert.deepEqual(providersFromSnapshot({ effective: {}, active: {}, provenance: {}, files: [], locked: [] }), {});
  const withProviders = {
    effective: { providers: { mine: { type: 'openai', models: [{ id: 'm' }] } } },
    active: {}, provenance: {}, files: [], locked: [],
  };
  assert.deepEqual(providersFromSnapshot(withProviders), { mine: { type: 'openai', models: [{ id: 'm' }] } });
});

test('routingFromSnapshot reads the effective view and is never a throw', () => {
  assert.deepEqual(routingFromSnapshot(null), {});
  const withRouting = {
    effective: { routing: { retry: { maxAttempts: 3, backoffMs: 500 } } },
    active: {}, provenance: {}, files: [], locked: [],
  };
  assert.deepEqual(routingFromSnapshot(withRouting), { retry: { maxAttempts: 3, backoffMs: 500 } });
});

// ---------------------------------------------------------------------------
// ProviderCredentials: pure logic lifted (or, for apiBaseUrl, newly added —
// see this page's module doc) from `BetaSettings`'s Providers section.
// ---------------------------------------------------------------------------

test('the deep link selects the requested provider when it exists, else falls back to anthropic', () => {
  assert.equal(initialProviderSelection('deepseek'), 'deepseek');
  assert.equal(initialProviderSelection('not-a-real-provider'), 'anthropic');
  assert.equal(initialProviderSelection(undefined), 'anthropic');
});

test('credential status: runtimeOnly wins over every other condition', () => {
  assert.equal(credentialStatusKind({ configured: true, encryptionAvailable: true, runtimeOnly: true }), 'runtime');
  assert.equal(credentialStatusKind({ configured: false, encryptionAvailable: false, runtimeOnly: true }), 'runtime');
});

test('credential status: configured + encrypted is the securely-persisted state', () => {
  assert.equal(credentialStatusKind({ configured: true, encryptionAvailable: true }), 'secure');
});

test('credential status: configured without Keychain encryption is the fallback-configured warning', () => {
  assert.equal(credentialStatusKind({ configured: true, encryptionAvailable: false }), 'fallback-configured');
});

test('credential status: unconfigured with no Keychain warns that connecting will use the fallback', () => {
  assert.equal(credentialStatusKind({ configured: false, encryptionAvailable: false }), 'fallback-unconfigured');
});

test('credential status: unconfigured with Keychain available (or unknown) has nothing to report', () => {
  assert.equal(credentialStatusKind({ configured: false, encryptionAvailable: true }), 'none');
  assert.equal(credentialStatusKind(undefined), 'none');
});

test('connect button label follows the busy > runtimeOnly > configured priority BetaSettings used', () => {
  assert.equal(connectButtonLabel({ connecting: true, modelApplying: false }), '连接中…');
  assert.equal(connectButtonLabel({ connecting: false, modelApplying: true }), '应用模型中…');
  assert.equal(connectButtonLabel({ connecting: false, modelApplying: false, runtimeOnly: true }), '使用输入的密钥');
  assert.equal(connectButtonLabel({ connecting: false, modelApplying: false, configured: true }), '替换');
  assert.equal(connectButtonLabel({ connecting: false, modelApplying: false }), '连接');
  // Busy wins even when the provider is also configured/runtimeOnly — a
  // mid-transaction relabel to "connect" would invite a second click.
  assert.equal(connectButtonLabel({ connecting: true, modelApplying: false, configured: true, runtimeOnly: true }), '连接中…');
});

test('apiBaseUrlPatch clears the override on blank input rather than persisting an empty string', () => {
  assert.equal(apiBaseUrlPatch('  '), null);
  assert.equal(apiBaseUrlPatch(''), null);
  assert.equal(apiBaseUrlPatch('  https://example.test  '), 'https://example.test');
});
