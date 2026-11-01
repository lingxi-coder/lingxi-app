import { fixtureAccountSnapshot } from './fixtures/provider-accounts-mock';
import { providersFromCatalog } from '../src/shared/providers';
import { test, before, after } from 'node:test';
import assert from 'node:assert/strict';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

import {
  parseModelsInput,
  withoutProviderModelPickerVisibility,
  visibleCustomProviderModelIds,
  validateCustomProvider,
} from '../src/renderer/components/settings/pages/CustomProviders';
import {
  initialProviderSelection,
  ProviderCredentials,
} from '../src/renderer/components/settings/pages/ProviderCredentials';
import { rowState, type SettingsSnapshot } from '../src/renderer/components/settings/useEngineSettings';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import { ProviderEditorFields } from '../src/renderer/components/settings/pages/ProviderEditorFields';

(globalThis as { React?: typeof React }).React = React;

// These behavior assertions intentionally exercise the Simplified Chinese UI.
// The language matrix is covered separately by advanced-settings-localization.
const originalLanguageStorage = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
before(() => Object.defineProperty(globalThis, 'localStorage', {
  configurable: true, value: { getItem: () => 'zh-Hans' },
}));
after(() => {
  if (originalLanguageStorage) Object.defineProperty(globalThis, 'localStorage', originalLanguageStorage);
  else delete (globalThis as { localStorage?: unknown }).localStorage;
});


