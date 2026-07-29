import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  groupModelReferences,
  modelReference,
} from '../src/renderer/bridge/modelCatalog';

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
