import assert from 'node:assert/strict';
import { test } from 'node:test';
import { editableProviderDraft, trimProviderDraft, renameProviderModel, removeProviderModel, canRemoveProviderModel } from '../src/renderer/components/settings/pages/customProviderDraft';
import { mergeProviderImport, validateImportEntry } from '../src/renderer/components/settings/pages/customProviderImport';

test('editable drafts repair null and legacy string models without losing advanced fields', () => {
  assert.equal(editableProviderDraft(null).type, 'openai');
  assert.deepEqual(editableProviderDraft({ type: 'openai', models: [' x ', null], billingMode: 'perToken' }), { type: 'openai', models: [{ id: ' x ' }, { id: '' }], billingMode: 'perToken' });
});
test('save and import trim editable identifiers while preserving advanced fields', () => {
  const draft = { type: 'openai', baseUrl: ' https://example.com/v1 ', apiKeyEnv: ' MODEL_KEY ', models: [{ id: ' model-one ', aliases: ['alias'], capabilities: { vision: true } }], billingMode: 'perToken' };
  const normalized = trimProviderDraft(draft);
  assert.equal(normalized.baseUrl, 'https://example.com/v1');
  assert.equal(normalized.apiKeyEnv, 'MODEL_KEY');
  assert.deepEqual(normalized.models[0], { id: 'model-one', aliases: ['alias'], capabilities: { vision: true } });
  assert.equal(normalized.billingMode, 'perToken');
  assert.equal(draft.models[0].id, ' model-one ');
  const entry = { name: 'custom', draft, selected: true, conflict: false, diagnostics: [] };
  assert.equal(validateImportEntry(entry), null);
  assert.deepEqual(mergeProviderImport({}, [entry]).providers.custom, normalized);
});

test('model rename and removal migrate pricing without changing advanced configuration', () => {
  const draft = { type: 'openai', models: [{ id: 'old', aliases: ['alias'] }, { id: 'other' }], pricing: { old: { inputPerMtok: 2 }, other: { outputPerMtok: 3 } }, billingMode: 'perToken' };
  const renamed = renameProviderModel(draft, 0, ' new ');
  assert.deepEqual(renamed.draft.pricing, { new: { inputPerMtok: 2 }, other: { outputPerMtok: 3 } });
  assert.deepEqual(renamed.draft.models[0].aliases, ['alias']);
  assert.equal(renamed.draft.billingMode, 'perToken');
  assert.deepEqual(removeProviderModel(renamed.draft, 0, renamed.pricingId).pricing, { other: { outputPerMtok: 3 } });
  assert.deepEqual(draft.pricing.old, { inputPerMtok: 2 });
});
test('temporary blank or duplicate IDs retain recoverable prices and never overwrite a target', () => {
  const draft = { type: 'openai', models: [{ id: 'old' }, { id: 'other' }], pricing: { old: { inputPerMtok: 2 }, other: { inputPerMtok: 8 } } };
  const blank = renameProviderModel(draft, 0, '');
  const duplicate = renameProviderModel(blank.draft, 0, 'other', blank.pricingId);
  assert.deepEqual(duplicate.draft.pricing, draft.pricing);
  const restored = renameProviderModel(duplicate.draft, 0, 'new', duplicate.pricingId);
  assert.deepEqual(restored.draft.pricing, { new: { inputPerMtok: 2 }, other: { inputPerMtok: 8 } });
  assert.deepEqual(removeProviderModel(duplicate.draft, 0, duplicate.pricingId).pricing, { other: { inputPerMtok: 8 } });
  const conflicting = renameProviderModel({ ...draft, pricing: { ...draft.pricing, new: { inputPerMtok: 9 } } }, 0, 'new');
  assert.deepEqual(conflicting.draft.pricing, { ...draft.pricing, new: { inputPerMtok: 9 } });
});

test('a duplicate edit cannot delete its pricing target until the source is resolved', () => {
  const draft = { type: 'openai', models: [{ id: 'old' }, { id: 'other' }], pricing: { old: { inputPerMtok: 2 }, other: { inputPerMtok: 8 } } };
  const duplicate = renameProviderModel(draft, 0, 'other');
  const owners = [duplicate.pricingId, 'other'];
  assert.equal(canRemoveProviderModel(duplicate.draft, 1, owners), false);
  assert.deepEqual(removeProviderModel(duplicate.draft, 1, owners[1], owners), duplicate.draft);
  assert.equal(canRemoveProviderModel(duplicate.draft, 0, owners), true);
  assert.deepEqual(removeProviderModel(duplicate.draft, 0, owners[0], owners).pricing, { other: { inputPerMtok: 8 } });
  const resolved = renameProviderModel(duplicate.draft, 0, 'new', owners[0]);
  assert.equal(canRemoveProviderModel(resolved.draft, 1, [resolved.pricingId, 'other']), true);
  assert.deepEqual(removeProviderModel(resolved.draft, 1, 'other', [resolved.pricingId, 'other']).pricing, { new: { inputPerMtok: 2 } });
});
