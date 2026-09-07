import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

import {
  isEditableLayer,
  parseModelsInput,
  providersFromLayer,
  routingFromLayer,
  withoutProviderModelPickerVisibility,
  visibleCustomProviderModelIds,
  validateCustomProvider,
} from '../src/renderer/components/settings/pages/CustomProviders';
import {
  credentialSaveButtonLabel,
  credentialStatusKind,
  initialProviderSelection,
  providerCredentialStatus,
  ProviderCredentials,
  shouldRequestCredentialPreview,
} from '../src/renderer/components/settings/pages/ProviderCredentials';
import { rowState, type SettingsSnapshot } from '../src/renderer/components/settings/useEngineSettings';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';

(globalThis as { React?: typeof React }).React = React;

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
// ProviderCredentials: pure logic for the built-in provider credential page.
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

test('credential status: signed broker configuration failures are explicit', () => {
  assert.equal(credentialStatusKind({
    configured: false,
    encryptionAvailable: false,
    storageError: '签名配置错误',
  }), 'unavailable');
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

test('provider list status describes credential configuration instead of a persistent connection', () => {
  assert.deepEqual(providerCredentialStatus({ configured: true, runtimeOnly: false }, true), { kind: 'configured', label: '已配置' });
  assert.deepEqual(providerCredentialStatus({ configured: false, runtimeOnly: true }, true), { kind: 'runtime', label: '仅运行时' });
  assert.deepEqual(providerCredentialStatus({ configured: false, runtimeOnly: false }, true), { kind: 'unconfigured', label: '未配置' });
  assert.deepEqual(providerCredentialStatus(undefined, false), { kind: 'unavailable', label: 'CLI / TUI' });
  assert.deepEqual(
    providerCredentialStatus({ configured: false, storageError: '签名配置错误' }, true),
    { kind: 'unavailable', label: '安全存储不可用' },
  );
});

test('credential save button uses save terminology for new and replacement keys', () => {
  assert.equal(credentialSaveButtonLabel({ saving: true, modelApplying: false }), '保存中…');
  assert.equal(credentialSaveButtonLabel({ saving: false, modelApplying: true }), '应用模型中…');
  assert.equal(credentialSaveButtonLabel({ saving: false, modelApplying: false }), '保存');
});

test('disconnected provider settings keeps credential storage independent from engine state', () => {
  const bridge = {
    activeSession: { projectPath: '/test/project', sessionId: 'session-a' },
    bootstrap: {
      settings: { version: 1, projects: [], pinnedSessions: [] },
      workspace: { path: '/test/project', trusted: true },
      providerCredentials: [{ providerId: 'anthropic', configured: false, encryptionAvailable: false }],
    },
    desktop: { currentModel: null },
    connected: false,
    running: false,
    openSession: async () => undefined,
    restartBridge: async () => undefined,
    setProviderCredential: async () => undefined,
    clearProviderCredential: async () => undefined,
    refreshProviderCredential: async () => undefined,
    setApiBaseUrl: async () => undefined,
    setModel: async () => undefined,
  };
  const markup = renderToStaticMarkup(React.createElement(
    Theme.Provider,
    { value: tokens(false) },
    React.createElement(ProviderCredentials, {
      bridge: bridge as any,
      initialProviderId: 'anthropic',
      snapshot: null,
      editingLayer: 'user',
      theme: 'light',
      onTheme: () => undefined,
      onNavigate: () => undefined,
      onClose: () => undefined,
      onJumpToLayer: () => undefined,
    }),
  ));

  assert.doesNotMatch(markup, /data-testid="provider-engine-recovery"/);
  assert.doesNotMatch(markup, /连接状态|连接引擎/);
  assert.match(markup, /data-testid="provider-list-back"/);
  assert.match(markup, /aria-label="Anthropic API key"/);
  const credentialMarker = markup.indexOf('aria-label="Anthropic API key"');
  const credentialStart = markup.lastIndexOf('<input', credentialMarker);
  const credentialEnd = markup.indexOf('>', credentialMarker);
  assert.ok(credentialMarker >= 0 && credentialStart >= 0 && credentialEnd > credentialMarker);
  assert.doesNotMatch(markup.slice(credentialStart, credentialEnd + 1), /\bdisabled\b/);
  const disconnectedTestMarker = markup.indexOf('data-testid="provider-connection-test"');
  const disconnectedTestStart = markup.lastIndexOf('<button', disconnectedTestMarker);
  const disconnectedTestEnd = markup.indexOf('>', disconnectedTestMarker);
  assert.ok(disconnectedTestMarker >= 0 && disconnectedTestStart >= 0 && disconnectedTestEnd > disconnectedTestMarker);
  assert.match(markup.slice(disconnectedTestStart, disconnectedTestEnd + 1), /\bdisabled\b/);
  assert.doesNotMatch(markup, /macOS 钥匙串不可用/);
  assert.doesNotMatch(markup, /自定义 API 地址/);
});

test('provider settings defaults to a clickable status list without mounting a credential form', () => {
  const bridge = {
    activeSession: { projectPath: '/test/project', sessionId: 'session-a' },
    bootstrap: {
      settings: { version: 1, projects: [], pinnedSessions: [] },
      workspace: { path: '/test/project', trusted: true },
      providerCredentials: [
        { providerId: 'deepseek', configured: true, encryptionAvailable: true, runtimeOnly: false },
        { providerId: 'openrouter', configured: false, encryptionAvailable: true, runtimeOnly: true },
      ],
    },
    desktop: { currentModel: null },
    connected: true,
    running: false,
    openSession: async () => undefined,
    restartBridge: async () => undefined,
    setProviderCredential: async () => undefined,
    clearProviderCredential: async () => undefined,
    refreshProviderCredential: async () => undefined,
    setApiBaseUrl: async () => undefined,
    setModel: async () => undefined,
  };
  const markup = renderToStaticMarkup(React.createElement(
    Theme.Provider,
    { value: tokens(false) },
    React.createElement(ProviderCredentials, {
      bridge: bridge as any,
      snapshot: null,
      editingLayer: 'user',
      theme: 'light',
      onTheme: () => undefined,
      onNavigate: () => undefined,
      onClose: () => undefined,
      onJumpToLayer: () => undefined,
    }),
  ));

  assert.match(markup, /data-testid="provider-list-item-deepseek"/);
  assert.match(markup, /aria-label="DeepSeek，已配置"/);
  assert.match(markup, /aria-label="OpenRouter，仅运行时"/);
  assert.doesNotMatch(markup, /data-testid="provider-list-back"/);
  assert.doesNotMatch(markup, /aria-label="Anthropic API key"/);
  assert.doesNotMatch(markup, /正在编辑|>选择<|>当前</);
});

test('configured provider detail shows only the masked suffix returned by the engine', () => {
  const bridge = {
    activeSession: { projectPath: '/test/project', sessionId: 'session-a' },
    bootstrap: {
      settings: { version: 1, projects: [], pinnedSessions: [] },
      workspace: { path: '/test/project', trusted: true },
      providerCredentials: [{
        providerId: 'deepseek',
        configured: true,
        encryptionAvailable: true,
        credentialPreview: '••••abcd',
      }],
    },
    desktop: { currentModel: 'deepseek/deepseek-v4-flash' },
    connected: true,
    running: false,
    openSession: async () => undefined,
    restartBridge: async () => undefined,
    setProviderCredential: async () => undefined,
    clearProviderCredential: async () => undefined,
    refreshProviderCredential: async () => undefined,
    setApiBaseUrl: async () => undefined,
    setModel: async () => undefined,
  };
  const markup = renderToStaticMarkup(React.createElement(
    Theme.Provider,
    { value: tokens(false) },
    React.createElement(ProviderCredentials, {
      bridge: bridge as any,
      initialProviderId: 'deepseek',
      snapshot: null,
      editingLayer: 'user',
      theme: 'light',
      onTheme: () => undefined,
      onNavigate: () => undefined,
      onClose: () => undefined,
      onJumpToLayer: () => undefined,
    }),
  ));

  assert.match(markup, /data-testid="provider-credential-preview"/);
  assert.match(markup, /placeholder="••••abcd"/);
  const saveMarker = markup.indexOf('>保存</button>');
  const saveStart = markup.lastIndexOf('<button', saveMarker);
  const saveEnd = markup.indexOf('>', saveStart);
  assert.ok(saveMarker >= 0 && saveStart >= 0 && saveEnd > saveStart);
  assert.match(markup.slice(saveStart, saveEnd + 1), /width:88px/);
  assert.match(markup.slice(saveStart, saveEnd + 1), /justify-content:center/);
  assert.match(markup, /data-testid="provider-connection-test"/);
  assert.match(markup, /测试连接/);
  const connectedTestMarker = markup.indexOf('data-testid="provider-connection-test"');
  const connectedTestStart = markup.lastIndexOf('<button', connectedTestMarker);
  const connectedTestEnd = markup.indexOf('>', connectedTestMarker);
  assert.ok(connectedTestMarker >= 0 && connectedTestStart >= 0 && connectedTestEnd > connectedTestMarker);
  assert.doesNotMatch(markup.slice(connectedTestStart, connectedTestEnd + 1), /\bdisabled\b/);
  assert.match(markup, /href="https:\/\/platform\.deepseek\.com\/api_keys"/);
  assert.match(markup, /获取或管理 API Key/);
  assert.match(markup, />保存<\/button>/);
  assert.match(markup, />删除 API Key<\/button>/);
  const connectionTestRow = markup.indexOf('>连接测试<');
  const saveAction = markup.indexOf('>保存</button>');
  const deleteAction = markup.indexOf('>删除 API Key</button>');
  assert.ok(connectionTestRow >= 0 && saveAction > connectionTestRow && deleteAction > connectionTestRow);
  assert.doesNotMatch(markup, /替换|断开连接|自定义 API 地址/);
  assert.doesNotMatch(markup, /sk-test-secret|sk-shared-secret/);
});

test('configured provider detail keeps its storage row while the engine is disconnected', () => {
  const bridge = {
    activeSession: { projectPath: '/test/project', sessionId: 'session-a' },
    bootstrap: {
      settings: { version: 1, projects: [], pinnedSessions: [] },
      workspace: { path: '/test/project', trusted: true },
      providerCredentials: [{
        providerId: 'deepseek',
        configured: true,
        encryptionAvailable: true,
        credentialPreview: '••••abcd',
      }],
    },
    desktop: { currentModel: 'deepseek/deepseek-v4-flash' },
    connected: false,
    running: false,
    openSession: async () => undefined,
    restartBridge: async () => undefined,
    setProviderCredential: async () => undefined,
    clearProviderCredential: async () => undefined,
    refreshProviderCredential: async () => undefined,
    setApiBaseUrl: async () => undefined,
    setModel: async () => undefined,
  };
  const markup = renderToStaticMarkup(React.createElement(
    Theme.Provider,
    { value: tokens(false) },
    React.createElement(ProviderCredentials, {
      bridge: bridge as any,
      initialProviderId: 'deepseek',
      snapshot: null,
      editingLayer: 'user',
      theme: 'light',
      onTheme: () => undefined,
      onNavigate: () => undefined,
      onClose: () => undefined,
      onJumpToLayer: () => undefined,
    }),
  ));

  assert.match(markup, /已安全保存在 macOS Data Protection Keychain 中/);
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
      providerCredentials: [{
        providerId: 'openai',
        configured: true,
        encryptionAvailable: true,
      }],
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
    setProviderCredential: async () => undefined,
    clearProviderCredential: async () => undefined,
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
      providerCredentials: [{
        providerId: 'openai',
        configured: true,
        encryptionAvailable: true,
      }],
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
    setProviderCredential: async () => undefined,
    clearProviderCredential: async () => undefined,
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

test('configured credentials request a disconnected preview only when the broker is available', () => {
  assert.equal(shouldRequestCredentialPreview(
    'deepseek',
    { configured: true, credentialPreview: undefined, storageError: undefined },
    new Set(),
    true,
  ), true);
  assert.equal(shouldRequestCredentialPreview(
    'deepseek',
    { configured: true, credentialPreview: '••••abcd', storageError: undefined },
    new Set(),
    true,
  ), false);
  assert.equal(shouldRequestCredentialPreview(
    'deepseek',
    { configured: true, credentialPreview: undefined, storageError: undefined },
    new Set(),
    false,
  ), false);
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
