import { test } from 'node:test';
import assert from 'node:assert/strict';

import type { ModelDetailsDto } from '@lingxi/bridge-client';
import {
  candidatesFromCatalog,
  configuredCandidates,
  missingRoles,
  rolesFromFusionObject,
  routeOf,
} from '../src/renderer/components/settings/pages/Fusion';
import { SETTINGS_NAV } from '../src/renderer/components/settings/nav';
import { PAGE_CONTENT } from '../src/renderer/components/settings/SettingsScreen';

function model(id: string, overrides: Partial<ModelDetailsDto> = {}): ModelDetailsDto {
  return {
    reference: `anthropic/${id}`,
    provider_id: 'anthropic',
    provider_label: 'Anthropic',
    display_name: id,
    model_id: id,
    input_modalities: ['text'],
    output_modalities: ['text'],
    capabilities: {
      streaming: true, tools: true, vision: false,
      documents: false, reasoning: true, structured_output: true,
    },
    reasoning: { options: [], forced_reasoning: false, editable: true },
    ...overrides,
  } as ModelDetailsDto;
}

test('the Fusion page is declared, layered, and actually registered', () => {
  const page = SETTINGS_NAV.find((entry) => entry.id === 'fusion');
  assert.ok(page, 'the nav must declare the Fusion page');
  assert.equal(page.layered, true, 'settings.fusion lives in the four settings layers');
  assert.equal(page.needsEngine, true, 'the model pickers read the live provider catalog');
  assert.ok(PAGE_CONTENT.fusion, 'a declared page with no component renders a placeholder');
});

test('roles read out of one layer, and a malformed entry reads as absent', () => {
  const roles = rolesFromFusionObject({
    panelModels: [
      { profile: 'anthropic', model: 'claude-opus-5' },
      { profile: '', model: 'gpt-5.6-sol' },
      { model: 'no-profile' },
      'not-an-object',
    ],
    analystModel: { profile: 'openai', model: 'gpt-5.6-terra' },
    synthesizerModel: { profile: 'openai' },
  });
  assert.deepEqual(roles.panels.map(routeOf), ['anthropic/claude-opus-5']);
  assert.deepEqual(roles.analyst, { profile: 'openai', model: 'gpt-5.6-terra' });
  assert.equal(roles.synthesizer, null, 'a half-written role is not a configured role');
});

test('a one-model roster reports as missing, not as a small roster', () => {
  const roles = rolesFromFusionObject({
    panelModels: [{ profile: 'anthropic', model: 'claude-opus-5' }],
    analystModel: { profile: 'openai', model: 'gpt-5.6-terra' },
    synthesizerModel: { profile: 'openai', model: 'gpt-5.6-terra' },
  });
  assert.deepEqual(missingRoles(roles), ['panel 模型']);
});

test('an empty fusion object reports every role missing, in engine order', () => {
  assert.deepEqual(
    missingRoles(rolesFromFusionObject({})),
    ['panel 模型', 'analyst 模型', 'synthesizer 模型'],
  );
});

test('analyst candidates gate on fusion_analyst_capable, not on the capability bit', () => {
  // The Gemini shape: the MODEL claims structured output, but its profile's
  // codec cannot put a `response_format` on the wire. Offering it would sell a
  // pick whose only symptom is a failed run after every panel has spent.
  const rows = candidatesFromCatalog([
    {
      provider_id: 'anthropic',
      provider_label: 'Anthropic',
      models: [
        model('claude-opus-5', { fusion_analyst_capable: true }),
        model('gemini-3-pro', { fusion_analyst_capable: false }),
        model('older-engine-row'),
      ],
    },
  ]);
  assert.deepEqual(
    rows.filter((row) => row.analystCapable).map((row) => row.choice.model),
    ['claude-opus-5'],
  );
  assert.equal(
    rows.find((row) => row.choice.model === 'gemini-3-pro')?.analystCapable,
    false,
    'structured_output: true alone must not make a route analyst-capable',
  );
  assert.equal(
    rows.find((row) => row.choice.model === 'older-engine-row')?.analystCapable,
    false,
    'an engine too old to send the field must fail closed, not open',
  );
});

test('a catalog row with no addressable route is dropped rather than written as a broken choice', () => {
  const rows = candidatesFromCatalog([
    {
      provider_id: '',
      provider_label: 'Broken',
      models: [model('has-id', { provider_id: '' }), model('', { model_id: '' })],
    },
  ]);
  assert.deepEqual(rows, []);
});

test('a model_id is what goes into settings, never the display name or the reference', () => {
  const rows = candidatesFromCatalog([
    {
      provider_id: 'openai',
      provider_label: 'OpenAI',
      models: [model('gpt-5.6-sol', {
        provider_id: 'openai',
        provider_label: 'OpenAI',
        display_name: 'GPT-5.6 Sol',
        reference: 'openai/gpt-5.6-sol',
      })],
    },
  ]);
  assert.deepEqual(rows[0]?.choice, { profile: 'openai', model: 'gpt-5.6-sol' });
  assert.equal(routeOf(rows[0]!.choice), 'openai/gpt-5.6-sol');
});


test('Fusion only offers configured providers including custom profiles and builtin alias', () => {
  const rows = candidatesFromCatalog(['openai', 'deepseek', 'custom', 'builtin'].map((provider_id) => ({
    provider_id, provider_label: provider_id,
    models: [model('test-model', { provider_id, fusion_analyst_capable: true })],
  })));
  assert.deepEqual(configuredCandidates(rows, [
    { providerId: 'openai', configured: false },
    { providerId: 'deepseek', configured: true },
    { providerId: 'custom', configured: true },
    { providerId: 'anthropic', configured: true },
  ]).map((row) => row.choice.profile), ['deepseek', 'custom', 'builtin']);
  assert.deepEqual(configuredCandidates(rows), []);
  assert.deepEqual(configuredCandidates(rows, []), []);
});
