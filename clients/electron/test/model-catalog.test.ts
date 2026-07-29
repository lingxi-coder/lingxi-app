import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  groupModelReferences,
  modelReference,
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

test('defaults Kimi Code to the model available on every membership tier', () => {
  assert.equal(providerById('kimi-code')?.defaultModel, 'kimi-code/kimi-for-coding');
});
