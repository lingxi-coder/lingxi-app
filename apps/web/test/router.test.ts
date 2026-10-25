import assert from 'node:assert/strict';
import test from 'node:test';
import { isRemoteRoute, normalizePath, parseRemoteRoute, remoteSessionRoute, resolveRoute, securityZone } from '../src/app/router';
import { docPageIds } from '../src/app/docRoutes';
import { docPages } from '../src/data/docs';

test('the lightweight documentation routes cover exactly the handbook pages', () => {
  const ids = docPages.map((page) => page.id);
  assert.equal(new Set(docPageIds).size, docPageIds.length, 'duplicate documentation routes');
  assert.deepEqual([...docPageIds].sort(), ids.sort(), 'update docRoutes.ts when adding or removing handbook pages');
  for (const id of docPageIds) {
    assert.equal(resolveRoute(`/docs/${id}`), `/docs/${id}`);
  }
});

test('normalizes duplicate slashes, query strings, casing, and trailing slashes', () => {
  assert.equal(normalizePath('CONSOLE//API-KEYS/?tab=all'), '/console/api-keys');
  assert.equal(normalizePath('/docs#quick-start'), '/docs');
});

test('resolves known routes and falls back to the public homepage', () => {
  assert.equal(resolveRoute('/console/usage'), '/console/usage');
  assert.equal(resolveRoute('/not-a-route'), '/');
  assert.equal(resolveRoute('/DOCS/LLM-CLIENT/?from=home'), '/docs/llm-client');
  assert.equal(resolveRoute('/docs/mobile-ios'), '/docs/mobile-ios');
  assert.equal(resolveRoute('/docs/no-such-sdk'), '/');
});


test('deep link route preserves case-sensitive resource identity', () => {
  assert.equal(resolveRoute('/open/session/AbC?workspace=project.X#payload=ignored'), '/open/session/AbC');
});

test('remote routes keep the case of host and session ids and reject other shapes', () => {
  assert.equal(resolveRoute('/remote'), '/remote');
  assert.equal(resolveRoute('/Remote/?from=push'), '/remote');
  assert.equal(resolveRoute('/remote/AbC_-xYz0123456789AAAA/sess%3AAb12?x=1#frag'), '/remote/AbC_-xYz0123456789AAAA/sess%3AAb12');
  assert.equal(resolveRoute('/remote/host/session/'), '/remote/host/session');
  assert.equal(resolveRoute('/remote/host'), '/', 'a host without a session is not a page');
  assert.equal(resolveRoute('/remote/host/session/extra'), '/');
  assert.equal(resolveRoute('/remote//session'), '/');
  assert.equal(resolveRoute('/login/remote'), '/login/remote');
});

test('remote session routes round-trip ids that need escaping', () => {
  const route = remoteSessionRoute('AbC_-xYz0123456789AAAA', 'sess:a/b 1');
  assert.equal(route, '/remote/AbC_-xYz0123456789AAAA/sess%3Aa%2Fb%201');
  assert.deepEqual(parseRemoteRoute(route), { hostId: 'AbC_-xYz0123456789AAAA', sessionId: 'sess:a/b 1' });
  assert.equal(resolveRoute(route), route);
  assert.equal(parseRemoteRoute('/remote/%E0%A4%A/x'), null, 'malformed escapes are not a route');
});

test('security zones separate the strict remote documents from the rest of the site', () => {
  assert.ok(isRemoteRoute('/remote') && isRemoteRoute('/remote/h/s'));
  assert.ok(!isRemoteRoute('/remote-control') && !isRemoteRoute('/login/remote') && !isRemoteRoute('/'));
  assert.equal(securityZone('/remote'), 'remote');
  assert.equal(securityZone('/remote/h/s'), 'remote');
  assert.equal(securityZone('/login/remote'), 'sign-in');
  assert.equal(securityZone('/docs'), 'site');
  assert.equal(securityZone('/admin/models'), 'site');
});