/**
 * A fully-typed `SettingsSnapshot`, the same helper every other new settings
 * test file in this branch uses. The fixtures here used to be bare object
 * literals, and they had already drifted: all six were missing `mergedKeys`,
 * which `providersFromLayer`/`routingFromLayer` never read — but `rowState`,
 * handed the same fixture, throws on `snapshot.mergedKeys.includes(key)`.
 * `apps/electron/test/` is not part of `npm run typecheck`, so nothing
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
    assert.equal(validateCustomProvider({
      type,
      baseUrl: 'https://example.com/v1',
      apiKeyEnv: 'TEST_PROVIDER_KEY',
      ...(type === 'azure-openai' ? { apiVersion: '2024-10-21' } : {}),
      ...(type === 'bedrock-claude' ? { region: 'us-east-1' } : {}),
      models: [{ id: 'm' }],
    }), null, `${type} should be accepted`);
  }
});

test('models input parses comma- and newline-separated ids, dropping blanks', () => {
  assert.deepEqual(parseModelsInput('gpt-x, gpt-y,\n , gpt-z'), [{ id: 'gpt-x' }, { id: 'gpt-y' }, { id: 'gpt-z' }]);
  assert.deepEqual(parseModelsInput(''), []);
  assert.deepEqual(parseModelsInput('   '), []);
});

test('removing a custom provider also removes its local model-picker visibility record', () => {
  assert.deepEqual(
    withoutProviderModelPickerVisibility({
      openai: { visibleModelIds: ['gpt-5.6-sol'] },
      customlab: { showInModelPicker: false, visibleModelIds: [] },
    }, 'customlab'),
    { openai: { visibleModelIds: ['gpt-5.6-sol'] } },
  );
  assert.deepEqual(withoutProviderModelPickerVisibility(undefined, 'customlab'), {});
});

test('custom provider visibility uses the draft+catalog union and defaults draft-only ids to selected', () => {
  const candidates = ['catalog-a', 'draft-only', 'catalog-a', ''];
  assert.deepEqual(
    visibleCustomProviderModelIds(candidates, undefined),
    ['catalog-a', 'draft-only'],
  );
  assert.deepEqual(
    visibleCustomProviderModelIds(candidates, { showInModelPicker: false, visibleModelIds: ['catalog-a'] }),
    ['catalog-a'],
  );
  assert.deepEqual(
    visibleCustomProviderModelIds(candidates, { visibleModelIds: ['draft-only', 'missing'] }),
    ['draft-only'],
  );
});

// ---------------------------------------------------------------------------
// ProviderCredentials: pure logic for the built-in provider credential page.
// ---------------------------------------------------------------------------

test('the deep link selects the requested provider when it exists, else falls back to anthropic', () => {
  assert.equal(initialProviderSelection('deepseek'), 'deepseek');
  assert.equal(initialProviderSelection('not-a-real-provider'), 'anthropic');
  assert.equal(initialProviderSelection(undefined), 'anthropic');
});

test('custom provider editing accepts account credentials and exposes no environment credential controls', () => {
  const markup = renderToStaticMarkup(React.createElement(
    Theme.Provider, { value: tokens(false) },
    React.createElement(ProviderEditorFields, {
      name: 'custom', onName() {}, onDraft() {}, onApiKey() {}, apiKey: '',
      draft: { type: 'openai', models: [{ id: 'm' }], connections: [{ id: 'cn', apiKeyEnv: 'CUSTOM_CN_KEY' }] },
    }),
  ));
  assert.doesNotMatch(markup, /<option value="env"/);
  assert.doesNotMatch(markup, /aria-label="apiKeyEnv"/);
  const keyInput = markup.match(/<input[^>]*aria-label="API 密钥"[^>]*>/)?.[0];
  assert.ok(keyInput);
  assert.doesNotMatch(keyInput, /\bdisabled\b/);
  assert.doesNotMatch(markup, /aria-label="连接 1 apiKeyEnv"/);
});

test('provider detail shows an honest zero-visible state when every catalog model is hidden', () => {
  const bridge = {
    activeSession: { projectPath: '/test/project', sessionId: 'session-a' },
    bootstrap: {
      settings: {
        version: 1,
        projects: [],
        pinnedSessions: [],
        modelPickerVisibility: { openai: { visibleModelIds: [] } },
      },
      workspace: { path: '/test/project', trusted: true },
      providerAccountSnapshot: fixtureAccountSnapshot(['openai']),
    },
    desktop: {
      currentModel: 'openai/gpt-5.7-preview',
      providerModelCatalog: [{
        provider_id: 'openai',
        provider_label: 'OpenAI',
        models: [
          { reference: 'openai/gpt-5.6-sol', model_id: 'gpt-5.6-sol', display_name: 'GPT 5.6 Sol' },
          { reference: 'openai/gpt-5.7-preview', model_id: 'gpt-5.7-preview', display_name: 'GPT 5.7 Preview' },
        ],
      }],
    },
    connected: true,
    running: false,
    openSession: async () => undefined,
    restartBridge: async () => undefined,
    refreshProviderCredential: async () => undefined,
    setApiBaseUrl: async () => undefined,
    setModel: async () => undefined,
  };
  const markup = renderToStaticMarkup(React.createElement(
    Theme.Provider,
    { value: tokens(false) },
    React.createElement(ProviderCredentials, {
      bridge: bridge as any,
      initialProviderId: 'openai',
      snapshot: null,
      editingLayer: 'user',
      theme: 'light',
      onTheme: () => undefined,
      onNavigate: () => undefined,
      onClose: () => undefined,
      onJumpToLayer: () => undefined,
    }),
  ));

  assert.match(markup, /0 \/ 2 个模型可见/);
  assert.match(markup, /当前没有可显示模型，这个 Provider 会从对话模型列表隐藏。/);
  assert.match(markup, /GPT 5\.7 Preview/);
});

test('provider detail keeps stored checks and suppresses the zero-visible warning when the provider is hidden', () => {
  const bridge = {
    activeSession: { projectPath: '/test/project', sessionId: 'session-a' },
    bootstrap: {
      settings: {
        version: 1,
        projects: [],
        pinnedSessions: [],
        modelPickerVisibility: {
          openai: {
            showInModelPicker: false,
            visibleModelIds: ['gpt-5.7-preview'],
          },
        },
      },
      workspace: { path: '/test/project', trusted: true },
      providerAccountSnapshot: fixtureAccountSnapshot(['openai']),
    },
    desktop: {
      currentModel: 'openai/gpt-5.7-preview',
      providerModelCatalog: [{
        provider_id: 'openai',
        provider_label: 'OpenAI',
        models: [
          { reference: 'openai/gpt-5.6-sol', model_id: 'gpt-5.6-sol', display_name: 'GPT 5.6 Sol' },
          { reference: 'openai/gpt-5.7-preview', model_id: 'gpt-5.7-preview', display_name: 'GPT 5.7 Preview' },
        ],
      }],
    },
    connected: true,
    running: false,
    openSession: async () => undefined,
    restartBridge: async () => undefined,
    refreshProviderCredential: async () => undefined,
    setApiBaseUrl: async () => undefined,
    setModel: async () => undefined,
  };
  const markup = renderToStaticMarkup(React.createElement(
    Theme.Provider,
    { value: tokens(false) },
    React.createElement(ProviderCredentials, {
      bridge: bridge as any,
      initialProviderId: 'openai',
      snapshot: null,
      editingLayer: 'user',
      theme: 'light',
      onTheme: () => undefined,
      onNavigate: () => undefined,
      onClose: () => undefined,
      onJumpToLayer: () => undefined,
    }),
  ));

  assert.match(markup, /1 \/ 2 个模型可见/);
  assert.match(markup, /GPT 5\.7 Preview/);
  assert.doesNotMatch(markup, /当前没有可显示模型，这个 Provider 会从对话模型列表隐藏。/);
});

// The runtime half of the same finding: `apps/electron/test/` is not part
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

test('official setup metadata supports a new provider without a desktop binary entry', () => {
  const providers = providersFromCatalog([{ provider_id: 'new-provider', provider_label: 'New provider', models: [],
    definition: { base_url: 'https://new.example/v1', protocol: 'open_ai_chat', auth: 'api_key', credential_env: 'NEW_KEY', default_model_id: 'new-model', description: 'Published provider' } }]);
  assert.equal(providers[0].defaultApiBase, 'https://new.example/v1');
  assert.equal(providers[0].defaultModel, 'new-provider/new-model');
  assert.deepEqual(providersFromCatalog([]), []);
});
