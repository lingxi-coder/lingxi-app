import assert from 'node:assert/strict';
import test from 'node:test';

import {
  DESKTOP_TARGETS,
  desktopArtifactPaths,
  desktopTarget,
  parseDesktopTargetArgs,
} from '../scripts/package-support.mjs';

test('the internal-beta package matrix has exactly the four native targets', () => {
  assert.deepEqual(Object.keys(DESKTOP_TARGETS), [
    'darwin-arm64',
    'darwin-x64',
    'win32-x64',
    'linux-x64',
  ]);
});

test('desktop artifacts use the requested target names and companion files', () => {
  for (const [id, expected] of Object.entries(DESKTOP_TARGETS)) {
    const paths = desktopArtifactPaths('/tmp/desktop', expected.platform, expected.arch);
    assert.equal(paths.artifactName, `${id}.${expected.extension}`);
    assert.ok(paths.checksumPath.endsWith(`${paths.artifactName}.sha256`));
    assert.ok(paths.metadataPath.endsWith(`${id}.build-metadata.json`));
  }
});

test('package target arguments are explicit and reject cross-matrix values', () => {
  assert.deepEqual(
    parseDesktopTargetArgs(['--platform', 'linux', '--arch', 'x64']),
    { platform: 'linux', arch: 'x64', extension: 'tar.gz', id: 'linux-x64' },
  );
  assert.throws(() => parseDesktopTargetArgs([]), /both --platform and --arch are required/);
  assert.throws(() => desktopTarget('win32', 'arm64'), /unsupported Desktop target/);
  assert.throws(() => parseDesktopTargetArgs(['--target', 'linux-x64']), /unknown package argument/);
});
