import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import {
  DESKTOP_TARGETS,
  desktopArtifactPaths,
  desktopTarget,
  parseDesktopTargetArgs,
} from '../scripts/package-support.mjs';

const electronRoot = join(dirname(fileURLToPath(import.meta.url)), '..');

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

test('the Flare macOS wrapper pins the iOS team and isolated development identifiers', () => {
  const output = execFileSync('/bin/bash', [
    join(electronRoot, 'scripts', 'package-macos-flare.sh'),
    '--print-config',
  ], { encoding: 'utf8' });
  assert.match(output, /^team_id=AZ4AX7J833$/m);
  assert.match(output, /^channel=development$/m);
  assert.match(output, /^desktop_bundle_id=com\.lingxi\.code\.development$/m);
  assert.match(output, /^broker_bundle_id=com\.lingxi\.code\.credential-broker\.development$/m);
  assert.match(output, /^audio_bundle_id=com\.lingxi\.code\.audio-helper\.development$/m);

  const packageJson = JSON.parse(readFileSync(join(electronRoot, 'package.json'), 'utf8'));
  assert.equal(packageJson.scripts['package:mac:flare'], 'bash scripts/package-macos-flare.sh');

  const help = execFileSync('/bin/bash', [
    join(electronRoot, 'scripts', 'package-macos-flare.sh'),
    '--help',
  ], { encoding: 'utf8' });
  assert.match(help, /Xcode Automatic Signing/);
  assert.match(help, /--no-register/);
  assert.match(help, /LINGXI_MAC_AUDIO_PROVISIONING_PROFILE/);
});
