import assert from 'node:assert/strict';
import test from 'node:test';
import { readDefinition } from '../src/main/teams/dataSourceDefinition.js';

const uid = 'owner_1';
const keychain = (service: string, account = 'orders') => ({ keychain: { service, account } });

function definition(input: Record<string, unknown>, extra: Record<string, unknown> = {}): string {
  return JSON.stringify({
    owner: uid, source: 'orders', key: keychain('com.lingxi.data.host-key'),
    presets: { main: { fields: ['id', 'total', 'id'], keyField: 'id', input } }, ...extra,
  });
}

const rejected = (input: Record<string, unknown>, extra?: Record<string, unknown>, owner = uid) =>
  assert.throws(() => readDefinition(definition(input, extra), owner), /INVALID_INPUT/);

test('network databases read their connection only from the Desktop keychain item for their kind and source', () => {
  for (const [kind, field, rest] of [
    ['mysql', 'dsn', { view: 'orders_view' }],
    ['postgres', 'dsn', { schema: 'reporting', view: 'orders_view' }],
    ['mongodb', 'uri', { database: 'shop', collection: 'orders' }],
    ['redis', 'url', { prefix: 'order:', value: 'hash' }],
  ] as const) {
    const input = { kind, [field]: keychain(`com.lingxi.data.${kind}`), ...rest };
    assert.deepEqual(readDefinition(definition(input), uid), { source: 'orders', kind, fields: ['id', 'total'] });
    rejected({ ...input, isolatedLoopback: true });
    rejected({ ...input, [field]: keychain(kind === 'mysql' ? 'com.lingxi.data.postgres' : 'com.lingxi.data.mysql') });
    rejected({ ...input, [field]: keychain(`com.lingxi.data.${kind}`, 'other') });
    rejected({ ...input, [field]: { localTestFile: '/tmp/connection' } });
    rejected({ kind, ...rest });
  }
});

test('file inputs need an absolute path', () => {
  for (const input of [
    { kind: 'sqlite', path: '/data/shop.db', view: 'orders' },
    { kind: 'csv', path: '/data/orders.csv' },
    { kind: 'json', path: '/data/orders.json' },
  ]) {
    assert.equal(readDefinition(definition(input), uid).kind, input.kind);
    rejected({ ...input, path: 'data/orders' });
    rejected({ kind: input.kind });
  }
});

test('unknown kinds, test proxies, extra presets and other owners are rejected', () => {
  const csv = { kind: 'csv', path: '/data/orders.csv' };
  rejected({ kind: 'oracle', dsn: keychain('com.lingxi.data.oracle') });
  rejected({ kind: 'constructor', path: '/data/orders.csv' });
  rejected(csv, { proxy: { listen: '127.0.0.1:9443' } });
  rejected(csv, { presets: { main: { fields: ['id'], keyField: 'id', input: csv }, other: { fields: ['id'], keyField: 'id', input: csv } } });
  rejected(csv, {}, 'owner_2');
});
