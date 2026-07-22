import assert from 'node:assert/strict';
import { existsSync, mkdirSync, mkdtempSync, rmSync, statSync, utimesSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

import {
  DEFAULT_SECRET_CANARY,
  FIXED_MTIME_SECONDS,
  artifactPaths,
  assertArm64Architecture,
  copyProductionDependencies,
  normalizeTimestamp,
  runtimePackageJson,
  scanTreeForForbiddenContent,
  validateZipEntries,
} from '../scripts/package-support.mjs';

test('single-path timestamp normalization does not retouch signed descendants', () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-timestamp-test-'));
  try {
    const child = join(root, 'signed-resource');
    writeFileSync(child, 'signed payload');
    const childTime = new Date((FIXED_MTIME_SECONDS + 60) * 1000);
    utimesSync(child, childTime, childTime);

    normalizeTimestamp(root);

    assert.equal(Math.floor(statSync(root).mtimeMs / 1000), FIXED_MTIME_SECONDS);
    assert.equal(Math.floor(statSync(child).mtimeMs / 1000), FIXED_MTIME_SECONDS + 60);
  } finally {
    rmSync(root, { force: true, recursive: true });
  }
});

test('artifact names are versioned and architecture-specific', () => {
  const paths = artifactPaths('/tmp/desktop', { version: '1.2.3' });
  assert.equal(paths.appPath, '/tmp/desktop/dist/LingXi-Code-1.2.3-mac-arm64/LingXi Code.app');
  assert.equal(paths.zipPath, '/tmp/desktop/dist/LingXi-Code-1.2.3-mac-arm64.zip');
  assert.equal(paths.checksumPath, '/tmp/desktop/dist/LingXi-Code-1.2.3-mac-arm64.zip.sha256');
});

test('runtime dependency copy expands package file globs by copying installed payload', () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-dependency-test-'));
  try {
    const packageRoot = join(root, 'node_modules', 'fixture');
    mkdirSync(join(packageRoot, 'lib'), { recursive: true });
    writeFileSync(join(packageRoot, 'package.json'), JSON.stringify({
      name: 'fixture',
      version: '1.0.0',
      files: ['lib/*.js'],
    }));
    writeFileSync(join(packageRoot, 'lib', 'runtime.js'), 'export const ready = true;');

    const destination = join(root, 'output', 'node_modules');
    copyProductionDependencies({ fixture: '1.0.0' }, root, destination);

    assert.equal(existsSync(join(destination, 'fixture', 'lib', 'runtime.js')), true);
  } finally {
    rmSync(root, { force: true, recursive: true });
  }
});

test('runtime manifests discard development-only install metadata', () => {
  const runtime = runtimePackageJson({
    name: 'fixture',
    version: '1.0.0',
    main: 'index.js',
    files: ['dist'],
    scripts: { test: 'false' },
    dependencies: { ws: '1.0.0' },
    devDependencies: { typescript: '1.0.0' },
  });
  assert.deepEqual(runtime, {
    name: 'fixture',
    version: '1.0.0',
    main: 'index.js',
    dependencies: { ws: '1.0.0' },
  });
});

test('architecture validation accepts arm64 and rejects x86-only binaries', () => {
  assert.doesNotThrow(() => assertArm64Architecture(['arm64'], 'fixture'));
  assert.doesNotThrow(() => assertArm64Architecture(['x86_64', 'arm64'], 'fixture'));
  assert.throws(
    () => assertArm64Architecture(['x86_64'], 'fixture'),
    /arm64 is required/,
  );
});

test('ZIP validation rejects traversal and entries outside the app', () => {
  assert.doesNotThrow(() => validateZipEntries([
    'LingXi Code.app',
    'LingXi Code.app/Contents/Info.plist',
  ]));
  assert.throws(() => validateZipEntries(['../payload']), /unsafe ZIP entry/);
  assert.throws(() => validateZipEntries(['unrelated.txt']), /outside LingXi Code.app/);
});

test('package scanning rejects developer paths and an obvious secret canary', () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-package-test-'));
  try {
    writeFileSync(join(root, 'safe.txt'), 'portable fixture');
    assert.doesNotThrow(() => scanTreeForForbiddenContent(root, [
      { label: 'secret canary', pattern: new RegExp(DEFAULT_SECRET_CANARY, 'g') },
    ]));
    writeFileSync(join(root, 'unsafe.txt'), DEFAULT_SECRET_CANARY);
    assert.throws(
      () => scanTreeForForbiddenContent(root, [
        { label: 'secret canary', pattern: new RegExp(DEFAULT_SECRET_CANARY, 'g') },
      ]),
      /secret canary found/,
    );
    writeFileSync(join(root, 'unsafe.txt'), '/Users/example/Projects/private/source.ts');
    assert.throws(
      () => scanTreeForForbiddenContent(root),
      /absolute macOS user path found/,
    );
    writeFileSync(join(root, 'unsafe.txt'), '/Users/example/.cargo/registry/src/private.rs');
    assert.throws(
      () => scanTreeForForbiddenContent(root),
      /absolute macOS user path found/,
    );
  } finally {
    rmSync(root, { force: true, recursive: true });
  }
});
