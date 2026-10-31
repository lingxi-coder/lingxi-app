import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { remoteBotRoute, remoteSessionRoute } from '../src/app/router';

const here = path.dirname(fileURLToPath(import.meta.url));
const config = JSON.parse(readFileSync(path.join(here, '../../../firebase.json'), 'utf8')) as {
  hosting: {
    headers: Array<{ source: string; headers: Array<{ key: string; value: string }> }>;
    rewrites: Array<{ source: string; destination?: string; function?: unknown }>;
  };
};
const indexHtml = readFileSync(path.join(here, '../index.html'), 'utf8');

const policyFor = (source: string): string | undefined =>
  config.hosting.headers.find((entry) => entry.source === source)?.headers.find((header) => header.key === 'Content-Security-Policy')?.value;
const directives = (policy: string): Map<string, string[]> =>
  new Map(policy.split(';').map((part) => part.trim().split(/\s+/u)).filter((words) => words[0]).map(([name, ...values]) => [name!, values]));

test('/remote and /remote/** share one strict policy', () => {
  const strict = policyFor('/remote');
  assert.ok(strict, 'no policy for /remote');
  assert.equal(policyFor('/remote/**'), strict);
});

test('the strict policy allows only what the viewer needs', () => {
  const policy = directives(policyFor('/remote')!);
  assert.deepEqual(policy.get('default-src'), ["'none'"]);
  for (const name of ['object-src', 'base-uri', 'form-action', 'frame-src', 'frame-ancestors']) assert.deepEqual(policy.get(name), ["'none'"], name);
  assert.deepEqual(policy.get('style-src'), ["'self'"]);
  assert.deepEqual(policy.get('font-src'), ["'self'"]);
  assert.deepEqual(policy.get('img-src'), ["'self'", 'data:', 'blob:']);
  assert.deepEqual(policy.get('connect-src'), [
    'https://lingxi-5e4b2-default-rtdb.firebaseio.com',
    'https://identitytoolkit.googleapis.com',
    'https://securetoken.googleapis.com',
  ]);
  for (const [name, values] of policy) {
    assert.ok(!values.includes("'unsafe-inline'") && !values.includes("'unsafe-eval'") && !values.includes('*'), `${name} is too permissive`);
  }
});

test('the one inline script of index.html is allowed by its hash, and nothing else is inline', () => {
  const scripts = [...indexHtml.matchAll(/<script(?<attributes>[^>]*)>(?<body>[\s\S]*?)<\/script>/gu)].filter((match) => !/\bsrc=/u.test(match.groups!.attributes!));
  assert.equal(scripts.length, 1, 'expected exactly one inline script');
  const hash = `'sha256-${createHash('sha256').update(scripts[0]!.groups!.body!, 'utf8').digest('base64')}'`;
  assert.ok(directives(policyFor('/remote')!).get('script-src')!.includes(hash), `the policy does not allow ${hash}; update firebase.json after changing the script`);
  assert.deepEqual(directives(policyFor('/remote')!).get('script-src')!.filter((value) => value.startsWith("'sha256-")), [hash]);
  assert.ok(!/<style[\s>]/u.test(indexHtml) && !/\sstyle=/u.test(indexHtml), 'inline styles would be blocked by style-src');
});

test('the sign-in page has its own, lighter policy and the strict one does not apply to it', () => {
  const light = policyFor('/login/remote');
  assert.ok(light);
  assert.ok(!directives(light).has('default-src'), 'a default-src would block the sign-in popup helpers');
  assert.deepEqual(directives(light).get('frame-ancestors'), ["'none'"]);
});

test('security zones have disjoint policy patterns', () => {
  const withPolicy = config.hosting.headers.filter((entry) => entry.headers.some((header) => header.key === 'Content-Security-Policy')).map((entry) => entry.source);
  assert.deepEqual(withPolicy, ['/remote', '/remote/**', '/login/remote', '/teams', '/teams/**']);
});

test('Teams alone gets Firestore connectivity and invitation links suppress referrers', () => {
  const teamPolicy = policyFor('/teams');
  assert.ok(teamPolicy);
  assert.equal(policyFor('/teams/**'), teamPolicy);
  assert.ok(directives(teamPolicy).get('connect-src')!.includes('https://firestore.googleapis.com'));
  assert.ok(!directives(policyFor('/remote')!).get('connect-src')!.includes('https://firestore.googleapis.com'));
  for (const source of ['/teams', '/teams/**', '/open/team/join']) {
    assert.equal(config.hosting.headers.find(entry => entry.source === source)?.headers.find(header => header.key === 'Referrer-Policy')?.value, 'no-referrer');
  }
});

// Firebase Hosting's globs, as far as firebase.json uses them: `**` crosses `/`, `*` does not.
const escapeLiteral = (text: string): string => text.replace(/[.+?^${}()|[\]\\]/gu, '\\$&');
const globToRegExp = (glob: string): RegExp => {
  const body = glob.split('**').map((part) => part.split('*').map(escapeLiteral).join('[^/]*')).join('.*');
  return new RegExp(`^${body}$`, 'u');
};
const matches = (source: string, route: string): boolean => globToRegExp(source).test(route);

test('membership requests reach teamsApi before the general catalog rewrite', () => {
  for (const route of ['/v1/teams', '/v1/teams/session', '/v1/teams/team-test']) {
    assert.deepEqual(config.hosting.rewrites.find(entry => matches(entry.source, route))?.function,
      { functionId: 'teamsApi', region: 'us-central1' });
  }
});

test('the globs of firebase.json are read as Hosting reads them', () => {
  assert.ok(matches('/remote/**', '/remote/a/b/c') && matches('/remote/**', '/remote/a'));
  assert.ok(!matches('/remote/**', '/remote') && !matches('/remote', '/remote/a') && !matches('/remote/**', '/remote-control/a'));
  assert.ok(matches('**', '/') && matches('**', '/anything/at/all'));
  assert.ok(matches('/assets/*', '/assets/a.js') && !matches('/assets/*', '/assets/a/b.js'));
});

test('a Bot page is served the app and the strict policy, the same as a session page', () => {
  const botPage = remoteBotRoute('AbC_-xYz0123456789AAAA', 'bot:a/b 1');
  const sessionPage = remoteSessionRoute('AbC_-xYz0123456789AAAA', 'sess:a/b 1');
  for (const route of [botPage, sessionPage]) {
    const withPolicy = config.hosting.headers
      .filter((entry) => matches(entry.source, route) && entry.headers.some((header) => header.key === 'Content-Security-Policy'))
      .map((entry) => entry.source);
    assert.deepEqual(withPolicy, ['/remote/**'], route);
    const rewrite = config.hosting.rewrites.find((entry) => matches(entry.source, route));
    assert.deepEqual(rewrite, { source: '**', destination: '/index.html' }, `${route} is answered with the app, not with a function`);
  }
});
