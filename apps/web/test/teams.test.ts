import assert from 'node:assert/strict';
import test from 'node:test';
import { buildDeepLink, parseDeepLink } from '../../../packages/deep-links';
import { inviteCode, inviteLink, teamsTransport } from '../../../packages/teams';
import { resolveRoute, securityZone } from '../src/app/router';

test('Team invitations roundtrip without folding case or automatically joining', () => {
  const code = 'AbCdEfGhIjKlMnOpQrStUvWxYz_12345';
  assert.equal(code.length, 32);
  const target = { route: 'team/join', params: { code } };
  assert.deepEqual(parseDeepLink(buildDeepLink(target)), target);
  assert.deepEqual(parseDeepLink(buildDeepLink(target, 'scheme')), target);
  assert.equal(inviteCode(inviteLink(code)), code);
  assert.equal(inviteCode(`lingxi://team/join?code=${code}`), code);
  assert.equal(inviteCode(`https://lingxios.net/teams/join?code=${code}`), code);
  assert.equal(resolveRoute('/teams/join'), '/teams/join');
  assert.equal(securityZone('/teams'), 'teams');
  assert.notEqual(securityZone('/teams'), securityZone('/remote'));
});
test('Team invitations reject ambiguous or credential-bearing links', () => {
  const code = 'a'.repeat(32);
  for (const raw of [
    'lingxi://team/join', `lingxi://team/join?code=${code}&code=${code}`,
    `lingxi://team/join?code=${code}&workspace=project.secret`, `lingxi://team/join?code=${code}#payload=x`,
    `lingxi://session/abc?code=${code}`, `https://lingxios.net.evil/open/team/join?code=${code}`,
    `https://user:password@lingxios.net/open/team/join?code=${code}`, 'lingxi://team/join?code=short',
  ]) { assert.throws(() => parseDeepLink(raw)); assert.equal(inviteCode(raw), null, raw); }
});
test('Teams transport never redirects credentials and preserves server errors', async () => {
  let options: RequestInit | undefined;
  const request = teamsTransport('https://fixture.invalid', async () => 'synthetic-token', async (url, input) => {
    assert.equal(url, 'https://fixture.invalid/v1/teams'); options = input;
    return Response.json({ error: { code: 'STALE_MEMBERSHIP' } }, { status: 409 });
  });
  await assert.rejects(request('', { action: 'leave' }), { code: 'STALE_MEMBERSHIP', status: 409 });
  assert.equal(options?.redirect, 'error'); assert.equal(options?.cache, 'no-store');
});
