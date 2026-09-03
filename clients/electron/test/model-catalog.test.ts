import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  filterModelGroups,
  groupModelReferences,
  modelBillingGroups,
  modelDisplayLabel,
  modelReference,
  modelSelectionConfirmed,
  resolveModelSelection,
  waitForModelSelection,
} from '../src/renderer/bridge/modelCatalog';
import { providerById } from '../src/shared/providers';

test('groups only the curated qualified references supplied by the engine', () => {
  const references = [
    'anthropic/claude-sonnet-5',
    'openai/gpt-5.4',
    'anthropic/claude-haiku-4-5',
  ];

  const groups = groupModelReferences(references);

  assert.deepEqual(groups.map((group) => group.providerLabel), ['Anthropic', 'OpenAI']);
  assert.deepEqual(
    groups.map((group) => group.models.map((model) => model.reference)),
    [
      ['anthropic/claude-sonnet-5', 'anthropic/claude-haiku-4-5'],
      ['openai/gpt-5.4'],
    ],
  );
  assert.equal(groups.flatMap((group) => group.models).length, references.length);
});

test('same request model from different providers remains independently selectable', () => {
  const groups = groupModelReferences([
    'openai/gpt-5.4',
    'github-copilot/gpt-5.4',
  ]);

  assert.deepEqual(
    groups.flatMap((group) => group.models).map((model) => model.reference),
    ['openai/gpt-5.4', 'github-copilot/gpt-5.4'],
  );
  assert.deepEqual(
    groups.flatMap((group) => group.models).map((model) => model.label),
    ['GPT 5.4', 'GPT 5.4'],
  );
});

test('preserves nested request model paths while parsing the provider once', () => {
  assert.deepEqual(modelReference('openrouter/openai/gpt-5.4'), {
    reference: 'openrouter/openai/gpt-5.4',
    providerId: 'openrouter',
    requestModel: 'openai/gpt-5.4',
    label: 'GPT 5.4',
  });
});

test('does not duplicate repeated engine entries or invent fallback models', () => {
  assert.deepEqual(
    groupModelReferences(['deepseek/deepseek-chat', 'deepseek/deepseek-chat'])
      .flatMap((group) => group.models)
      .map((model) => model.reference),
    ['deepseek/deepseek-chat'],
  );
  assert.deepEqual(groupModelReferences([]), []);
});

test('renders Kimi provider and model labels from qualified engine references', () => {
  const [group] = groupModelReferences(['kimi/kimi-k3']);

  assert.equal(group?.providerLabel, 'Kimi');
  assert.equal(group?.models[0]?.label, 'Kimi K3');
});

test('keeps Kimi Code separate from the pay-as-you-go Kimi provider', () => {
  const groups = groupModelReferences(['kimi/kimi-k3', 'kimi-code/k3']);

  assert.deepEqual(groups.map((group) => group.providerLabel), ['Kimi', 'Kimi Code']);
  assert.deepEqual(groups.map((group) => group.models[0]?.label), ['Kimi K3', 'K3']);
});

test('filters models by friendly name, wire id, and provider without changing catalog order', () => {
  const groups = groupModelReferences([
    'anthropic/claude-sonnet-5',
    'openrouter/~openai/gpt-latest',
    'openrouter/inclusionai/ling-3.0-flash-fin:free',
  ]);
  const details = [
    { reference: 'openrouter/inclusionai/ling-3.0-flash-fin:free', display_name: 'InclusionAI: Ling 3.0 Flash Fin (free)' },
  ];

  assert.deepEqual(
    filterModelGroups(groups, 'flash fin', details).flatMap((group) => group.models.map((model) => model.reference)),
    ['openrouter/inclusionai/ling-3.0-flash-fin:free'],
  );
  assert.deepEqual(
    filterModelGroups(groups, '~OPENAI/GPT', details).flatMap((group) => group.models.map((model) => model.reference)),
    ['openrouter/~openai/gpt-latest'],
  );
  assert.deepEqual(
    filterModelGroups(groups, 'anthropic', details).flatMap((group) => group.models.map((model) => model.reference)),
    ['anthropic/claude-sonnet-5'],
  );
  assert.deepEqual(filterModelGroups(groups, '   ', details), groups);
});

