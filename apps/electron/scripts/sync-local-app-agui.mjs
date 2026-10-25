#!/usr/bin/env node
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { constants as fsConstants, promises as fs } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const electronRoot = resolve(here, '..');
const productRoot = resolve(electronRoot, '../..');
const sourceRoot = resolve(process.env.LINGXI_HARNESS_ROOT ?? resolve(productRoot, '../harness-runtime'), 'packages/local-app-agui');
const vendorRoot = join(electronRoot, 'vendor/local-app-agui');
const EXPECTED_COMMIT = 'a1131c90709d51d562202b7b6cc0a2a1b48b05fc';
const EXPECTED_SCHEMA_SHA256 = '4b5c93226838a0e72d88e6c5df20633c686c49fcb75d9be2815c6cbf9e48e71a';
const fixturePath = 'tests/fixtures/run-agent-input.ag-ui-1.0.3.json';

async function fileSha256(path) {
  return createHash('sha256').update(await fs.readFile(path)).digest('hex');
}

async function assertRegular(path) {
  const stat = await fs.lstat(path);
  assert.ok(stat.isFile() && !stat.isSymbolicLink(), `Expected a regular source file: ${path}`);
}

const provenancePath = join(sourceRoot, 'spec/provenance.json');
const schemaPath = join(sourceRoot, 'spec/ag-ui-1.0.schema.json');
const sourcePackagePath = join(sourceRoot, 'package.json');
await Promise.all([assertRegular(provenancePath), assertRegular(schemaPath), assertRegular(sourcePackagePath)]);
const provenance = JSON.parse(await fs.readFile(provenancePath, 'utf8'));
const sourcePackage = JSON.parse(await fs.readFile(sourcePackagePath, 'utf8'));
assert.equal(provenance.sourceCommit, EXPECTED_COMMIT, 'AG-UI upstream revision changed; review and update the explicit pin before syncing.');
assert.equal(provenance.schemaSha256, EXPECTED_SCHEMA_SHA256, 'Official AG-UI schema pin changed.');
assert.equal(await fileSha256(schemaPath), EXPECTED_SCHEMA_SHA256, 'Official AG-UI schema file hash mismatch.');
assert.equal(sourcePackage.dependencies?.['@ag-ui/client'], '1.0.3');
assert.equal(sourcePackage.dependencies?.['@ag-ui/core'], '1.0.3');

const sourceManifestPath = join(sourceRoot, 'dist/manifest.json');
await assertRegular(sourceManifestPath);
const sourceManifest = JSON.parse(await fs.readFile(sourceManifestPath, 'utf8'));
assert.equal(sourceManifest.schemaVersion, 1);
assert.equal(sourceManifest.package?.name, '@lingxi/local-app-agui');
assert.equal(sourceManifest.package?.version, '0.1.0');
assert.equal(sourceManifest.source?.commit, EXPECTED_COMMIT);
assert.equal(sourceManifest.schema?.sha256, EXPECTED_SCHEMA_SHA256);
assert.equal(sourceManifest.schema?.bytes, 94359);
assert.deepEqual(sourceManifest.dependencies, {
  '@ag-ui/client': '1.0.3', '@ag-ui/core': '1.0.3', zod: '3.25.76', esbuild: '0.28.2', typescript: '5.8.3',
});
assert.equal(sourceManifest.lockfile?.path, 'package-lock.json');
await assertRegular(join(sourceRoot, sourceManifest.lockfile.path));
assert.equal(await fileSha256(join(sourceRoot, sourceManifest.lockfile.path)), sourceManifest.lockfile.sha256, 'Pinned SDK lockfile hash mismatch.');
const artifactPaths = Object.keys(sourceManifest.artifacts ?? {}).sort();
assert.ok(artifactPaths.includes('dist/browser.source.js'));
assert.ok(artifactPaths.includes('dist/types/browser-source.d.ts'));
assert.ok(artifactPaths.includes('dist/types/file.d.ts'));
assert.ok(artifactPaths.includes('dist/types/validators.d.ts'));
assert.ok(artifactPaths.includes('dist/licenses/THIRD-PARTY-NOTICES.md'));
assert.ok(artifactPaths.includes('dist/licenses/AG-UI-MIT.txt'));
for (const artifact of artifactPaths) {
  assert.ok(artifact.startsWith('dist/') && !artifact.split('/').includes('..'), `Invalid generated AG-UI artifact path: ${artifact}`);
  const path = join(sourceRoot, artifact);
  await assertRegular(path);
  assert.equal(await fileSha256(path), sourceManifest.artifacts?.[artifact], `Generated AG-UI artifact hash mismatch: ${artifact}`);
}
await assertRegular(join(sourceRoot, fixturePath));
await Promise.all(['AG-UI-MIT.txt', 'README.md'].map(file => assertRegular(join(sourceRoot, 'licenses', file))));

