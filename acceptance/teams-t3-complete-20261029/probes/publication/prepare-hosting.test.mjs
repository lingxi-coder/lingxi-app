import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtempSync, writeFileSync, symlinkSync, rmSync, realpathSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { BASELINE_VERSION, TEAM_ENTRY, buildHostingPlan, canonicalHash, hostingFileHash, readRegular } from './prepare-hosting.mjs';

const digest = letter => letter.repeat(64);
const version = { name: BASELINE_VERSION, status: 'FINALIZED', fileCount: '4', config: {
  headers: [{ glob: '**', headers: { 'X-Content-Type-Options': 'nosniff' } }],
  rewrites: [{ glob: '/v1/**', run: { serviceId: 'catalogapi', region: 'us-central1' } }, { glob: '**', path: '/index.html' }],
  redirects: [{ glob: '/old', location: '/docs', statusCode: 301 }], cleanUrls: true,
} };
const files = [{ path: '/index.html', hash: digest('a'), status: 'ACTIVE' }, { path: '/docs/manual.html', hash: digest('b'), status: 'ACTIVE' },
  { path: '/assets/live-12345678.js', hash: digest('c'), status: 'ACTIVE' }, { path: TEAM_ENTRY, hash: digest('d'), status: 'ACTIVE' }];
const firebaseConfig = { hosting: { site: 'lingxi-5e4b2', headers: ['/teams', '/teams/**', '/open/team/join'].map(source => ({ source, headers: [{ key: 'Cache-Control', value: 'no-store' }] })) } };
const additions = [{ path: '/assets/current-12345678.js', hash: digest('e') }, { path: TEAM_ENTRY, hash: digest('f') }];
const build = overrides => buildHostingPlan({ version, files, firebaseConfig, additions, ...overrides });

test('Hosting file hash uses the level-9 gzip upload representation, not raw bytes', () => {
  assert.equal(hostingFileHash(Buffer.from('hello')), '7c8163cbba6ec068812be635f974971faba6c21f6d2daf1084142649dd70ae85');
});

test('preserves all baseline non-Team hashes and config keys/order; prepends only five scoped REST rewrites', () => {
  const input = structuredClone({ version, files, firebaseConfig });
  const plan = build();
  for (const file of files.filter(file => file.path !== TEAM_ENTRY)) assert.equal(plan.unionFiles.find(value => value.path === file.path).hash, file.hash);
  assert.equal(plan.unionFiles.find(file => file.path === TEAM_ENTRY).hash, digest('f'));
  assert.deepEqual(Object.keys(plan.candidateConfig), Object.keys(version.config));
  assert.deepEqual(plan.candidateConfig.headers.slice(0, 1), version.config.headers);
  assert.deepEqual(plan.candidateConfig.headers.slice(1).map(rule => rule.glob), ['/teams', '/teams/**', '/open/team/join']);
  assert.deepEqual(plan.candidateConfig.rewrites.slice(5), version.config.rewrites);
  assert.deepEqual(plan.candidateConfig.rewrites.slice(0, 5).map(rule => rule.glob), ['/v1/teams', '/v1/teams/**', '/teams', '/teams/**', '/open/team/join']);
  assert.deepEqual(plan.candidateConfig.redirects, version.config.redirects);
  assert.deepEqual({ version, files, firebaseConfig }, input, 'pure planning must not mutate the captured baseline');
  assert.equal(plan.hashes.baselineConfig, canonicalHash(version.config));
});

test('rejects different-hash existing assets and every non-Teams overlay path; same-hash live assets are reused without staging', () => {
  assert.throws(() => build({ additions: [...additions, { path: '/assets/live-12345678.js', hash: digest('f') }] }), /collision/);
  for (const name of ['/index.html', '/docs/manual.html', '/admin/index.html']) assert.throws(() => build({ additions: [...additions, { path: name, hash: digest('f') }] }), /outside Teams/);
  const plan = build({ additions: [...additions, { path: '/assets/live-12345678.js', hash: digest('c') }] });
  assert.equal(plan.newFiles.some(file => file.path === '/assets/live-12345678.js'), false);
  assert.equal(plan.newFiles.find(file => file.path === TEAM_ENTRY).change, 'replace-entry');
});

test('fails closed for nonfinalized/drifted/incomplete/duplicate/inactive snapshots', () => {
  assert.throws(() => build({ version: { ...version, status: 'CREATED' } }), /FINALIZED/);
  assert.throws(() => build({ version: { ...version, name: BASELINE_VERSION + '-changed' } }), /changed/);
  assert.throws(() => build({ files: files.slice(1) }), /pagination/);
  assert.throws(() => build({ files: [files[0], files[0], files[2], files[3]] }), /duplicate baseline/);
  assert.throws(() => build({ files: files.map((file, index) => index ? file : { ...file, status: 'EXPECTED' }) }), /incomplete baseline/);
  assert.throws(() => build({ additions: additions.filter(file => file.path !== TEAM_ENTRY) }), /entry HTML/);
});

test('local input reader accepts regular bytes and rejects file and parent-directory symlinks', () => {
  const root = mkdtempSync(path.join(realpathSync(tmpdir()), 'teams-hosting-plan-'));
  try {
    const regular = path.join(root, 'regular.json'); writeFileSync(regular, 'owned bytes');
    assert.equal(readRegular(regular).toString(), 'owned bytes');
    const link = path.join(root, 'link.json'); symlinkSync(regular, link);
    assert.throws(() => readRegular(link), /symlink/);
    const directoryLink = path.join(root, 'directory-link'); symlinkSync(root, directoryLink);
    assert.throws(() => readRegular(path.join(directoryLink, 'regular.json')), /symlink/);
  } finally { rmSync(root, { recursive: true }); }
});
