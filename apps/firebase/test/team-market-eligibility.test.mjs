import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTeamMarketEligibilityHandler } from '../src/team-market-eligibility.mjs';
const settings = { serviceEmail: 'market-service@example.iam.gserviceaccount.com', serviceSubject: '100000001',
  serviceAudience: 'https://eligibility.example.run.app', projectId: 'lingxi-5e4b2', tenantId: null };
const claims = { iss: 'https://accounts.google.com', sub: settings.serviceSubject, aud: settings.serviceAudience,
  email: settings.serviceEmail, email_verified: true };
const input = { schemaVersion: 1, principal: { uid: 'fixture-member', issuer: 'https://securetoken.google.com/lingxi-5e4b2',
  audience: 'lingxi-5e4b2', tenantId: null }, teamId: 'team_0001', operation: 'read', requestDigest: 'a'.repeat(64) };
const request = (body = input, token = 'service-token', path = '/internal/v1/market/eligibility') => new Request(`https://fixture.local${path}`,
  { method: 'POST', headers: { Authorization: `Bearer ${token}` }, body: JSON.stringify(body) });
function fixture(overrides = {}) {
  let calls = 0;
  const handler = createTeamMarketEligibilityHandler({ config: () => settings,
    verifyServiceToken: async token => { if (token !== 'service-token') throw new Error('no'); return claims; },
    store: { async eligibility(teamId, uid) { calls++; assert.equal(teamId, input.teamId); assert.equal(uid, input.principal.uid);
      return { schemaVersion: 1, eligible: true, memberGeneration: calls, policyRevision: calls }; } }, ...overrides });
  return { handler, calls: () => calls };
}
test('market eligibility: exact verified service identity can query current Team generations without a positive cache', async () => {
  const f = fixture();
  for (let i = 1; i <= 2; i++) { const response = await f.handler(request()); assert.equal(response.status, 200);
    assert.equal((await response.json()).memberGeneration, i); assert.equal(response.headers.get('cache-control'), 'no-store'); }
  assert.equal(f.calls(), 2);
});
test('market eligibility: browser tokens, forged services, wrong subjects/audiences/issuers and unverified email fail before membership lookup', async () => {
  const invalid = [null, { ...claims, iss: 'https://securetoken.google.com/lingxi-5e4b2' },
    { ...claims, sub: 'other-service' }, { ...claims, email: 'other@example.com' }, { ...claims, aud: 'different-service' }, { ...claims, email_verified: false }];
  for (const value of invalid) {
    const f = fixture({ verifyServiceToken: async () => value }); assert.equal((await f.handler(request())).status, 403); assert.equal(f.calls(), 0);
  }
  const f = fixture(); assert.equal((await f.handler(request(input, 'firebase-user-token'))).status, 401); assert.equal(f.calls(), 0);
});
test('market eligibility: wrong viewer project/issuer/tenant and caller-injected membership facts are rejected', async () => {
  for (const principal of [{ ...input.principal, issuer: 'https://evil.example' }, { ...input.principal, audience: 'other-project' }, { ...input.principal, tenantId: 'other-tenant' }]) {
    const f = fixture(); assert.equal((await f.handler(request({ ...input, principal }))).status, 403); assert.equal(f.calls(), 0);
  }
  const f = fixture();
  assert.equal((await f.handler(request({ ...input, memberGeneration: 3 }))).status, 400);
  assert.equal((await f.handler(request({ ...input, principal: { ...input.principal, email: 'same@example.com' } }))).status, 400);
  assert.equal(f.calls(), 0);
});
test('market eligibility: absent production allowlist and other paths remain closed without CORS', async () => {
  const f = fixture({ config: () => ({ projectId: settings.projectId, tenantId: null }) });
  const response = await f.handler(request()); assert.equal(response.status, 503); assert.equal(f.calls(), 0);
  assert.equal(response.headers.has('access-control-allow-origin'), false);
  assert.equal((await fixture().handler(request(input, 'service-token', '/v1/teams'))).status, 404);
});
