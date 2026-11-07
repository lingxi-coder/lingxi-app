import assert from 'node:assert/strict';
import test from 'node:test';
import type { AccountUsageRowDto } from '@lingxi/bridge-client';
import { accountSessionUsage, formatSessionTokens, formatSessionUsd } from '../src/shared/accountUsage';

function row(input: {
  provider: string; account: string; generation: number; auth: string; model: string;
  nano?: number; inputTokens?: number; outputTokens?: number; cacheRead?: number; cacheWrite?: number; requests?: number;
}): AccountUsageRowDto {
  return {
    attribution: { provider_id: input.provider, account_id: input.account, identity_generation: input.generation, auth_kind: input.auth },
    model: input.model,
    total_nano_usd: input.nano ?? 1_000_000_000,
    input_tokens: input.inputTokens ?? 10,
    output_tokens: input.outputTokens ?? 4,
    cache_read_input_tokens: input.cacheRead ?? 1,
    cache_creation_input_tokens: input.cacheWrite ?? 2,
    reasoning_tokens: 0,
    web_search_requests: 0,
    request_count: input.requests ?? 1,
  };
}

const rows: AccountUsageRowDto[] = [
  row({ provider: 'anthropic', account: 'key-a', generation: 2, auth: 'api_key', model: 'claude-sonnet', nano: 2_000_000_000, inputTokens: 100, outputTokens: 20, requests: 2 }),
  row({ provider: 'anthropic', account: 'key-a', generation: 2, auth: 'api_key', model: 'claude-opus', nano: 500_000_000, inputTokens: 15, outputTokens: 5, requests: 1 }),
  row({ provider: 'anthropic', account: 'key-a', generation: 1, auth: 'api_key', model: 'claude-sonnet', nano: 100, inputTokens: 7, outputTokens: 1, requests: 1 }),
  row({ provider: 'anthropic', account: 'key-b', generation: 1, auth: 'api_key', model: 'claude-sonnet', nano: 9, inputTokens: 3, outputTokens: 3, requests: 4 }),
  row({ provider: 'openai', account: 'sub', generation: 3, auth: 'oauth', model: 'gpt', nano: 40, inputTokens: 8, outputTokens: 8, requests: 1 }),
  row({ provider: 'openai-chatgpt', account: 'sub', generation: 3, auth: 'oauth', model: 'gpt', nano: 60, inputTokens: 2, outputTokens: 2, requests: 1 }),
  { attribution: null, model: 'orphan', total_nano_usd: 77, input_tokens: 9, output_tokens: 9, cache_read_input_tokens: 0, cache_creation_input_tokens: 0, reasoning_tokens: 0, web_search_requests: 0, request_count: 1 },
];

test('session usage keeps providers, keys, and credential generations apart', () => {
  const keyA = accountSessionUsage(rows, 'anthropic', { accountId: 'key-a', identityGeneration: 2 });
  assert.equal(keyA.current.inputTokens, 115);
  assert.equal(keyA.current.outputTokens, 25);
  assert.equal(keyA.current.nanoUsd, 2_500_000_000);
  assert.equal(keyA.current.requestCount, 3);
  assert.deepEqual(keyA.current.models.map(model => model.model), ['claude-opus', 'claude-sonnet']);
  assert.equal(keyA.earlier.length, 1);
  assert.equal(keyA.earlier[0]?.generation, 1);
  assert.equal(keyA.earlier[0]?.usage.inputTokens, 7);
  assert.equal(keyA.earlier[0]?.usage.nanoUsd, 100);

  const keyB = accountSessionUsage(rows, 'anthropic', { accountId: 'key-b', identityGeneration: 1 });
  assert.equal(keyB.current.inputTokens, 3);
  assert.equal(keyB.current.requestCount, 4);
  assert.deepEqual(keyB.earlier, []);

  const openai = accountSessionUsage(rows, 'openai-chatgpt', { accountId: 'sub', identityGeneration: 3 });
  assert.equal(openai.current.inputTokens, 10);
  assert.equal(openai.current.nanoUsd, 100);
  assert.equal(openai.current.models.length, 1);

  const unused = accountSessionUsage(rows, 'google', { accountId: 'missing', identityGeneration: 1 });
  assert.equal(unused.current.inputTokens, 0);
  assert.equal(unused.current.nanoUsd, 0);
  assert.deepEqual(unused.current.models, []);
  assert.deepEqual(unused.earlier, []);
  assert.equal(accountSessionUsage(undefined, 'anthropic', { accountId: 'key-a', identityGeneration: 2 }).current.requestCount, 0);
});

test('session cost and token formatters stay compact', () => {
  assert.equal(formatSessionTokens(0), '0');
  assert.equal(formatSessionTokens(999), '999');
  assert.equal(formatSessionTokens(2000), '2k');
  assert.equal(formatSessionTokens(1500), '1.5k');
  assert.equal(formatSessionUsd(0), '$0.0000');
  assert.equal(formatSessionUsd(1_500_000_000), '$1.50');
  assert.equal(formatSessionUsd(123_000), '$0.0001');
});
