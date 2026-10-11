import assert from 'node:assert/strict';
import { test } from 'node:test';
import { normalizeAudioUsage } from '../src/shared/audioUsage';
import { NativeAudioManager } from '../src/main/audio/nativeAudioManager';
import { DiagnosticBuffer } from '../src/main/host-utils';

test('audio usage preserves reported numeric cost and counters without raw provider payloads or guessed chat cost', () => {
  assert.deepEqual(normalizeAudioUsage({ input_tokens: 3, output_tokens: 4, audio_seconds: 0.5, cost_usd: 0.001, native: { transcript: 'private', authorization: 'secret' } }), { inputTokens: 3, outputTokens: 4, audioSeconds: 0.5, costUsd: 0.001 });
  assert.deepEqual(normalizeAudioUsage({ inputTokens: 8, outputTokens: 2, totalTokens: 10, cost_usd: null }), { inputTokens: 8, outputTokens: 2, totalTokens: 10 });
  assert.deepEqual(normalizeAudioUsage({ input_tokens: -1, total_tokens: Infinity, cost_usd: NaN }), {});
});

test('per-operation usage ledger is bounded, account/profile scoped, and snapshots cannot mutate retained data', () => {
  const manager = new NativeAudioManager({ isPackaged: false, resourcesPath: '/unused', userDataPath: '/tmp', diagnostics: new DiagnosticBuffer() });
  for (let index = 0; index < 130; index++) manager.recordAudioUsage({ operationId: `operation-${index}`, configurationRevision: 4, kind: 'speech', profileId: 'exact-profile', providerId: 'openai', accountScope: 'account', modelId: 'audio-model' }, { characters: index });
  const ledger = manager.getAudioUsageLedger();
  assert.equal(ledger.length, 128); assert.equal(ledger[0]?.operationId, 'operation-2'); assert.equal(ledger.at(-1)?.usage.characters, 129);
  assert.equal(ledger.at(-1)?.usage.costUsd, undefined); ledger[0]!.profileId = 'other';
  assert.equal(manager.getAudioUsageLedger()[0]?.profileId, 'exact-profile');
});
