import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import {
  DESKTOP_TARGETS,
  desktopArtifactPaths,
  desktopTarget,
  parseDesktopTargetArgs,
} from '../scripts/package-support.mjs';
import { assertBinaryArchitecture } from '../scripts/package-desktop.mjs';
import { verifyDesktopPackage, verifyMacFrameworkArchitectures } from '../scripts/verify-desktop-package.mjs';
import { verifyPackage } from '../scripts/verify-package.mjs';

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

test('macOS package architecture checks use real Mach-O slice names', { skip: process.platform !== 'darwin' }, () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-desktop-arch-'));
  try {
    const source = join(root, 'main.c');
    writeFileSync(source, 'int main(void) { return 0; }\n');
    const intel = join(root, 'intel');
    const arm = join(root, 'arm');
    const universal = join(root, 'universal');
    execFileSync('clang', ['-arch', 'x86_64', '-o', intel, source]);
    execFileSync('clang', ['-arch', 'arm64', '-o', arm, source]);
    execFileSync('/usr/bin/lipo', ['-create', intel, arm, '-output', universal]);
    const intelTarget = { platform: 'darwin', arch: 'x64' };
    const armTarget = { platform: 'darwin', arch: 'arm64' };
    assert.doesNotThrow(() => assertBinaryArchitecture(intel, intelTarget, 'sidecar'));
    assert.doesNotThrow(() => assertBinaryArchitecture(arm, armTarget, 'sidecar'));
    assert.doesNotThrow(() => assertBinaryArchitecture(universal, intelTarget, 'sidecar'));
    assert.doesNotThrow(() => assertBinaryArchitecture(universal, armTarget, 'sidecar'));
    assert.throws(() => assertBinaryArchitecture(intel, armTarget, 'sidecar'), /x86_64; arm64 is required/);
    assert.throws(() => assertBinaryArchitecture(arm, intelTarget, 'sidecar'), /arm64; x64 is required/);
    chmodSync(arm, 0o644);
    assert.throws(() => assertBinaryArchitecture(arm, armTarget, 'sidecar'), /not executable/);
    const invalid = join(root, 'invalid');
    writeFileSync(invalid, 'not a Mach-O', { mode: 0o755 });
    assert.throws(() => assertBinaryArchitecture(invalid, intelTarget, 'sidecar'), /cannot inspect Mach-O/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('the desktop packager still runs its preflight as a CLI', { skip: process.platform !== 'darwin' }, () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-desktop-cli-'));
  try {
    const script = join(electronRoot, 'scripts', 'package-desktop.mjs');
    const alias = join(root, 'package-desktop.mjs');
    symlinkSync(script, alias);
    for (const entry of [script, alias]) {
      const result = spawnSync(process.execPath, [
        entry, '--platform', 'darwin', '--arch', process.arch,
      ], {
        encoding: 'utf8',
        env: { ...process.env, LINGXI_BRIDGE_SERVER_BIN: join(root, 'missing-bridge') },
      });
      assert.equal(result.status, 1);
      assert.match(result.stderr, /release bridge-server sidecar is missing/);
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('standalone desktop verification rejects executable and sidecar target mismatches', { skip: process.platform !== 'darwin' }, () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-desktop-wrong-target-'));
  try {
    const paths = desktopArtifactPaths(root, 'darwin', 'arm64');
    const contents = join(paths.payloadRoot, 'Contents');
    const resources = join(contents, 'Resources');
    const bin = join(resources, 'bin');
    const executable = join(contents, 'MacOS', 'LingXi Code');
    mkdirSync(bin, { recursive: true });
    mkdirSync(dirname(executable), { recursive: true });
    mkdirSync(join(contents, 'Frameworks'));
    for (const path of [paths.artifactPath, paths.checksumPath, paths.metadataPath, join(resources, 'app.asar')]) {
      writeFileSync(path, 'fixture');
    }
    copyFileSync(join(electronRoot, 'assets', 'icons', 'icon.png'), join(resources, 'icon.png'));
    const source = join(root, 'main.c');
    writeFileSync(source, 'int main(void) { return 0; }\n');
    const arm = join(root, 'arm');
    const intel = join(root, 'intel');
    execFileSync('clang', ['-arch', 'arm64', '-o', arm, source]);
    execFileSync('clang', ['-arch', 'x86_64', '-o', intel, source]);
    copyFileSync(intel, executable);
    copyFileSync(arm, join(bin, 'bridge-server'));
    assert.throws(() => verifyDesktopPackage(root, 'darwin', 'arm64'), /Desktop executable has the wrong architecture/);
    copyFileSync(arm, executable);
    copyFileSync(intel, join(bin, 'bridge-server'));
    assert.throws(() => verifyDesktopPackage(root, 'darwin', 'arm64'), /sidecar has architecture x86_64; expected arm64/);

    copyFileSync(arm, join(bin, 'bridge-server'));
    execFileSync('/usr/bin/codesign', ['--force', '--sign', '-', '--timestamp=none', join(bin, 'bridge-server')]);
    const frameworks = join(contents, 'Frameworks');
    const framework = join(frameworks, 'Electron Framework.framework', 'Versions', 'A', 'Electron Framework');
    const helper = join(frameworks, 'Electron Helper.app', 'Contents', 'MacOS', 'Electron Helper');
    mkdirSync(dirname(framework), { recursive: true });
    mkdirSync(dirname(helper), { recursive: true });
    copyFileSync(intel, framework);
    copyFileSync(arm, helper);
    assert.throws(() => verifyDesktopPackage(root, 'darwin', 'arm64'), /nested framework or helper has the wrong architecture.*Electron Framework/);
    copyFileSync(arm, framework);
    copyFileSync(intel, helper);
    assert.throws(() => verifyDesktopPackage(root, 'darwin', 'arm64'), /nested framework or helper has the wrong architecture.*Electron Helper/);
    const universal = join(root, 'universal');
    execFileSync('/usr/bin/lipo', ['-create', intel, arm, '-output', universal]);
    copyFileSync(universal, framework);
    copyFileSync(universal, helper);
    assert.equal(verifyMacFrameworkArchitectures(frameworks, 'arm64').length, 2);
    assert.equal(verifyMacFrameworkArchitectures(frameworks, 'x86_64').length, 2);
    rmSync(helper);
    symlinkSync(intel, helper);
    assert.throws(() => verifyMacFrameworkArchitectures(frameworks, 'arm64'), /framework symlink escapes Frameworks/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('the signed-release verifier requires an explicit expected team', { skip: process.platform !== 'darwin' }, () => {
  const previous = process.env.LINGXI_MAC_TEAM_ID;
  try {
    delete process.env.LINGXI_MAC_TEAM_ID;
    assert.throws(() => verifyPackage('/nonexistent'), /LINGXI_MAC_TEAM_ID must identify the team/);
    process.env.LINGXI_MAC_TEAM_ID = 'invalid';
    assert.throws(() => verifyPackage('/nonexistent'), /LINGXI_MAC_TEAM_ID must identify the team/);
  } finally {
    if (previous === undefined) delete process.env.LINGXI_MAC_TEAM_ID;
    else process.env.LINGXI_MAC_TEAM_ID = previous;
  }
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
  assert.match(help, /--output-root DIR/);
  assert.match(help, /LINGXI_MAC_AUDIO_PROVISIONING_PROFILE/);
  assert.match(help, /LINGXI_BRIDGE_SERVER_BIN/);
});

test('Flare wrapper rejects invalid explicit bridge paths before signing', () => {
  for (const bridge of ['relative/bridge-server', '/does-not-exist/bridge-server']) {
    const result = spawnSync('/bin/bash', [join(electronRoot, 'scripts', 'package-macos-flare.sh')], {
      encoding: 'utf8', env: { ...process.env, LINGXI_BRIDGE_SERVER_BIN: bridge },
    });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /LINGXI_BRIDGE_SERVER_BIN must be/);
  }
});

test('Flare wrapper parses and prints an explicit isolated output root', () => {
  const outputRoot = mkdtempSync(join(tmpdir(), 'lingxi-flare-artifacts-'));
  try {
    const output = execFileSync('/bin/bash', [
      join(electronRoot, 'scripts', 'package-macos-flare.sh'),
      '--output-root',
      outputRoot,
      '--print-config',
    ], { encoding: 'utf8' });
    assert.match(output, new RegExp(`^output_root=${outputRoot.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}$`, 'm'));
  } finally {
    rmSync(outputRoot, { recursive: true, force: true });
  }
});
