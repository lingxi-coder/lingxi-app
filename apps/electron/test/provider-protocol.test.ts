import test from 'node:test';
import assert from 'node:assert/strict';
import { protocolDefaults, protocolSettingsPatch } from '../src/renderer/components/settings/pages/ProviderProtocol';
import { PROVIDER_PROTOCOLS } from '../src/shared/providerProtocols';

test('protocol setting changes only the selected account in its destination layer', () => {
  const routing = { aliases: { fast: 'deepseek/deepseek-flash' }, retry: { maxAttempts: 2 },
    defaultProtocols: { openai: 'open_ai_chat', deepseek: 'open_ai_chat' } };
  const patch = protocolSettingsPatch(routing, 'deepseek', 'anthropic_messages');
  assert.deepEqual(patch.routing.aliases, routing.aliases);
  assert.deepEqual(patch.routing.retry, routing.retry);
  assert.deepEqual(protocolDefaults(patch.routing), { openai: 'open_ai_chat', deepseek: 'anthropic_messages' });
  assert.equal(routing.defaultProtocols.deepseek, 'open_ai_chat');
  assert.deepEqual(protocolDefaults(protocolSettingsPatch(patch.routing, 'deepseek', '__inherit__').routing), { openai: 'open_ai_chat' });
  assert.equal(protocolDefaults(protocolSettingsPatch(patch.routing, 'deepseek', '').routing).deepseek, null);
});

test('Codex account keeps its Responses format while DeepSeek offers all declared formats', () => {
  assert.deepEqual(PROVIDER_PROTOCOLS.find(p => p.profile === 'openai-chatgpt')?.formats, ['open_ai_responses']);
  assert.deepEqual(PROVIDER_PROTOCOLS.find(p => p.profile === 'deepseek')?.formats,
    ['open_ai_chat', 'anthropic_messages', 'open_ai_responses']);
  assert.deepEqual(protocolDefaults(null), {});
});
