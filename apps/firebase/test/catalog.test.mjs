import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { validateCatalog, selectCatalog, compareVersions, createHandler, ApiError } from '../src/catalog.mjs';

const seed = JSON.parse(readFileSync(new URL('../src/seed.json', import.meta.url), 'utf8'));
const sample = () => ({ schemaVersion: 1, providers: [structuredClone(seed.providers[0])], models: [structuredClone(seed.models[0])] });
const admin = async () => ({ uid: 'admin', model_catalog_admin: true, email_verified: true });
const request = (path, init) => new Request(`https://api.test${path}`, init);

test('SDK seed preserves all provider/model identities and fits bounded payload size', () => {
  const c = validateCatalog(seed); assert.equal(c.providers.length, 46); assert.equal(c.models.length, 1327);
  for (const p of c.providers) {
    assert.equal(p.executionDefinition.format, 'llm-client-preset-toml');
    assert.equal(createHash('sha256').update(p.executionDefinition.source, 'utf8').digest('hex'), p.executionDefinition.sha256);
  }
});
test('execution definitions survive filtering and public JSON without lossy projection', async () => {
  const c = sample();
  const source = '# 完整 SDK 定义 🧭\r\ninference = { reasoning = "thinking_type" }\r\n[pricing.peak]\r\nutc_windows = ["01:00-04:00"]\r\n';
  c.providers[0].executionDefinition = { format: 'llm-client-preset-toml', source, sha256: createHash('sha256').update(source).digest('hex') };
  const validated = validateCatalog(c);
  assert.equal(selectCatalog(validated, 'android', '1.0.0').providers[0].executionDefinition.source, source);
  const handle = createHandler({ current: async () => ({ id: 'release', createdAt: 1, catalog: validated }) }, admin, []);
  const response = await handle(request('/v1/model-catalog?platform=android&appVersion=1.0.0&schemaVersion=1'));
  assert.equal(response.status, 200);
  assert.equal((await response.json()).providers[0].executionDefinition.source, source);
  validated.providers[0].executionDefinition.source += '# altered';
  assert.throws(() => validateCatalog(validated), /EXECUTION_DEFINITION_HASH_MISMATCH/);
});
test('semver range comparison handles prereleases, numeric identifiers, and build metadata', () => {
  assert.ok(compareVersions('1.2.0', '1.1.99') > 0);
  assert.ok(compareVersions('1.0.0-alpha-beta.2', '1.0.0-alpha-beta.11') < 0);
  assert.ok(compareVersions('1.0.0-rc.1', '1.0.0') < 0);
  assert.equal(compareVersions('1.0.0+build.1', '1.0.0+build.2'), 0);
  assert.throws(() => compareVersions('1.0.0-01', '1.0.0'));
  assert.throws(() => compareVersions('9007199254740992.0.0', '1.0.0'));
});
test('catalog validation rejects credentials, duplicate identities, missing providers and invalid limits', () => {
  let c = sample(); c.models[0].apiKey = 'must-not-be-published'; assert.throws(() => validateCatalog(c));
  c = sample(); c.models.push(c.models[0]); assert.throws(() => validateCatalog(c), /DUPLICATE_MODEL/);
  c = sample(); c.models[0].providerId = 'missing'; assert.throws(() => validateCatalog(c), /UNKNOWN_PROVIDER/);
  c = sample(); c.models[0].minAppVersion = '2.0.0'; c.models[0].maxAppVersion = '1.0.0'; assert.throws(() => validateCatalog(c), /INVALID_VERSION_RANGE/);
  c = sample(); c.models[0].maxOutputTokens = c.models[0].contextWindow + 1; assert.throws(() => validateCatalog(c), /OUTPUT_EXCEEDS_CONTEXT/);
});
test('platform and version filtering uses inclusive bounds and respects provider disablement', () => {
  const c = sample(); Object.assign(c.models[0], { platforms: ['android'], minAppVersion: '1.0.0', maxAppVersion: '2.0.0' });
  assert.equal(selectCatalog(c, 'desktop', '1.0.0').models.length, 0);
  assert.equal(selectCatalog(c, 'android', '1.0.0-rc.1').models.length, 0);
  assert.equal(selectCatalog(c, 'android', '1.0.0').models.length, 1);
  assert.equal(selectCatalog(c, 'android', '2.0.0').models.length, 1);
  assert.equal(selectCatalog(c, 'android', '2.0.1').models.length, 0);
  c.providers[0].enabled = false; assert.equal(selectCatalog(c, 'android', '1.0.0').models.length, 0);
});
test('public API returns stable ETags, rejects unsupported schemas and never exposes a draft', async () => {
  const service = { current: async () => ({ id: 'release', createdAt: 1, catalog: sample() }) };
  const handle = createHandler(service, admin, ['https://lingxios.net']);
  const path = '/v1/model-catalog?platform=desktop&appVersion=1.0.0&schemaVersion=1';
  const first = await handle(request(path)); assert.equal(first.status, 200); assert.ok(first.headers.get('ETag'));
  const cached = await handle(request(path, { headers: { 'If-None-Match': first.headers.get('ETag') } })); assert.equal(cached.status, 304);
  assert.equal((await handle(request('/v1/model-catalog?platform=desktop&appVersion=1.0.0&schemaVersion=2'))).status, 400);
  service.current = async () => null; assert.equal((await handle(request(path))).status, 404);
});
test('admin API verifies token, verified email, exact claim and allowed browser origin', async () => {
  const service = { draft: async () => ({ revision: 0, catalog: sample() }) };
  const handle = createHandler(service, admin, ['https://lingxios.net']);
  assert.equal((await handle(request('/v1/admin/models'))).status, 401);
  assert.equal((await handle(request('/v1/admin/models', { headers: { Authorization: 'Bearer test', Origin: 'https://evil.test' } }))).status, 403);
  const ordinary = createHandler(service, async () => ({ uid: 'normal', email_verified: true }), []);
  assert.equal((await ordinary(request('/v1/admin/models', { headers: { Authorization: 'Bearer test' } }))).status, 403);
  const revoked = createHandler(service, async () => { throw Error('revoked'); }, []);
  assert.equal((await revoked(request('/v1/admin/models', { headers: { Authorization: 'Bearer test' } }))).status, 401);
  assert.equal((await handle(request('/v1/admin/models', { headers: { Authorization: 'Bearer test', Origin: 'https://lingxios.net' } }))).status, 200);
});
test('draft conflicts remain actionable HTTP 409 rather than silently overwriting', async () => {
  const handle = createHandler({ save: async () => { throw new ApiError(409, 'REVISION_CONFLICT'); } }, admin, []);
  const response = await handle(request('/v1/admin/models', { method: 'PUT', headers: { Authorization: 'Bearer test' }, body: JSON.stringify({ expectedRevision: 0, catalog: sample() }) }));
  assert.equal(response.status, 409); assert.equal((await response.json()).error.code, 'REVISION_CONFLICT');
});
