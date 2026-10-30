import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { constants, openSync, closeSync, fstatSync, readFileSync, lstatSync, readdirSync, mkdirSync, writeFileSync, existsSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { gzipSync } from 'node:zlib';

export const BASELINE_VERSION = 'sites/lingxi-5e4b2/versions/b628323e196483e0';
export const TEAM_ENTRY = '/teams/index.html';
const TEAM_HEADERS = ['/teams', '/teams/**', '/open/team/join'];
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
// Firebase Hosting hashes its level-9 gzip upload stream, not the raw file.
export const hostingFileHash = bytes => hash(gzipSync(bytes, { level: 9 }));
const ordered = value => Array.isArray(value) ? value.map(ordered) : value && typeof value === 'object'
  ? Object.fromEntries(Object.keys(value).sort().map(key => [key, ordered(value[key])])) : value;
export const canonicalHash = value => hash(JSON.stringify(ordered(value)));
const validPath = value => typeof value === 'string' && value.startsWith('/') && value.length > 1
  && !/[\\\u0000-\u001f\u007f]/u.test(value) && !value.slice(1).split('/').some(part => ['', '.', '..'].includes(part));
const validHash = value => typeof value === 'string' && /^[a-f0-9]{64}$/u.test(value);

export function buildHostingPlan({ version, files, firebaseConfig, additions, expectedVersion = BASELINE_VERSION }) {
  assert.equal(version.status, 'FINALIZED', 'baseline version must be FINALIZED');
  assert.equal(version.name, expectedVersion, 'live baseline version changed; refresh and review its files/config');
  assert.ok(Array.isArray(files) && Number(version.fileCount) === files.length, 'baseline file pagination must be complete');
  const baseline = new Map();
  for (const file of files) {
    assert.ok(validPath(file.path) && validHash(file.hash) && file.status === 'ACTIVE', 'invalid/incomplete baseline file');
    assert.ok(!baseline.has(file.path), `duplicate baseline path: ${file.path}`);
    baseline.set(file.path, file.hash);
  }
  assert.ok(baseline.has('/index.html'), 'baseline root index must exist');
  const config = structuredClone(version.config);
  assert.ok(config && Array.isArray(config.headers) && Array.isArray(config.rewrites), 'expected REST headers/rewrites arrays');
  const site = version.name.split('/')[1];
  const hosting = Array.isArray(firebaseConfig.hosting) ? firebaseConfig.hosting.find(value => value.site === site) : firebaseConfig.hosting;
  assert.equal(hosting?.site, site, 'firebase.json Hosting site mismatch');
  const headers = TEAM_HEADERS.map(source => {
    const matches = hosting.headers.filter(value => value.source === source);
    assert.equal(matches.length, 1, `expected exactly one Teams header rule for ${source}`);
    const pairs = matches[0].headers.map(({ key, value }) => {
      assert.ok(typeof key === 'string' && typeof value === 'string', 'invalid Teams header'); return [key, value];
    });
    assert.equal(new Set(pairs.map(([key]) => key.toLowerCase())).size, pairs.length, 'duplicate Teams header key');
    return { glob: source, headers: Object.fromEntries(pairs) };
  });
  const rewrites = [
    { glob: '/v1/teams', run: { serviceId: 'teamsapi', region: 'us-central1' } },
    { glob: '/v1/teams/**', run: { serviceId: 'teamsapi', region: 'us-central1' } },
    ...['/teams', '/teams/**', '/open/team/join'].map(glob => ({ glob, path: TEAM_ENTRY })),
  ];
  config.headers = [...config.headers, ...headers];
  config.rewrites = [...rewrites, ...config.rewrites];
  assert.deepEqual(Object.keys(config), Object.keys(version.config), 'existing config key order changed');
  assert.deepEqual(config.headers.slice(0, -headers.length), version.config.headers);
  assert.deepEqual(config.rewrites.slice(rewrites.length), version.config.rewrites);
  for (const key of Object.keys(version.config).filter(key => !['headers', 'rewrites'].includes(key))) assert.deepEqual(config[key], version.config[key]);

  const union = new Map(baseline), seen = new Set(), newFiles = [];
  for (const file of additions) {
    assert.ok(validPath(file.path) && validHash(file.hash), 'invalid overlay file');
    assert.ok(file.path === TEAM_ENTRY || file.path.startsWith('/assets/'), `overlay path outside Teams entry/assets: ${file.path}`);
    assert.ok(!seen.has(file.path), `duplicate overlay path: ${file.path}`); seen.add(file.path);
    const previous = baseline.get(file.path);
    assert.ok(!previous || previous === file.hash || file.path === TEAM_ENTRY, `different-hash existing non-Team path collision: ${file.path}`);
    union.set(file.path, file.hash);
    if (!previous || file.path === TEAM_ENTRY) newFiles.push({ ...file, change: !previous ? 'new' : previous === file.hash ? 'unchanged-entry' : 'replace-entry' });
  }
  assert.ok(seen.has(TEAM_ENTRY), 'Teams entry HTML is required');
  for (const [name, digest] of baseline) if (name !== TEAM_ENTRY) assert.equal(union.get(name), digest, `baseline file changed: ${name}`);
  const manifest = map => [...map].sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0).map(([filePath, digest]) => ({ path: filePath, hash: digest }));
  const baselineFiles = manifest(baseline), unionFiles = manifest(union);
  return { baselineFiles, unionFiles, candidateConfig: config, newFiles,
    hashes: { baselineConfig: canonicalHash(version.config), candidateConfig: canonicalHash(config), baselineFiles: canonicalHash(baselineFiles), unionFiles: canonicalHash(unionFiles) } };
}