test('separates OpenRouter paid and free choices from authoritative billing metadata', () => {
  const [openrouter] = groupModelReferences([
    'openrouter/openrouter/auto',
    'openrouter/~anthropic/claude-opus-latest',
    'openrouter/openrouter/free',
    'openrouter/cohere/north-mini-code:free',
  ]);
  const details = [
    { reference: 'openrouter/openrouter/auto', display_name: 'OpenRouter Auto', pricing: { billing_mode: 'per_token' } },
    { reference: 'openrouter/~anthropic/claude-opus-latest', display_name: 'Anthropic: Claude Opus Latest', pricing: { billing_mode: 'per_token' } },
    { reference: 'openrouter/openrouter/free', display_name: 'OpenRouter Free', pricing: { billing_mode: 'free' } },
    { reference: 'openrouter/cohere/north-mini-code:free', display_name: 'Cohere: North Mini Code (free)', pricing: { billing_mode: 'free' } },
  ];

  assert.deepEqual(
    modelBillingGroups(openrouter!, details).map((section) => [section.label, section.models.map((model) => model.reference)]),
    [
      ['Paid', ['openrouter/openrouter/auto', 'openrouter/~anthropic/claude-opus-latest']],
      ['Free', ['openrouter/openrouter/free', 'openrouter/cohere/north-mini-code:free']],
    ],
  );
  assert.equal(modelDisplayLabel(openrouter!.models[1]!, details), 'Anthropic: Claude Opus Latest');
});

test('keeps non-OpenRouter providers in one unlabeled billing section', () => {
  const [anthropic] = groupModelReferences(['anthropic/claude-sonnet-5']);
  assert.deepEqual(modelBillingGroups(anthropic!, []), [{ label: null, models: anthropic!.models }]);
});

test('defaults Kimi Code to the model available on every membership tier', () => {
  assert.equal(providerById('kimi-code')?.defaultModel, 'kimi-code/k3');
});

test('routes an unconfigured known provider to its settings while preserving the model', () => {
  assert.deepEqual(
    resolveModelSelection('deepseek/deepseek-v4-flash', [{ providerId: 'deepseek', configured: false }]),
    { kind: 'connect', providerId: 'deepseek', reference: 'deepseek/deepseek-v4-flash' },
  );
});

test('maps built-in models to Anthropic and waits for credential status', () => {
  assert.deepEqual(resolveModelSelection('builtin/claude-sonnet-5'), {
    kind: 'loading', providerId: 'anthropic', reference: 'builtin/claude-sonnet-5',
  });
  assert.deepEqual(resolveModelSelection('builtin/claude-sonnet-5', [{ providerId: 'anthropic', configured: true }]), {
    kind: 'select', reference: 'builtin/claude-sonnet-5',
  });
});

test('keeps unknown and unqualified engine models directly selectable', () => {
  assert.deepEqual(resolveModelSelection('community/custom-model', []), { kind: 'select', reference: 'community/custom-model' });
  assert.deepEqual(resolveModelSelection('custom-model'), { kind: 'select', reference: 'custom-model' });
});

test('model confirmation requires the authoritative model_changed value', () => {
  assert.equal(modelSelectionConfirmed('openai/gpt-5.6-sol', 'deepseek/deepseek-v4-flash'), false);
  assert.equal(modelSelectionConfirmed('deepseek/deepseek-v4-flash', 'deepseek/deepseek-v4-flash'), true);
});

test('waits for authoritative model confirmation and times out deterministically', async () => {
  let current: string | null = 'anthropic/claude-sonnet-5';
  setTimeout(() => { current = 'deepseek/deepseek-v4-flash'; }, 5);
  await waitForModelSelection('deepseek/deepseek-v4-flash', () => current, { timeoutMs: 100, pollMs: 1 });
  await assert.rejects(
    waitForModelSelection('never/confirmed', () => current, { timeoutMs: 5, pollMs: 1 }),
    /did not confirm model/,
  );
});

test('aborting an authoritative wait clears its polling path', async () => {
  const controller = new AbortController();
  const pending = waitForModelSelection('never/confirmed', () => null, { timeoutMs: 100, pollMs: 1, signal: controller.signal });
  controller.abort();
  await assert.rejects(pending, /wait aborted/);
});
