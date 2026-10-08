#!/usr/bin/env node

import { chmodSync, copyFileSync, cpSync, mkdirSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { execFileSync } from 'node:child_process';


import {
  APP_NAME,
  TARGET_ARCH,
  artifactPaths,
  assertArm64Executable,
  commandAvailable,
  copyElectronApp,
  copyProductionDependencies,
  createAsarArchive,
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
import {
  BROKER_RESOURCE_DIRNAME,
  brokerIdentifiers,
  buildCredentialBrokerResources,
  ensureBrokerResourceLayout,
  findMachOFiles,
  signAppBundle,
  signBinary,
  validateProvisioningProfile,
  verifyIdentifier,
  verifySignedEntitlements,
  verifyTeamIdentifier,
  writeEntitlements,
} from './credential-broker.mjs';
import {
  AUDIO_HELPER_APP,
  AUDIO_HELPER_EXECUTABLE,
  audioHelperIdentifiers,
  buildAudioHelperApp,
  ensureAudioHelperResourceLayout,
} from './audio-helper.mjs';
import { prepareModBun, sealModBun, stageModBun } from './mod-bun.mjs';

function log(message) {
  process.stdout.write(`[package:mac] ${message}\n`);
}

function run(command, args, cwd = packageRoot) {
  log(`${command} ${args.join(' ')}`);
  execFileSync(command, args, { cwd, stdio: 'inherit' });
}

function writeHelperEntitlements(path, plugin = false) {
  const pluginRows = plugin ? `
  <key>com.apple.security.cs.allow-unsigned-executable-memory</key><true/>
  <key>com.apple.security.cs.disable-library-validation</key><true/>` : '';
  writeFileSync(path, `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "https://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>com.apple.security.cs.allow-jit</key><true/>${pluginRows}
</dict></plist>
`, 'utf8');
}

function signNestedFrameworks(frameworksDir, identity, helperEntitlementsPath, pluginEntitlementsPath) {
  const timestamp = identity === '-' ? '--timestamp=none' : '--timestamp';
  const nestedBinaries = findMachOFiles(frameworksDir);
  for (const path of nestedBinaries) {
    execFileSync('/usr/bin/codesign', ['--force', '--sign', identity, timestamp, '--options', 'runtime', path], { stdio: 'inherit' });
  }
  for (const entry of readdirSync(frameworksDir, { withFileTypes: true })) {
    const path = join(frameworksDir, entry.name);
    if (entry.isDirectory() && entry.name.endsWith('.framework')) {
      execFileSync('/usr/bin/codesign', ['--force', '--sign', identity, timestamp, '--options', 'runtime', path], { stdio: 'inherit' });
      continue;
    }
    if (entry.isDirectory() && entry.name.endsWith('.app')) {
      const entitlementsPath = entry.name.includes('(Plugin).app')
        ? pluginEntitlementsPath
        : helperEntitlementsPath;
      execFileSync('/usr/bin/codesign', [
        '--force', '--sign', identity, '--options', 'runtime', timestamp,
        '--entitlements', entitlementsPath, path,
      ], { stdio: 'inherit' });
    }
  }
  return nestedBinaries;
}

function signApplication(appPath, contents, appContainer, packagedSidecar, brokerResourceRoot, version, channel) {
  if (!commandAvailable('/usr/bin/codesign')) {
    throw new Error('codesign not found; signed macOS packaging requires Xcode command line tools');
  }
  const identity = process.env['LINGXI_CODESIGN_IDENTITY']?.trim();
  if (!identity) throw new Error('LINGXI_CODESIGN_IDENTITY is required for macOS packaging');
  const teamId = process.env['LINGXI_MAC_TEAM_ID']?.trim();
  const profilePath = process.env['LINGXI_MAC_PROVISIONING_PROFILE']?.trim();
  const brokerProfilePath = process.env['LINGXI_MAC_BROKER_PROVISIONING_PROFILE']?.trim() ?? profilePath;
  const audioProfilePath = process.env['LINGXI_MAC_AUDIO_PROVISIONING_PROFILE']?.trim();
  if (!teamId || !/^[A-Z0-9]{10}$/.test(teamId)) {
    throw new Error('LINGXI_MAC_TEAM_ID must be the 10-character team identifier for signed builds');
  }
  if (!profilePath) throw new Error('LINGXI_MAC_PROVISIONING_PROFILE is required for signed builds');
  if (!brokerProfilePath) throw new Error('LINGXI_MAC_BROKER_PROVISIONING_PROFILE is required for the credential broker app');
  if (!audioProfilePath) throw new Error('LINGXI_MAC_AUDIO_PROVISIONING_PROFILE is required for the audio helper app');
  const absoluteProfilePath = resolve(profilePath);
  const absoluteBrokerProfilePath = resolve(brokerProfilePath);
  const absoluteAudioProfilePath = resolve(audioProfilePath);
  const identifiers = brokerIdentifiers(channel);
  const audioIdentifiers = audioHelperIdentifiers(channel);
  validateProvisioningProfile(absoluteProfilePath, teamId, identifiers.desktopBundleId);
  validateProvisioningProfile(absoluteBrokerProfilePath, teamId, identifiers.brokerBundleId);
  validateProvisioningProfile(absoluteAudioProfilePath, teamId, audioIdentifiers.bundleId);
  copyFileSync(absoluteProfilePath, join(contents, 'embedded.provisionprofile'));
  copyFileSync(
    absoluteBrokerProfilePath,
    join(brokerResourceRoot, 'LingXiCredentialBroker.app', 'Contents', 'embedded.provisionprofile'),
  );
  const appEntitlementsPath = join(appContainer, 'LingXi-Code.entitlements.plist');
  writeEntitlements(appEntitlementsPath, {
    teamId,
    bundleId: identifiers.desktopBundleId,
    allowJit: true,
    disableLibraryValidation: true,
    audioInput: true,
  });
  const brokerEntitlementsPath = join(appContainer, 'LingXiCredentialBroker.entitlements.plist');
  writeEntitlements(brokerEntitlementsPath, {
    teamId,
    bundleId: identifiers.brokerBundleId,
  });
  const audioEntitlementsPath = join(appContainer, 'LingXiAudioHelper.entitlements.plist');
  writeEntitlements(audioEntitlementsPath, {
    teamId,
    bundleId: audioIdentifiers.bundleId,
    audioInput: true,
  });
  const helperEntitlementsPath = join(appContainer, 'Electron-Helper.entitlements.plist');
  const pluginEntitlementsPath = join(appContainer, 'Electron-Plugin-Helper.entitlements.plist');
  writeHelperEntitlements(helperEntitlementsPath);
  writeHelperEntitlements(pluginEntitlementsPath, true);
  ensureBrokerResourceLayout(brokerResourceRoot);
  ensureAudioHelperResourceLayout(join(contents, 'Resources'));
  const nestedFrameworkBinaries = signNestedFrameworks(
    join(contents, 'Frameworks'),
    identity,
    helperEntitlementsPath,
    pluginEntitlementsPath,
  );
  const audioHelperAppPath = join(contents, 'Resources', AUDIO_HELPER_APP);
  const modCompilerPath = join(contents, 'Resources', 'bin', 'bun');
  signBinary(modCompilerPath, identity, `${identifiers.desktopBundleId}.mod-compiler`, helperEntitlementsPath);
  sealModBun(join(contents, 'Resources'));
  signBinary(packagedSidecar, identity, identifiers.bridgeServerIdentifier);
  signBinary(join(brokerResourceRoot, 'bin', 'lingxi-credential-client'), identity, identifiers.clientIdentifier);
  signAppBundle(audioHelperAppPath, identity, audioEntitlementsPath);
  signAppBundle(join(brokerResourceRoot, 'LingXiCredentialBroker.app'), identity, brokerEntitlementsPath);
  signAppBundle(appPath, identity, appEntitlementsPath);
  verifyIdentifier(packagedSidecar, identifiers.bridgeServerIdentifier, teamId);
  verifyIdentifier(modCompilerPath, `${identifiers.desktopBundleId}.mod-compiler`, teamId);
  verifyIdentifier(join(brokerResourceRoot, 'bin', 'lingxi-credential-client'), identifiers.clientIdentifier, teamId);
  verifyIdentifier(audioHelperAppPath, audioIdentifiers.bundleId, teamId);
  verifyIdentifier(join(brokerResourceRoot, 'LingXiCredentialBroker.app'), identifiers.brokerBundleId, teamId);
  verifyIdentifier(appPath, identifiers.desktopBundleId, teamId);
  verifySignedEntitlements(audioHelperAppPath, teamId, audioIdentifiers.bundleId, {
    'com.apple.security.device.audio-input': true,
  });
  verifySignedEntitlements(join(brokerResourceRoot, 'LingXiCredentialBroker.app'), teamId, identifiers.brokerBundleId);
  verifySignedEntitlements(appPath, teamId, identifiers.desktopBundleId, {
    'com.apple.security.device.audio-input': true,
  });
  verifyTeamIdentifier(join(audioHelperAppPath, 'Contents', 'MacOS', AUDIO_HELPER_EXECUTABLE), teamId);
  for (const path of nestedFrameworkBinaries) verifyTeamIdentifier(path, teamId);
  log(`signed app, audio helper, and credential broker with ${identity} (${version})`);
}

async function main() {
  if (process.platform !== 'darwin' || process.arch !== TARGET_ARCH) {
    throw new Error(`macOS ${TARGET_ARCH} host required; received ${process.platform} ${process.arch}`);
  }

  const metadata = readJson(join(packageRoot, 'package.json'));
  const brokerChannel = process.env['LINGXI_CREDENTIAL_BROKER_CHANNEL']?.trim() || 'production';
  const identifiers = brokerIdentifiers(brokerChannel);
  const paths = artifactPaths(packageRoot, metadata);
  const electronApp = join(packageRoot, 'node_modules', 'electron', 'dist', 'Electron.app');
  const electronExecutable = join(electronApp, 'Contents', 'MacOS', 'Electron');
  const cargoTargetDir = resolve(repoRoot, process.env['CARGO_TARGET_DIR'] || 'target');
  const sidecar = resolve(
    process.env['LINGXI_BRIDGE_SERVER_BIN'] ??
      join(cargoTargetDir, 'release', 'bridge-server'),
  );

  // Reject the most expensive and most common packaging mistakes before building.
  assertArm64Executable(electronExecutable, 'Electron runtime');
  assertArm64Executable(sidecar, 'release bridge-server sidecar');
  const modCompiler = await prepareModBun({ platform: 'darwin', arch: TARGET_ARCH });
  assertArm64Executable(modCompiler.executable, 'Mod Bun compiler');
  try {
    scanTreeForForbiddenContent(sidecar);
  } catch (error) {
    throw new Error(
      `release bridge-server sidecar is not portable: ${formatError(error)}. ` +
      'Rebuild it with RUSTFLAGS entries that remap both the repository root and Cargo home ' +
      'cargo build --locked --release -p bridge-server --bin bridge-server.',
    );
  }

  run('npm', ['run', 'build'], join(repoRoot, 'packages', 'bridge-client'));
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
  await createAsarArchive(packagedApp, asarPath);
  rmSync(packagedApp, { recursive: true, force: true });
  const binDir = join(resources, 'bin');
  mkdirSync(binDir, { recursive: true });
  const packagedSidecar = join(binDir, 'bridge-server');
  copyFileSync(sidecar, packagedSidecar);
  chmodSync(packagedSidecar, 0o755);
  stageModBun(resources, modCompiler);
  const brokerResourceRoot = join(resources, BROKER_RESOURCE_DIRNAME);
  buildCredentialBrokerResources(brokerResourceRoot, {
    channel: brokerChannel,
    profilePath: process.env['LINGXI_MAC_BROKER_PROVISIONING_PROFILE']?.trim()
      ? resolve(process.env['LINGXI_MAC_BROKER_PROVISIONING_PROFILE'])
      : (process.env['LINGXI_MAC_PROVISIONING_PROFILE']?.trim()
        ? resolve(process.env['LINGXI_MAC_PROVISIONING_PROFILE'])
        : undefined),
    version: metadata.version,
    targetTriple: 'aarch64-apple-darwin',
    target: TARGET_ARCH,
  });
  const audioHelperStageRoot = join(paths.appContainer, 'audio-helper-resource');
  const builtAudioHelperApp = buildAudioHelperApp(audioHelperStageRoot, {
    channel: brokerChannel,
    profilePath: process.env['LINGXI_MAC_AUDIO_PROVISIONING_PROFILE']?.trim()
      ? resolve(process.env['LINGXI_MAC_AUDIO_PROVISIONING_PROFILE'])
      : undefined,
    version: metadata.version,
    targetTriple: 'aarch64-apple-darwin',
    target: TARGET_ARCH,
  });
  cpSync(builtAudioHelperApp, join(resources, AUDIO_HELPER_APP), { recursive: true });
  ensureAudioHelperResourceLayout(resources);

  copyFileSync(join(packageRoot, 'assets', 'icons', 'icon.icns'), join(resources, 'icon.icns'));
  copyFileSync(join(packageRoot, 'INTERNAL_BETA.md'), join(resources, 'INTERNAL_BETA.md'));
  const mainExecutable = renameElectronExecutable(paths.appPath);
  rewriteInfoPlist(join(contents, 'Info.plist'), metadata.version, identifiers.desktopBundleId);

  assertArm64Executable(mainExecutable, `${APP_NAME} executable`);
  assertArm64Executable(packagedSidecar, 'packaged bridge-server sidecar');
  signApplication(
    paths.appPath,
    contents,
    paths.appContainer,
    packagedSidecar,
    brokerResourceRoot,
    metadata.version,
    brokerChannel,
  );

  normalizeTimestamps(paths.appPath);
  createDeterministicZip(paths.appPath, paths.zipPath);
  const checksum = sha256File(paths.zipPath);
  const metadataPath = join(paths.appContainer, 'build-metadata.json');
  writeJson(metadataPath, {
    app: `${APP_NAME}.app`,
    architecture: TARGET_ARCH,
    bundleId: identifiers.desktopBundleId,
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
