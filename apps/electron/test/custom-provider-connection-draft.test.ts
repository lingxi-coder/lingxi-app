import assert from 'node:assert/strict';
import test from 'node:test';
import { addProviderConnection, providerConnections, removeProviderConnection, updateProviderConnection } from '../src/renderer/components/settings/pages/customProviderDraft';

const flat = { type: 'openai', baseUrl: 'https://api.example.com/v1', apiKeyEnv: 'EXAMPLE_KEY', models: [{ id: 'm' }] };

test('a flat provider reports no connections', () => {
  assert.deepEqual(providerConnections(flat), []);
});

test('the first add migrates existing settings into a "default" connection', () => {
  const next = addProviderConnection(flat, 'cn');
  assert.deepEqual(next.connections, [
    { id: 'default', baseUrl: 'https://api.example.com/v1', apiKeyEnv: 'EXAMPLE_KEY' },
    { id: 'cn' },
  ]);
  // Moved DOWN, not copied: a new connection must not silently inherit the old URL.
  assert.equal(next.baseUrl, undefined);
  assert.equal(next.apiKeyEnv, undefined);
  // Everything not connection-specific stays at provider level.
  assert.deepEqual(next.models, flat.models);
  assert.equal(next.type, 'openai');
});

test('later adds append without disturbing existing connections', () => {
  const two = addProviderConnection(flat, 'cn');
  const three = addProviderConnection(two, 'eu');
  assert.deepEqual((three.connections ?? []).map((c) => c.id), ['default', 'cn', 'eu']);
  assert.deepEqual(three.connections?.[0], two.connections?.[0]);
});

test('updating one connection leaves its siblings untouched', () => {
  const two = addProviderConnection(flat, 'cn');
  const next = updateProviderConnection(two, 1, { baseUrl: 'https://cn.example.com/v1' });
  assert.equal(next.connections?.[1].baseUrl, 'https://cn.example.com/v1');
  assert.deepEqual(next.connections?.[0], two.connections?.[0]);
});

test('patching a key to undefined removes it rather than storing undefined', () => {
  const two = addProviderConnection(flat, 'cn');
  const next = updateProviderConnection(two, 0, { apiKeyEnv: undefined });
  assert.ok(!('apiKeyEnv' in (next.connections?.[0] ?? {})));
});

test('removing down to one connection collapses back to a flat provider', () => {
  const two = addProviderConnection(flat, 'cn');
  const next = removeProviderConnection(two, 1);
  assert.equal(next.connections, undefined, 'a one-entry list would still claim several connections');
  // The survivor's fields are lifted back to provider level, not dropped.
  assert.equal(next.baseUrl, 'https://api.example.com/v1');
  assert.equal(next.apiKeyEnv, 'EXAMPLE_KEY');
});

test('removing from three leaves a real list', () => {
  const three = addProviderConnection(addProviderConnection(flat, 'cn'), 'eu');
  const next = removeProviderConnection(three, 0);
  assert.deepEqual((next.connections ?? []).map((c) => c.id), ['cn', 'eu']);
});

test('collapsing also drops the now-meaningless fallback policy', () => {
  const two = { ...addProviderConnection(flat, 'cn'), fallback: { on: ['rate_limit'] } };
  const next = removeProviderConnection(two, 1);
  assert.equal(next.fallback, undefined);
});

test('trimming normalizes connection strings, not just provider-level ones', async () => {
  const { trimProviderDraft } = await import('../src/renderer/components/settings/pages/customProviderDraft');
  const draft = {
    type: 'openai',
    models: [{ id: ' m ' }],
    connections: [{ id: ' cn ', baseUrl: ' https://cn.example.com/v1 ', apiKeyEnv: ' CN_KEY ' }],
  };
  const next = trimProviderDraft(draft);
  // A stray space in the id would change the profile name the engine derives.
  assert.deepEqual(next.connections, [{ id: 'cn', baseUrl: 'https://cn.example.com/v1', apiKeyEnv: 'CN_KEY' }]);
  assert.equal(next.models[0].id, 'm');
});

test('trimming a flat provider still leaves connections absent', async () => {
  const { trimProviderDraft } = await import('../src/renderer/components/settings/pages/customProviderDraft');
  const next = trimProviderDraft({ type: 'openai', baseUrl: ' https://x ', models: [{ id: 'm' }] });
  assert.ok(!('connections' in next));
});
