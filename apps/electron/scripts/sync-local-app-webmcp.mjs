#!/usr/bin/env node
import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import { createHash } from 'node:crypto';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const electronRoot = resolve(here, '..');
const productRoot = resolve(electronRoot, '../..');
const sourceRoot = resolve(
  process.env.LINGXI_HARNESS_ROOT ?? resolve(productRoot, '../harness-runtime'),
  'packages/local-app-webmcp',
);
const vendorRoot = join(electronRoot, 'vendor/local-app-webmcp');
const expectedCommit = 'f629c99458514784b9104864e5535245ca7ac6b0';

async function sha256(path) {
  return createHash('sha256').update(await fs.readFile(path)).digest('hex');
}

async function assertRegular(path) {
  const stat = await fs.lstat(path);
  assert.ok(stat.isFile() && !stat.isSymbolicLink(), `Expected a regular resource file: ${path}`);
}

const provenancePath = join(sourceRoot, 'provenance.json');
const manifestPath = join(sourceRoot, 'dist/manifest.json');
const packagePath = join(sourceRoot, 'package.json');
await Promise.all([assertRegular(provenancePath), assertRegular(manifestPath), assertRegular(packagePath)]);
const provenance = JSON.parse(await fs.readFile(provenancePath, 'utf8'));
const manifest = JSON.parse(await fs.readFile(manifestPath, 'utf8'));
const sourcePackage = JSON.parse(await fs.readFile(packagePath, 'utf8'));
assert.equal(provenance.upstream?.commit, expectedCommit);
assert.equal(manifest.sourceCommit, expectedCommit);
assert.equal(sourcePackage.name, '@lingxi/local-app-webmcp');
assert.equal(sourcePackage.version, '0.1.0');

for (const [relativePath, expectedHash] of Object.entries(provenance.upstream.sha256)) {
  const sourcePath = join(sourceRoot, 'upstream', relativePath);
  await assertRegular(sourcePath);
  assert.equal(await sha256(sourcePath), expectedHash, `Upstream source hash mismatch: ${relativePath}`);
}
for (const [relativePath, artifact] of [
  ['dist/webmcp-polyfill.iife.js', manifest.artifact],
  ['dist/webmcp-polyfill.source.js', manifest.sourceModule],
]) {
  const sourcePath = join(sourceRoot, relativePath);
  await assertRegular(sourcePath);
  assert.equal(await sha256(sourcePath), artifact.sha256, `Generated WebMCP artifact hash mismatch: ${relativePath}`);
}

const files = [
  'package.json',
  'README.md',
  'provenance.json',
  'upstream/LICENSE',
  'upstream/package.json',
  'upstream/tsconfig.json',
  'upstream/src/auto.ts',
  'upstream/src/index.ts',
  'upstream/src/frames.ts',
  'dist/manifest.json',
  'dist/webmcp-polyfill.iife.js',
  'dist/webmcp-polyfill.source.js',
  'dist/webmcp-polyfill.source.d.ts',
];
for (const relativePath of files) {
  const sourcePath = join(sourceRoot, relativePath);
  const targetPath = join(vendorRoot, relativePath);
  await assertRegular(sourcePath);
  await fs.mkdir(dirname(targetPath), { recursive: true, mode: 0o755 });
  await fs.copyFile(sourcePath, targetPath);
}
process.stdout.write(`Synced pinned Local App WebMCP resource ${expectedCommit}\n`);
