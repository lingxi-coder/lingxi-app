import assert from 'node:assert/strict';
import test from 'node:test';

import { localAppDataRequest, parseLocalAppDataBinding } from '../src/main/local-apps/dataBinding.js';
import { parseManifest } from '../src/main/local-apps/manifest.js';

test('Local App published data aliases declare access without choosing the dataset', () => {
  const manifest = { schema: 2, name: 'Orders', entry: { type: 'static', path: 'index.html' }, targets: [{ platform: 'macos', environment: 'native' }] };
  assert.equal(parseManifest({ ...manifest, dataBindings: { orders: 'edit' } }).dataBindings?.orders, 'edit');
  assert.throws(() => parseManifest({ ...manifest, dataBindings: { orders: { dataset: 'private' } } }), /Invalid data binding declaration/);
  assert.throws(() => parseManifest({ ...manifest, dataBindings: { 'orders/other': 'read' } }), /Invalid data binding declaration/);
  assert.equal(parseManifest(manifest).dataBindings, undefined);
});

test('Local App data commands use the host binding and only edit grants can write', () => {
  const binding = parseLocalAppDataBinding({ dataset: 'orders', scope: { kind: 'team', teamId: 'team', conversationId: 'thread', generation: 4 } });
  const pageScope = { kind: 'personal' };
  assert.deepEqual(localAppDataRequest(binding, 'read', { scope: pageScope, action: 'get', dataset: 'other', record: 'order', revision: 2, schemaVersion: 1 }, undefined), {
    scope: binding.scope, command: { action: 'get', reference: { dataset: 'orders', record: 'order', revision: 2, schemaVersion: 1 } },
  });
  const edit = { action: 'put', dataset: 'other', record: 'order', expectedRevision: 2, schemaVersion: 1, value: { title: 'Packed' } };
  assert.throws(() => localAppDataRequest(binding, 'read', edit, 'write-1'), /read-only/);
  assert.throws(() => localAppDataRequest(binding, 'edit', edit, undefined), /Invalid data identifier/);
  assert.deepEqual(localAppDataRequest(binding, 'edit', edit, 'write-1'), {
    scope: binding.scope, operation: 'write-1',
    command: { action: 'put', dataset: 'orders', record: 'order', expectedRevision: 2, schemaVersion: 1, value: { title: 'Packed' } },
  });
  assert.throws(() => localAppDataRequest(binding, 'edit', { action: 'sourcePreview' }, 'preview-1'), /Unsupported/);
});
