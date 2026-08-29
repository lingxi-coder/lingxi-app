import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  isEditableLayer,
  parseModelsInput,
  providersFromLayer,
  routingFromLayer,
  validateCustomProvider,
} from '../src/renderer/components/settings/pages/CustomProviders';
import {
  apiBaseUrlPatch,
  connectButtonLabel,
  credentialStatusKind,
  initialProviderSelection,
} from '../src/renderer/components/settings/pages/ProviderCredentials';
import { rowState, type SettingsSnapshot } from '../src/renderer/components/settings/useEngineSettings';

/**
 * A fully-typed `SettingsSnapshot`, the same helper every other new settings
 * test file in this branch uses. The fixtures here used to be bare object
 * literals, and they had already drifted: all six were missing `mergedKeys`,
 * which `providersFromLayer`/`routingFromLayer` never read — but `rowState`,
 * handed the same fixture, throws on `snapshot.mergedKeys.includes(key)`.
 * `clients/electron/test/` is not part of `npm run typecheck`, so nothing
 * said so.
 */
function snap(overrides: Partial<SettingsSnapshot> = {}): SettingsSnapshot {
  return {
    files: [],
    effective: {},
    active: {},
    provenance: {},
    locked: [],
    layers: {},
    mergedKeys: [],
    ...overrides,
  };
}

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

test('providersFromLayer reads the SELECTED LAYER\'s own map, and is never a throw', () => {
  assert.deepEqual(providersFromLayer(null, 'user'), {});
  assert.deepEqual(
    providersFromLayer(snap(), 'user'),
    {},
  );
  const snapshot = snap({
    layers: {
      user: { providers: { userOnly: { type: 'openai', models: [{ id: 'm-user' }] } } },
      local: { providers: { localOnly: { type: 'openai', models: [{ id: 'm-local' }] } } },
    },
  });
  assert.deepEqual(providersFromLayer(snapshot, 'user'), { userOnly: { type: 'openai', models: [{ id: 'm-user' }] } });
  assert.deepEqual(providersFromLayer(snapshot, 'local'), { localOnly: { type: 'openai', models: [{ id: 'm-local' }] } });
});

// Fix round 1: this is the regression test for the Important finding. Before
// the fix, this page read `snapshot.effective['providers']` — the
// cross-layer MERGED view — so a write based on it would silently copy
// whichever layer won the merge into whichever layer the user meant to save.
// `providersFromLayer` must give `project` NONE of `local`'s entries even
// though `local` would win an `effective` merge.
test('providersFromLayer never leaks another layer\'s entries into the selected layer (fix round 1 regression)', () => {
  const snapshot = snap({
    effective: { providers: { localOnly: { type: 'openai', models: [{ id: 'm-local' }] } } },
    provenance: { providers: 'local' },
    layers: {
      user: { providers: { userOnly: { type: 'openai', models: [{ id: 'm-user' }] } } },
      project: {},
      local: { providers: { localOnly: { type: 'openai', models: [{ id: 'm-local' }] } } },
    },
  });
  assert.deepEqual(
    providersFromLayer(snapshot, 'project'),
    {},
    'project never set `providers`, so it must read empty, not local\'s merged-in value',
  );
  assert.deepEqual(
    providersFromLayer(snapshot, 'user'),
    { userOnly: { type: 'openai', models: [{ id: 'm-user' }] } },
    'user must see only its own entry, not local\'s',
  );
});

test('routingFromLayer reads the SELECTED LAYER\'s own map, and is never a throw', () => {
  assert.deepEqual(routingFromLayer(null, 'project'), {});
  const snapshot = snap({
    layers: {
      project: { routing: { retry: { maxAttempts: 3, backoffMs: 500 } } },
    },
  });
  assert.deepEqual(routingFromLayer(snapshot, 'project'), { retry: { maxAttempts: 3, backoffMs: 500 } });
  assert.deepEqual(routingFromLayer(snapshot, 'user'), {}, 'user never set `routing`, so it must read empty');
});

test('isEditableLayer accepts exactly the three layer-switcher tabs', () => {
  assert.equal(isEditableLayer('user'), true);
  assert.equal(isEditableLayer('project'), true);
  assert.equal(isEditableLayer('local'), true);
  assert.equal(isEditableLayer('managed'), false);
  assert.equal(isEditableLayer('device'), false);
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

// The runtime half of the same finding: `clients/electron/test/` is not part
// of `npm run typecheck`, so a fixture that is not really a
// `SettingsSnapshot` only shows up when something reads the field it is
// missing. `rowState` is that something — it is the reader every other
// settings page hands a snapshot to, and it throws outright on
// `snapshot.mergedKeys.includes(key)`.

test('the fixtures in this file are real SettingsSnapshots — rowState can read one', () => {
  assert.deepEqual(
    rowState(snap({ provenance: { providers: 'user' } }), 'providers', 'user'),
    { kind: 'set-here' },
    'an untyped fixture missing mergedKeys throws here rather than failing an assertion',
  );
});
