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
  const binding = parseLocalAppDataBinding({ reference: { kind: 'dataset', dataset: 'orders' }, via: 'chat-grant' });
  const pageScope = { kind: 'personal' };
  assert.deepEqual(localAppDataRequest(binding, 'read', { scope: pageScope, action: 'get', dataset: 'other', via: 'other-grant',
    record: 'order', revision: 2, schemaVersion: 1 }, undefined), {
    protocol: 2, dataset: 'orders', via: 'chat-grant',
    command: { action: 'read', reference: { kind: 'record', dataset: 'orders', record: 'order', revision: 2, schemaVersion: 1 } },
  });
  const edit = { action: 'put', dataset: 'other', record: 'order', expectedRevision: 2, schemaVersion: 1, partition: 'east warehouse', value: { title: 'Packed' } };
  assert.throws(() => localAppDataRequest(binding, 'read', edit, 'write-1'), /read-only/);
  assert.throws(() => localAppDataRequest(binding, 'edit', edit, undefined), /Invalid data identifier/);
  assert.deepEqual(localAppDataRequest(binding, 'edit', edit, 'write-1'), {
    protocol: 2, dataset: 'orders', via: 'chat-grant', operation: 'write-1',
    command: { action: 'put', record: 'order', schemaVersion: 1, expectedRevision: 2, partition: 'east warehouse', value: { title: 'Packed' } },
  });
  assert.throws(() => localAppDataRequest(binding, 'edit', { action: 'sourcePreview' }, 'preview-1'), /Unsupported/);
});

test('A fixed Local App binding ignores page-supplied resource IDs and cannot escalate to live access', () => {
  const snapshot = parseLocalAppDataBinding({ reference: { kind: 'snapshot', dataset: 'orders', snapshot: 'fixed' }, via: 'share' });
  assert.deepEqual(localAppDataRequest(snapshot, 'read', { action: 'list', snapshot: 'another', block: 1 }, null), {
    protocol: 2, dataset: 'orders', via: 'share', command: { action: 'openSnapshot', snapshot: 'fixed', block: 1 },
  });
  assert.throws(() => localAppDataRequest(snapshot, 'edit', { action: 'put' }, 'write'), /requires a live/);
  assert.throws(() => parseLocalAppDataBinding({ reference: { kind: 'dataset', dataset: 'orders' }, scope: 'caller' }), /Unknown data binding property/);
});
