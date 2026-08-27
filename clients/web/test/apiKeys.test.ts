import assert from 'node:assert/strict';
import test from 'node:test';
import { createLocalApiKey, deleteApiKey, maskApiKeySecret, renameApiKey, setApiKeyStatus } from '../src/utils/apiKeys';

test('masks secrets while retaining only a bounded prefix and suffix', () => {
  const secret = 'lx_live_abcdefghijklmnop';
  const masked = maskApiKeySecret(secret);
  assert.equal(masked, 'lx_live••••mnop');
  assert.equal(masked.includes('abcdefgh'), false);
  assert.equal(maskApiKeySecret('short'), '••••••••');
});

test('creates a one-time secret and a separately masked record', () => {
  const created = createLocalApiKey('CI', 'Responses only', '2026-08-27');
  assert.match(created.secret, /^lx_live_/);
  assert.notEqual(created.secret, created.record.maskedSecret);
  assert.equal(created.record.lastUsedAt, 'Never');
});

test('renames, disables, and deletes records immutably', () => {
  const created = createLocalApiKey('CI', 'Full API', '2026-08-27').record;
  const original = [created];
  const renamed = renameApiKey(original, created.id, 'Production');
  const disabled = setApiKeyStatus(renamed, created.id, 'disabled');
  assert.equal(original[0].name, 'CI');
  assert.equal(disabled[0].name, 'Production');
  assert.equal(disabled[0].status, 'disabled');
  assert.deepEqual(deleteApiKey(disabled, created.id), []);
});
