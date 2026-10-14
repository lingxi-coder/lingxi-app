import assert from 'node:assert/strict';
import { test } from 'node:test';
import { validateClientEvent, validateFusionCatalog } from '../src/validation.js';
import type { FusionAdminCommandDto } from '../src/protocol.js';

test('Fusion selection receipts retain correlation and require exactly one result', () => {
  for (const result of [{ selection_json: '{"reference":{"scope":"user","id":"review"}}' }, { error: 'Select a Fusion strategy' }]) {
    const event = { type: 'fusion_strategy_resolved', operation_id: 41, ...result };
    assert.deepEqual(validateClientEvent(event), event);
  }
  assert.throws(() => validateClientEvent({ type: 'fusion_strategy_resolved', operation_id: 41 }));
  assert.throws(() => validateClientEvent({ type: 'fusion_strategy_resolved', operation_id: 41, selection_json: '{}', error: 'failed' }));
  assert.throws(() => validateClientEvent({ type: 'fusion_strategy_resolved', operation_id: -1, selection_json: '{}' }));
});

test('Fusion catalog and admin commands carry explicit scope revisions', () => {
  const event = { type: 'fusion_catalog', catalog_json: '{"scopes":[],"effective_globals":{},"default_strategy":null,"diagnostics":[]}' };
  assert.deepEqual(validateClientEvent(event), event);
  assert.throws(() => validateClientEvent({ type: 'fusion_catalog', catalog_json: {} }));
  const command: FusionAdminCommandDto = {
    action: 'save', operation_id: 41, scope: 'project', revision: 'sha256',
    payload_json: JSON.stringify({ id: 'review', name: 'Review', config: {} }),
  };
  assert.deepEqual(JSON.parse(JSON.stringify(command)), command);
});


test('Fusion catalog explicitly preserves excluded editable scopes and rejects missing admission', () => {
  const scope = {
    scope: 'project', admitted: false, path: '/project/.lingxi/settings.json', revision: 'sha256',
    strategies: [{ id: 'review', name: 'Review', config: {} }], default_strategy_id: 'review',
    globals: {}, diagnostic: null,
  };
  const catalog = { scopes: [scope], effective_globals: {}, default_strategy: null, diagnostics: [] };
  assert.deepEqual(validateFusionCatalog(catalog), catalog);
  assert.deepEqual(validateClientEvent({ type: 'fusion_catalog', catalog_json: JSON.stringify(catalog) }), {
    type: 'fusion_catalog', catalog_json: JSON.stringify(catalog),
  });
  const { admitted: _, ...missingAdmission } = scope;
  assert.throws(() => validateFusionCatalog({ ...catalog, scopes: [missingAdmission] }), /admission/);
  assert.throws(() => validateFusionCatalog({ ...catalog, scopes: [{ ...scope, admitted: 'false' }] }), /admission/);
  assert.throws(() => validateClientEvent({ type: 'fusion_catalog', catalog_json: '{broken' }));
});