function noSymlinks(filePath) {
  const absolute = path.resolve(filePath), root = path.parse(absolute).root;
  let current = root;
  for (const part of absolute.slice(root.length).split(path.sep).filter(Boolean)) {
    current = path.join(current, part);
    assert.ok(!lstatSync(current).isSymbolicLink(), `symlink input/output component rejected: ${current}`);
  }
}
export function readRegular(filePath) {
  noSymlinks(filePath);
  const fd = openSync(filePath, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const before = fstatSync(fd); assert.ok(before.isFile(), `regular file required: ${filePath}`);
    const bytes = readFileSync(fd), after = fstatSync(fd);
    assert.ok(before.size === after.size && before.mtimeMs === after.mtimeMs && before.ctimeMs === after.ctimeMs, `source changed while reading: ${filePath}`);
    return bytes;
  } finally { closeSync(fd); }
}

export function prepareHosting({ baselineDir, dist, firebasePath, output, expectedVersion = BASELINE_VERSION }) {
  assert.ok(!existsSync(output), 'output must be a fresh directory; existing plans are never overwritten');
  noSymlinks(path.dirname(output));
  const versionBytes = readRegular(path.join(baselineDir, 'hosting-version.json'));
  const filesBytes = readRegular(path.join(baselineDir, 'hosting-files.json'));
  const firebaseBytes = readRegular(firebasePath);
  const additions = [], contents = new Map();
  const add = (source, destination) => {
    const bytes = readRegular(source);
    additions.push({ path: destination, hash: hostingFileHash(bytes), rawSha256: hash(bytes), bytes: bytes.length }); contents.set(destination, bytes);
  };
  const walk = directory => {
    noSymlinks(directory); assert.ok(lstatSync(directory).isDirectory(), 'assets directory required');
    for (const entry of readdirSync(directory, { withFileTypes: true }).sort((a, b) => a.name < b.name ? -1 : 1)) {
      const source = path.join(directory, entry.name);
      if (entry.isDirectory()) walk(source);
      else {
        assert.ok(entry.isFile() && /-[A-Za-z0-9_-]{8,}\.[A-Za-z0-9.]+$/u.test(entry.name), `only regular hashed assets admitted: ${source}`);
        add(source, `/assets/${path.relative(path.join(dist, 'assets'), source).split(path.sep).join('/')}`);
      }
    }
  };
  walk(path.join(dist, 'assets'));
  add(path.join(dist, 'index.html'), TEAM_ENTRY);
  const version = JSON.parse(versionBytes), plan = buildHostingPlan({ version, files: JSON.parse(filesBytes), firebaseConfig: JSON.parse(firebaseBytes), additions, expectedVersion });
  for (const match of contents.get(TEAM_ENTRY).toString('utf8').matchAll(/(?:src|href)=["'](\/[^"'?#]+)(?:[?#][^"']*)?["']/gu)) {
    assert.ok(plan.unionFiles.some(file => file.path === match[1]), `Teams HTML references a missing baseline/overlay resource: ${match[1]}`);
  }
  mkdirSync(output, { mode: 0o700 });
  const stage = path.join(output, 'stage'); mkdirSync(stage, { mode: 0o700 });
  for (const file of plan.newFiles) {
    const destination = path.join(stage, file.path.slice(1)); mkdirSync(path.dirname(destination), { recursive: true, mode: 0o700 });
    writeFileSync(destination, contents.get(file.path), { flag: 'wx', mode: 0o600 });
    const staged = readRegular(destination);
    assert.equal(hash(staged), file.rawSha256); assert.equal(hostingFileHash(staged), file.hash);
  }
  const documents = { 'baseline-files.json': plan.baselineFiles, 'baseline-config.json': version.config,
    'candidate-config.json': plan.candidateConfig, 'union-files.json': plan.unionFiles, 'new-files.json': plan.newFiles };
  const artifacts = {};
  for (const [name, value] of Object.entries(documents)) {
    const bytes = Buffer.from(JSON.stringify(value, null, 2) + '\n'); writeFileSync(path.join(output, name), bytes, { flag: 'wx', mode: 0o600 }); artifacts[name] = hash(bytes);
  }
  const summary = { status: 'LOCAL_PREPARATION_ONLY', remoteWritesPerformed: false, publicationReady: false,
    baselineVersion: version.name, baselineStatus: version.status, fileHashEncoding: 'sha256(gzip-level-9)', baselineFileCount: plan.baselineFiles.length,
    unionFileCount: plan.unionFiles.length, stagedFileCount: plan.newFiles.length, preservedNonTeamFiles: plan.baselineFiles.filter(file => file.path !== TEAM_ENTRY).length,
    rootIndexHash: plan.baselineFiles.find(file => file.path === '/index.html').hash, hashes: plan.hashes, artifacts,
    sourceSha256: { hostingVersion: hash(versionBytes), hostingFiles: hash(filesBytes), firebaseConfig: hash(firebaseBytes) },
    requiredBeforePublication: ['Confirm gcloud teamsApi serviceConfig.service resolves to Hosting run.serviceId teamsapi in us-central1; this plan does not verify it.',
      'Refresh the current LIVE version and every file page immediately before publication; any version/file/config drift requires rebuilding and reviewing this plan.',
      'Obtain explicit publication authorization; do not deploy the full Hosting tree, merge upstream main, change IAM or overwrite non-Team files.'] };
  writeFileSync(path.join(output, 'plan.json'), JSON.stringify(summary, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
  return summary;
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  const repo = fileURLToPath(new URL('../../../../', import.meta.url));
  const options = { baselineDir: path.join(repo, 'output/teams-t3-private-preflight-r2'), dist: path.join(repo, 'apps/web/dist'),
    firebasePath: path.join(repo, 'firebase.json'), output: path.join(repo, 'output/teams-t3-publication-plan'), expectedVersion: BASELINE_VERSION };
  const names = { '--baseline-dir': 'baselineDir', '--dist': 'dist', '--firebase-config': 'firebasePath', '--output': 'output', '--expected-version': 'expectedVersion' };
  for (let index = 2; index < process.argv.length; index += 2) {
    const name = names[process.argv[index]], value = process.argv[index + 1];
    assert.ok(name && value, `unknown/missing option: ${process.argv[index]}`); options[name] = name === 'expectedVersion' ? value : path.resolve(value);
  }
  console.log(JSON.stringify(prepareHosting(options), null, 2));
}
