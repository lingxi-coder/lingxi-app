import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import {
  audioConfigurationDefaults,
  normalizeAudioConfiguration,
  resolveAudioRoute,
  type AudioRouteRequest,
} from '../../../apps/electron/src/shared/generatedAudioConfiguration.ts';

const fixture = JSON.parse(readFileSync(new URL('../audio-config-fixtures.json', import.meta.url), 'utf8')) as {
  normalization: Array<{ name: string; input: unknown; expected: unknown }>;
  routes: Array<{ name: string; input: AudioRouteRequest; expected: unknown }>;
};

test('generated TypeScript defaults, normalization, and unsupported version rejection match the shared fixture', () => {
  assert.deepEqual(audioConfigurationDefaults(), fixture.normalization[0]?.expected);
  for (const item of fixture.normalization) {
    assert.deepEqual(normalizeAudioConfiguration(item.input), item.expected, item.name);
  }

});

test('generated TypeScript requested/effective routes match the shared fixture', () => {
  for (const item of fixture.routes) {
    assert.deepEqual(resolveAudioRoute(item.input), item.expected, item.name);
  }
});