const targetDist = join(vendorRoot, 'dist');
const targetSpec = join(vendorRoot, 'spec');
const targetLicenses = join(vendorRoot, 'licenses');
const targetFixtures = join(targetSpec, 'fixtures');
await Promise.all([targetDist, targetSpec, targetLicenses].map(path => fs.rm(path, { recursive: true, force: true })));
await Promise.all([fs.mkdir(targetDist, { recursive: true, mode: 0o755 }), fs.mkdir(targetSpec, { recursive: true, mode: 0o755 }), fs.mkdir(targetLicenses, { recursive: true, mode: 0o755 })]);
const artifactHashes = {};
for (const artifact of artifactPaths) {
  const target = join(vendorRoot, artifact);
  await fs.mkdir(dirname(target), { recursive: true, mode: 0o755 });
  await fs.copyFile(join(sourceRoot, artifact), target, fsConstants.COPYFILE_EXCL);
  artifactHashes[artifact] = await fileSha256(target);
}
await fs.copyFile(sourceManifestPath, join(targetDist, 'manifest.json'), fsConstants.COPYFILE_EXCL);
artifactHashes['manifest.json'] = await fileSha256(join(targetDist, 'manifest.json'));
for (const file of ['ag-ui-1.0.schema.json', 'provenance.json']) {
  const target = join(targetSpec, file);
  await fs.copyFile(join(sourceRoot, 'spec', file), target, fsConstants.COPYFILE_EXCL);
  artifactHashes[`spec/${file}`] = await fileSha256(target);
}
await fs.mkdir(targetFixtures, { recursive: true, mode: 0o755 });
await fs.copyFile(join(sourceRoot, fixturePath), join(targetFixtures, 'run-agent-input.ag-ui-1.0.3.json'), fsConstants.COPYFILE_EXCL);
artifactHashes['spec/fixtures/run-agent-input.ag-ui-1.0.3.json'] = await fileSha256(join(targetFixtures, 'run-agent-input.ag-ui-1.0.3.json'));
for (const file of ['AG-UI-MIT.txt', 'README.md']) {
  const target = join(targetLicenses, file);
  await fs.copyFile(join(sourceRoot, 'licenses', file), target, fsConstants.COPYFILE_EXCL);
  artifactHashes[`licenses/${file}`] = await fileSha256(target);
}
const sourceReadme = await fs.readFile(join(sourceRoot, 'README.md'), 'utf8');
const standaloneUse = [
  '## Desktop vendored use',
  '',
  'This checked-in package is complete for normal Desktop `npm ci`, typechecks, and Electron builds. Those commands use only the files and lockfile in `apps/electron`; they do not require the adjacent Harness checkout.',
  '',
  'Maintainers may refresh these generated files with `npm run sync:local-app-agui` from `apps/electron`. That explicit sync step reads the pinned source package from the adjacent Harness checkout and verifies every manifest hash before copying.',
  '',
].join('\n');
const vendorReadme = sourceReadme.replace('## Reproducibility\n', `${standaloneUse}\n## Reproducibility\n`)
  .replace('Run `npm ci`, `npm run build`, and `npm test` from this directory.', 'The source package is built and tested in the Harness checkout; the checked-in Desktop vendor contains its generated client, browser installer, official schema, declarations, and license notices.');
await fs.writeFile(join(vendorRoot, 'README.md'), vendorReadme);
const receipt = {
  sourceRepository: provenance.sourceRepository,
  sourceCommit: provenance.sourceCommit,
  packages: provenance.packages,
  schemaSha256: EXPECTED_SCHEMA_SHA256,
  generatedArtifacts: artifactHashes,
};
await fs.writeFile(join(targetDist, 'sync-receipt.json'), `${JSON.stringify(receipt, null, 2)}\n`, { mode: 0o644 });
process.stdout.write(`${JSON.stringify({ vendorRoot, sourceCommit: receipt.sourceCommit, schemaSha256: receipt.schemaSha256, generatedArtifactCount: Object.keys(artifactHashes).length })}\n`);
