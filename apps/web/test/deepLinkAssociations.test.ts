import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

test('association artifacts use configured iOS and verified direct Android identities', () => {
  const ios = JSON.parse(readFileSync(new URL('../public/.well-known/apple-app-site-association', import.meta.url), 'utf8'));
  const details = ios.applinks.details;
  assert.deepEqual(details.flatMap((item: { appIDs: string[] }) => item.appIDs), ['AZ4AX7J833.com.lingxi.code', 'AZ4AX7J833.com.lingxi.code.full']);
  const project = readFileSync(new URL('../../ios/native/project.yml', import.meta.url), 'utf8');
  assert.match(project, /DEVELOPMENT_TEAM: "AZ4AX7J833"/);
  for (const id of ['com.lingxi.code', 'com.lingxi.code.full']) assert.ok(project.includes(`PRODUCT_BUNDLE_IDENTIFIER: ${id}`));
  const android: Array<{ target: { sha256_cert_fingerprints: string[] } }> = JSON.parse(readFileSync(new URL('../public/.well-known/assetlinks.json', import.meta.url), 'utf8'));
  // Only the public certificate verified for direct distribution is present.
  // A local release key is not evidence of the Play App Signing identity.
  assert.deepEqual(android, [{ relation: ['delegate_permission/common.handle_all_urls'], target: { namespace: 'android_app', package_name: 'com.lingxi.code.direct', sha256_cert_fingerprints: ['88:61:45:6D:EC:29:98:66:09:86:8D:27:94:2D:12:33:29:AE:5E:2C:A3:AA:43:20:54:B6:6A:34:02:98:46:54'] } }]);
  const firebase = JSON.parse(readFileSync(new URL('../../../firebase.json', import.meta.url), 'utf8'));
  assert.equal(firebase.hosting.site, 'lingxi-5e4b2');
  assert.ok(!firebase.hosting.ignore.includes('**/.*'));
  assert.ok(firebase.hosting.headers.some((entry: { source: string; headers: Array<{key: string; value: string}> }) => entry.source === '/.well-known/**' && entry.headers.some(header => header.key === 'Content-Type' && header.value === 'application/json')));
});

test('release preflight rejects empty, partial, debug and unverified Android associations', async () => {
  const { validateAndroidReleaseAssociations } = await import('../scripts/verify-deep-link-release.mjs');
  const fingerprint = Array(32).fill('AB').join(':');
  const make = (name: string) => ({ relation: ['delegate_permission/common.handle_all_urls'], target: { namespace: 'android_app', package_name: name, sha256_cert_fingerprints: [fingerprint] } });
  const certificates = new Map([['com.lingxi.code', new Set([fingerprint])], ['com.lingxi.code.direct', new Set([fingerprint])]]);
  assert.throws(() => validateAndroidReleaseAssociations([], certificates), /no verified signing entries/);
  assert.throws(() => validateAndroidReleaseAssociations([make('com.lingxi.code.debug')], certificates), /configured production package/);
  assert.throws(() => validateAndroidReleaseAssociations([make('com.lingxi.code')], new Map()), /not verified/);
  assert.throws(() => validateAndroidReleaseAssociations([make('com.lingxi.code')], certificates), /Missing production association/);
  validateAndroidReleaseAssociations([make('com.lingxi.code'), make('com.lingxi.code.direct')], certificates);
  validateAndroidReleaseAssociations([make('com.lingxi.code.direct')], certificates, 'direct');
  assert.throws(() => validateAndroidReleaseAssociations([make('com.lingxi.code.direct')], certificates), /Play App Signing/);
  assert.throws(() => validateAndroidReleaseAssociations([make('com.lingxi.code.direct')], new Map(), 'play'), /Play App Signing/);
});
