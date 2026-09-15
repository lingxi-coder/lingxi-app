import { test } from 'node:test';
import assert from 'node:assert/strict';

import { PROVIDERS } from '../src/shared/providers';

/**
 * Ruling B-5 (2026-08-27 desktop-audio-capability ledger): `ProviderDefinition`
 * gained a `transcriptionCapable` field so the voice-capability probe can
 * tell "a provider is configured" apart from "the configured provider can
 * actually transcribe audio". The safe failure direction is under-claiming
 * (a capable provider reported as incapable); the dangerous one is a
 * provider flipped to `true` without evidence, which would route a user's
 * audio to an endpoint that 404s.
 *
 * This test pins the exact transcription-capable set so a provider cannot
 * be flipped on (or off) silently in a future edit — any change to the set
 * must touch this assertion and, per the rule above, must come with a
 * verifiable, documented endpoint cited in `shared/providers.ts`.
 */
test('exactly the evidenced providers are marked transcription-capable', () => {
  const capable = PROVIDERS.filter((p) => p.transcriptionCapable).map((p) => p.id).sort();
  assert.deepEqual(
    capable,
    ['gemini', 'openai', 'openrouter', 'zai'].sort(),
    'a provider was flipped without updating this pin — verify a real hosted transcription ' +
      'endpoint exists for it and update both the provider comment and this list together',
  );
});

test('every provider declares transcriptionCapable explicitly (no accidental undefined)', () => {
  for (const provider of PROVIDERS) {
    assert.equal(
      typeof provider.transcriptionCapable, 'boolean',
      `${provider.id} must declare transcriptionCapable as a real boolean, not leave it undefined`,
    );
  }
});

test('every provider exposes a secure official API base and credential-management link', () => {
  for (const provider of PROVIDERS) {
    assert.match(provider.defaultApiBase, /^https:\/\//, `${provider.id} API base must use HTTPS`);
    assert.match(provider.credentialManagementUrl, /^https:\/\//, `${provider.id} credential link must use HTTPS`);
  }
});

test('the providers with no hosted transcription endpoint stay false', () => {
  const incapable = PROVIDERS.filter((p) => !p.transcriptionCapable).map((p) => p.id).sort();
  assert.deepEqual(
    incapable,
    ['anthropic', 'deepseek', 'github-copilot', 'glm-coding', 'kimi', 'kimi-code', 'openai-chatgpt'].sort(),
  );
});
