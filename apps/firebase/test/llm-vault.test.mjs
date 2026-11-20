import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readFileSync } from 'node:fs';
import { randomBytes } from 'node:crypto';
import { PRESETS, VaultError, baseUrl, keyHint, open, parseKeyring, providerId, seal, upsertInput } from '../src/llm-vault.mjs';

const k1 = randomBytes(32).toString('base64'), k2 = randomBytes(32).toString('base64');
const ring = parseKeyring(JSON.stringify({ current: 'k1', keys: { k1 } }));

test('vault: a sealed key opens for its owner and provider only', () => {
  const sealed = seal(ring, 'alice', 'deepseek', 'sk-test-0123456789');
  assert.equal(open(ring, 'alice', 'deepseek', sealed), 'sk-test-0123456789');
  assert.doesNotMatch(JSON.stringify(sealed), /sk-test/u);
  for (const [owner, provider] of [['bob', 'deepseek'], ['alice', 'openai']])
    assert.throws(() => open(ring, owner, provider, sealed), (error) => error instanceof VaultError && error.code === 'VAULT_UNREADABLE');
});

test('vault: a changed byte, an unknown key id or another master key does not open', () => {
  const sealed = seal(ring, 'alice', 'deepseek', 'sk-test-0123456789');
  const flip = (packed) => { const raw = Buffer.from(packed, 'base64'); raw[raw.length - 1] ^= 1; return raw.toString('base64'); };
  for (const broken of [{ ...sealed, data: flip(sealed.data) }, { ...sealed, dek: flip(sealed.dek) }, { ...sealed, kid: 'k9' }, { ...sealed, v: 2 }])
    assert.throws(() => open(ring, 'alice', 'deepseek', broken), { code: 'VAULT_UNREADABLE' });
  const other = parseKeyring(JSON.stringify({ current: 'k1', keys: { k1: k2 } }));
  assert.throws(() => open(other, 'alice', 'deepseek', sealed), { code: 'VAULT_UNREADABLE' });
});

test('vault: a rotated keyring seals with the new key and still opens old records', () => {
  const old = seal(ring, 'alice', 'deepseek', 'sk-old');
  const rotated = parseKeyring(JSON.stringify({ current: 'k2', keys: { k1, k2 } }));
  const fresh = seal(rotated, 'alice', 'deepseek', 'sk-new');
  assert.equal(fresh.kid, 'k2');
  assert.equal(open(rotated, 'alice', 'deepseek', old), 'sk-old');
  assert.equal(open(rotated, 'alice', 'deepseek', fresh), 'sk-new');
});

test('vault: the keyring must name a current 32-byte key', () => {
  for (const text of ['nope', '{}', JSON.stringify({ current: 'k1', keys: { k1: randomBytes(16).toString('base64') } }), JSON.stringify({ current: 'k2', keys: { k1 } })])
    assert.throws(() => parseKeyring(text));
});

test('vault: hints never carry the key', () => {
  assert.equal(keyHint('sk-ant-api03-abcdefghijklmnop-WXYZ'), 'sk-ant-…WXYZ');
  assert.equal(keyHint('AIzaSyD-0123456789abcd'), 'AIza…abcd');
  assert.equal(keyHint('short-key'), '…ey');
});

test('vault: provider ids follow the Desktop profile-name rule', () => {
  for (const ok of ['deepseek', 'my-proxy', 'a.b_c-1']) assert.equal(providerId.safeParse(ok).success, true, ok);
  for (const bad of ['', 'Deepseek', '-x', 'builtin', 'claude', '__proto__', 'a/b', 'x'.repeat(65)]) assert.equal(providerId.safeParse(bad).success, false, bad);
});

test('vault: a base URL is https to a public host name', () => {
  for (const ok of ['https://api.deepseek.com', 'https://my-gateway.example.com/v1', 'https://x.example.com:443/v1'])
    assert.equal(baseUrl.safeParse(ok).success, true, ok);
  for (const bad of ['http://api.deepseek.com', 'https://127.0.0.1/v1', 'https://[::1]/v1', 'https://localhost/v1', 'https://user:pw@api.example.com',
    'https://api.example.com/v1?key=1', 'https://api.example.com:8443', 'https://metadata.google.internal/', 'https://printer.local', 'https://intranet', 'ftp://x.example.com'])
    assert.equal(baseUrl.safeParse(bad).success, false, bad);
});

test('vault: an upsert rejects keys with whitespace or control characters and unknown fields', () => {
  assert.equal(upsertInput.safeParse({ action: 'upsert', id: 'deepseek', presetId: 'deepseek', apiKey: 'sk-abc' }).success, true);
  for (const apiKey of ['sk abc', 'sk-\nabc', 'x'.repeat(4097)]) assert.equal(upsertInput.safeParse({ action: 'upsert', id: 'deepseek', apiKey }).success, false);
  assert.equal(upsertInput.safeParse({ action: 'upsert', id: 'deepseek', apiKey: 'sk-abc', sealed: {} }).success, false);
});

test('presets: every preset matches the model catalog it was generated from', () => {
  const seed = JSON.parse(readFileSync(new URL('../src/seed.json', import.meta.url), 'utf8'));
  for (const preset of PRESETS) {
    const provider = seed.providers.find(entry => entry.id === preset.id);
    assert.ok(provider, preset.id);
    assert.equal(preset.protocol, provider.protocol, preset.id);
    assert.equal(preset.baseUrl, provider.baseUrl, preset.id);
    assert.deepEqual(preset.hosts, [new URL(provider.baseUrl).hostname]);
    const models = seed.models.filter(model => model.providerId === preset.id && model.status === 'active').map(model => model.id);
    assert.deepEqual(preset.models.map(model => model.id), models, preset.id);
    assert.ok(models.includes(preset.defaultModel), preset.id);
    assert.equal(baseUrl.safeParse(preset.baseUrl).success, true, preset.id);
    // Everything the Web shows or the proxy sends also follows the catalog.
    assert.equal(preset.label, provider.name, preset.id);
    assert.equal(preset.defaultModel, provider.defaultModelId, preset.id);
    assert.deepEqual(preset.regions, provider.regions, preset.id);
    const names = new Map(seed.models.filter(model => model.providerId === preset.id).map(model => [model.id, model.name]));
    for (const model of preset.models) assert.equal(model.name, names.get(model.id), `${preset.id}/${model.id}`);
    const definition = provider.executionDefinition.source;
    assert.equal(preset.keyUrl, /^api_key_url = "([^"]+)"/mu.exec(definition)?.[1], preset.id);
    if (preset.protocol === 'anthropic_messages')
      assert.equal(preset.auth === 'bearer', /^auth = "bearer"/mu.test(definition), `${preset.id} auth`);
  }
});
