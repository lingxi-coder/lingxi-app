#!/usr/bin/env node

import { chmodSync, copyFileSync, cpSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { execFileSync } from 'node:child_process';

import { createPackage } from '@electron/asar';

import {
  APP_NAME,
  BUNDLE_ID,
  TARGET_ARCH,
  artifactPaths,
  assertArm64Executable,
  commandAvailable,
  copyElectronApp,
  copyProductionDependencies,
  createDeterministicZip,
  formatError,
  normalizeTimestamp,
  normalizeTimestamps,
  packageRoot,
  readJson,
  renameElectronExecutable,
  repoRoot,
  resetDirectory,
  rewriteInfoPlist,
  scanTreeForForbiddenContent,
  sha256File,
  unlinkIfPresent,
  writeJson,
} from './package-support.mjs';

function log(message) {
  process.stdout.write(`[package:mac] ${message}\n`);
}

function run(command, args, cwd = packageRoot) {
  log(`${command} ${args.join(' ')}`);
  execFileSync(command, args, { cwd, stdio: 'inherit' });
}

function escapeXml(value) {
  return value.replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;').replaceAll('"', '&quot;');
}

function readProvisioningProfile(path) {
  const plist = execFileSync('/usr/bin/security', ['cms', '-D', '-i', path]);
  return JSON.parse(execFileSync('/usr/bin/plutil', ['-convert', 'json', '-o', '-', '-'], {
    input: plist,
    encoding: 'utf8',
  }));
}

function signApplication(appPath, contents, appContainer) {
  if (!commandAvailable('/usr/bin/codesign')) {
    log('codesign not found; artifact remains unsigned');
    return;
  }
  const identity = process.env['LINGXI_CODESIGN_IDENTITY']?.trim();
  if (!identity) {
    log('applying deterministic ad-hoc code signature (shared login Keychain remains available)');
    execFileSync('/usr/bin/codesign', [
      '--force', '--deep', '--sign', '-', '--timestamp=none', appPath,
    ], { stdio: 'inherit' });
    return;
  }

  const teamId = process.env['LINGXI_MAC_TEAM_ID']?.trim();
  const profilePath = process.env['LINGXI_MAC_PROVISIONING_PROFILE']?.trim();
  if (!teamId || !/^[A-Z0-9]{10}$/.test(teamId)) {
    throw new Error('LINGXI_MAC_TEAM_ID must be the 10-character team identifier for signed builds');
  }
  if (!profilePath) throw new Error('LINGXI_MAC_PROVISIONING_PROFILE is required for signed builds');
  const absoluteProfilePath = resolve(profilePath);
  const profile = readProvisioningProfile(absoluteProfilePath);
  const profileTeams = Array.isArray(profile.TeamIdentifier) ? profile.TeamIdentifier : [];
  const applicationIdentifier = profile.Entitlements?.['com.apple.application-identifier'];
  const expectedApplicationIdentifier = `${teamId}.${BUNDLE_ID}`;
  if (!profileTeams.includes(teamId)) throw new Error('provisioning profile team does not match LINGXI_MAC_TEAM_ID');
  if (applicationIdentifier !== expectedApplicationIdentifier && applicationIdentifier !== `${teamId}.*`) {
    throw new Error(`provisioning profile does not authorize ${expectedApplicationIdentifier}`);
  }
  copyFileSync(absoluteProfilePath, join(contents, 'embedded.provisionprofile'));
  const entitlementsPath = join(appContainer, 'LingXi-Code.entitlements.plist');
  writeFileSync(entitlementsPath, `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "https://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>com.apple.application-identifier</key><string>${escapeXml(expectedApplicationIdentifier)}</string>
  <key>com.apple.developer.team-identifier</key><string>${escapeXml(teamId)}</string>
  <key>com.apple.security.cs.allow-jit</key><true/>
  <key>com.apple.security.cs.allow-unsigned-executable-memory</key><true/>
  <key>com.apple.security.cs.disable-library-validation</key><true/>
</dict></plist>
`, 'utf8');
  log(`signing with ${identity} and hardened runtime entitlements`);
  execFileSync('/usr/bin/codesign', [
    '--force', '--deep', '--sign', identity,
    '--entitlements', entitlementsPath,
    '--options', 'runtime',
    '--timestamp=none',
    appPath,
  ], { stdio: 'inherit' });
}

async function main() {
  if (process.platform !== 'darwin' || process.arch !== TARGET_ARCH) {
    throw new Error(`macOS ${TARGET_ARCH} host required; received ${process.platform} ${process.arch}`);
  }

  const metadata = readJson(join(packageRoot, 'package.json'));
  const paths = artifactPaths(packageRoot, metadata);
  const electronApp = join(packageRoot, 'node_modules', 'electron', 'dist', 'Electron.app');
  const electronExecutable = join(electronApp, 'Contents', 'MacOS', 'Electron');
  const sidecar = resolve(
    process.env['LINGXI_BRIDGE_SERVER_BIN'] ??
      join(repoRoot, 'lingxi-code', 'target', 'release', 'bridge-server'),
  );

  // Reject the most expensive and most common packaging mistakes before building.
  assertArm64Executable(electronExecutable, 'Electron runtime');
  assertArm64Executable(sidecar, 'release bridge-server sidecar');
  try {
    scanTreeForForbiddenContent(sidecar);
  } catch (error) {
    throw new Error(
      `release bridge-server sidecar is not portable: ${formatError(error)}. ` +
      'Rebuild it with RUSTFLAGS entries that remap both the repository root and Cargo home ' +
      'cargo build --locked --release -p bridge-server --bin bridge-server.',
    );
  }

  run('npm', ['run', 'build'], join(packageRoot, '..', 'shared'));
  run('npm', ['run', 'build'], packageRoot);

  resetDirectory(paths.appContainer);
  unlinkIfPresent(paths.zipPath);
  unlinkIfPresent(paths.checksumPath);
  copyElectronApp(electronApp, paths.appPath);

  const contents = join(paths.appPath, 'Contents');
  const resources = join(contents, 'Resources');
  const packagedApp = join(resources, 'app');
  rmSync(join(resources, 'default_app.asar'), { force: true });
  mkdirSync(packagedApp, { recursive: true });
  cpSync(join(packageRoot, 'out'), join(packagedApp, 'out'), { recursive: true });

  const runtimeMetadata = {
    name: metadata.name,
    version: metadata.version,
    description: metadata.description,
    license: metadata.license,
    private: true,
    type: metadata.type,
    main: metadata.main,
    dependencies: Object.fromEntries(
      Object.keys(metadata.dependencies ?? {}).sort().map((name) => {
        const dependencyManifest = readJson(join(packageRoot, 'node_modules', ...name.split('/'), 'package.json'));
        return [name, dependencyManifest.version];
      }),
    ),
  };
  writeJson(join(packagedApp, 'package.json'), runtimeMetadata);
  copyProductionDependencies(metadata.dependencies, packageRoot, join(packagedApp, 'node_modules'));
  const asarPath = join(resources, 'app.asar');
  await createPackage(packagedApp, asarPath);
  rmSync(packagedApp, { recursive: true, force: true });
  const binDir = join(resources, 'bin');
  mkdirSync(binDir, { recursive: true });
  const packagedSidecar = join(binDir, 'bridge-server');
  copyFileSync(sidecar, packagedSidecar);
  chmodSync(packagedSidecar, 0o755);

  copyFileSync(join(packageRoot, 'assets', 'icons', 'icon.icns'), join(resources, 'icon.icns'));
  copyFileSync(join(packageRoot, 'INTERNAL_BETA.md'), join(resources, 'INTERNAL_BETA.md'));
  const mainExecutable = renameElectronExecutable(paths.appPath);
  rewriteInfoPlist(join(contents, 'Info.plist'), metadata.version);

  assertArm64Executable(mainExecutable, `${APP_NAME} executable`);
  assertArm64Executable(packagedSidecar, 'packaged bridge-server sidecar');
  signApplication(paths.appPath, contents, paths.appContainer);

  normalizeTimestamps(paths.appPath);
  createDeterministicZip(paths.appPath, paths.zipPath);
  const checksum = sha256File(paths.zipPath);
  const metadataPath = join(paths.appContainer, 'build-metadata.json');
  writeJson(metadataPath, {
    app: `${APP_NAME}.app`,
    architecture: TARGET_ARCH,
    bundleId: BUNDLE_ID,
    version: metadata.version,
    zip: paths.zipPath.split('/').at(-1),
    sha256: checksum,
  });
  // The metadata is for the unpacked staging directory, not the signed app or ZIP.
  // Do not recursively touch the signed app again after the ZIP has been created.
  normalizeTimestamp(metadataPath);
  normalizeTimestamp(paths.appContainer);
  writeFileSync(paths.checksumPath, `${checksum}  ${paths.zipPath.split('/').at(-1)}\n`, 'utf8');

  log(`app: ${paths.appPath}`);
  log(`zip: ${paths.zipPath}`);
  log(`sha256: ${checksum}`);
}

main().catch((error) => {
  process.stderr.write(`[package:mac] ERROR: ${formatError(error)}\n`);
  process.exitCode = 1;
});
