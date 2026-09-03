#!/usr/bin/env node

import { extractAll } from '@electron/asar';
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';

import {
  APP_NAME,
  NS_MICROPHONE_USAGE_DESCRIPTION,
  artifactPaths,
  assertArm64Executable,
  commandAvailable,
  formatError,
  packageRoot,
  readJson,
  scanTreeForForbiddenContent,
  sha256File,
  validateZipEntries,
} from './package-support.mjs';
import {
  BROKER_RESOURCE_DIRNAME,
  brokerIdentifiers,
  findMachOFiles,
  verifyIdentifier,
  verifySignedEntitlements,
  verifyTeamIdentifier,
} from './credential-broker.mjs';

function requirePath(path, kind = 'path') {
  if (!existsSync(path)) throw new Error(`required ${kind} is missing: ${path}`);
}

function plistValue(plistPath, key) {
  return execFileSync('/usr/libexec/PlistBuddy', ['-c', `Print :${key}`, plistPath], {
    encoding: 'utf8',
  }).trim();
}

function plistHasKey(plistPath, key) {
  try {
    plistValue(plistPath, key);
    return true;
  } catch {
    return false;
  }
}

export function verifyPackage(root = packageRoot) {
  if (process.platform !== 'darwin') {
    throw new Error(`package verification requires macOS; received ${process.platform}`);
  }
  const metadata = readJson(join(root, 'package.json'));
  const paths = artifactPaths(root, metadata);
  requirePath(paths.appPath, 'application bundle');
  requirePath(paths.zipPath, 'ZIP artifact');
  requirePath(paths.checksumPath, 'SHA-256 file');

  const contents = join(paths.appPath, 'Contents');
  const resources = join(contents, 'Resources');
  const packagedAsar = join(resources, 'app.asar');
  const packagedApp = join(resources, 'app');
  requirePath(packagedAsar, 'application asar');
  if (existsSync(packagedApp)) {
    throw new Error('unpacked Resources/app must not remain beside app.asar');
  }
  if (existsSync(join(resources, 'default_app.asar'))) {
    throw new Error('Electron default_app.asar must not be present');
  }
  const extractedApp = mkdtempSync(join(tmpdir(), 'lingxi-asar-'));
  extractAll(packagedAsar, extractedApp);
  const expected = [
    join(contents, 'Info.plist'),
    join(contents, 'MacOS', APP_NAME),
    join(resources, 'bin', 'bridge-server'),
    join(resources, BROKER_RESOURCE_DIRNAME, 'broker-manifest.json'),
    join(resources, BROKER_RESOURCE_DIRNAME, 'LingXiCredentialBroker.launchd.plist'),
    join(resources, BROKER_RESOURCE_DIRNAME, 'bin', 'lingxi-credential-client'),
    join(resources, BROKER_RESOURCE_DIRNAME, 'LingXiCredentialBroker.app'),
    join(resources, 'icon.icns'),
    join(resources, 'INTERNAL_BETA.md'),
    join(extractedApp, 'package.json'),
    join(extractedApp, 'out', 'main', 'index.js'),
    join(extractedApp, 'out', 'preload', 'index.cjs'),
    join(extractedApp, 'out', 'renderer', 'index.html'),
    join(extractedApp, 'node_modules', '@lingxi', 'bridge-client', 'dist', 'index.js'),
    join(extractedApp, 'node_modules', 'ws', 'package.json'),
    join(extractedApp, 'node_modules', 'ws', 'lib', 'stream.js'),
  ];
  try {
  for (const path of expected) requirePath(path);
  const manifest = readJson(join(resources, BROKER_RESOURCE_DIRNAME, 'broker-manifest.json'));
  const identifiers = brokerIdentifiers(manifest.channel);

  assertArm64Executable(join(contents, 'MacOS', APP_NAME), `${APP_NAME} executable`);
  assertArm64Executable(join(resources, 'bin', 'bridge-server'), 'packaged bridge-server sidecar');
  assertArm64Executable(
    join(resources, BROKER_RESOURCE_DIRNAME, 'bin', 'lingxi-credential-client'),
    'packaged credential broker client',
  );
  assertArm64Executable(
    join(resources, BROKER_RESOURCE_DIRNAME, 'LingXiCredentialBroker.app', 'Contents', 'MacOS', 'LingXiCredentialBroker'),
    'packaged credential broker app',
  );

  const plistPath = join(contents, 'Info.plist');
  const expectedPlist = {
    CFBundleDisplayName: APP_NAME,
    CFBundleExecutable: APP_NAME,
    CFBundleIdentifier: identifiers.desktopBundleId,
    CFBundleName: APP_NAME,
    CFBundleShortVersionString: metadata.version,
    CFBundleVersion: metadata.version,
    NSMicrophoneUsageDescription: NS_MICROPHONE_USAGE_DESCRIPTION,
  };
  for (const [key, value] of Object.entries(expectedPlist)) {
    const actual = plistValue(plistPath, key);
    if (actual !== value) throw new Error(`${key} is ${actual}; expected ${value}`);
  }
  for (const key of [
    'NSAppTransportSecurity',
    'NSAudioCaptureUsageDescription',
    'NSBluetoothAlwaysUsageDescription',
    'NSBluetoothPeripheralUsageDescription',
    'NSCameraUsageDescription',
  ]) {
    if (plistHasKey(plistPath, key)) throw new Error(`unused or permissive plist key remains: ${key}`);
  }

  const runtimeMetadata = readJson(join(extractedApp, 'package.json'));
  if (runtimeMetadata.main !== 'out/main/index.js') throw new Error('runtime package main is incorrect');
  if ('devDependencies' in runtimeMetadata) throw new Error('runtime package contains devDependencies');
  if (Object.keys(runtimeMetadata.dependencies ?? {}).sort().join(',') !== '@lingxi/bridge-client') {
    throw new Error('runtime package contains unexpected production dependencies');
  }
  for (const dependency of ['@lingxi/bridge-client', 'ws']) {
    const dependencyMetadata = readJson(join(extractedApp, 'node_modules', ...dependency.split('/'), 'package.json'));
    if ('devDependencies' in dependencyMetadata) {
      throw new Error(`${dependency} package contains devDependencies`);
    }
  }

  execFileSync(join(contents, 'MacOS', APP_NAME), [
    '--input-type=module',
    '-e',
    'import("@lingxi/bridge-client").then(({ BridgeClient }) => { if (typeof BridgeClient !== "function") process.exit(2); })',
  ], {
    cwd: extractedApp,
    env: { ...process.env, ELECTRON_RUN_AS_NODE: '1' },
    stdio: 'pipe',
  });

  scanTreeForForbiddenContent(paths.appPath);

  const zipEntries = execFileSync('/usr/bin/unzip', ['-Z1', paths.zipPath], { encoding: 'utf8' })
    .split('\n')
    .filter(Boolean);
  validateZipEntries(zipEntries);
  for (const path of [
    `${APP_NAME}.app/Contents/Info.plist`,
    `${APP_NAME}.app/Contents/MacOS/${APP_NAME}`,
    `${APP_NAME}.app/Contents/Resources/bin/bridge-server`,
    `${APP_NAME}.app/Contents/Resources/${BROKER_RESOURCE_DIRNAME}/broker-manifest.json`,
    `${APP_NAME}.app/Contents/Resources/${BROKER_RESOURCE_DIRNAME}/LingXiCredentialBroker.launchd.plist`,
    `${APP_NAME}.app/Contents/Resources/${BROKER_RESOURCE_DIRNAME}/bin/lingxi-credential-client`,
    `${APP_NAME}.app/Contents/Resources/${BROKER_RESOURCE_DIRNAME}/LingXiCredentialBroker.app/Contents/Info.plist`,
    `${APP_NAME}.app/Contents/Resources/INTERNAL_BETA.md`,
    `${APP_NAME}.app/Contents/Resources/app.asar`,
  ]) {
    if (!zipEntries.includes(path)) throw new Error(`required ZIP entry is missing: ${path}`);
  }

  const actualChecksum = sha256File(paths.zipPath);
  const checksumLine = readFileSync(paths.checksumPath, 'utf8').trim();
  const expectedChecksumLine = `${actualChecksum}  ${paths.zipPath.split('/').at(-1)}`;
  if (checksumLine !== expectedChecksumLine) throw new Error('SHA-256 file does not match ZIP artifact');

  if (commandAvailable('/usr/bin/codesign')) {
    execFileSync('/usr/bin/codesign', ['--verify', '--strict', paths.appPath], { stdio: 'pipe' });
    execFileSync('/usr/bin/codesign', ['--verify', '--strict', join(resources, 'bin', 'bridge-server')], { stdio: 'pipe' });
    execFileSync('/usr/bin/codesign', ['--verify', '--strict', join(resources, BROKER_RESOURCE_DIRNAME, 'bin', 'lingxi-credential-client')], { stdio: 'pipe' });
    execFileSync('/usr/bin/codesign', ['--verify', '--strict', join(resources, BROKER_RESOURCE_DIRNAME, 'LingXiCredentialBroker.app')], { stdio: 'pipe' });
    const teamId = process.env['LINGXI_MAC_TEAM_ID']?.trim();
    if (teamId) {
      verifyIdentifier(paths.appPath, identifiers.desktopBundleId, teamId);
      verifyIdentifier(join(resources, 'bin', 'bridge-server'), identifiers.bridgeServerIdentifier, teamId);
      verifyIdentifier(join(resources, BROKER_RESOURCE_DIRNAME, 'bin', 'lingxi-credential-client'), identifiers.clientIdentifier, teamId);
      verifyIdentifier(join(resources, BROKER_RESOURCE_DIRNAME, 'LingXiCredentialBroker.app'), identifiers.brokerBundleId, teamId);
      verifySignedEntitlements(join(resources, BROKER_RESOURCE_DIRNAME, 'LingXiCredentialBroker.app'), teamId, identifiers.brokerBundleId);
      verifySignedEntitlements(paths.appPath, teamId, identifiers.desktopBundleId);
      for (const path of findMachOFiles(join(contents, 'Frameworks'))) {
        verifyTeamIdentifier(path, teamId);
      }
    }
  }

  return {
    appPath: paths.appPath,
    appBytes: statSync(paths.appPath).size,
    checksum: actualChecksum,
    zipPath: paths.zipPath,
    zipBytes: statSync(paths.zipPath).size,
  };
  } finally {
    rmSync(extractedApp, { recursive: true, force: true });
  }
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  try {
    const result = verifyPackage();
    process.stdout.write(`[verify:package] OK ${result.appPath}\n`);
    process.stdout.write(`[verify:package] ZIP ${result.zipBytes} bytes, SHA-256 ${result.checksum}\n`);
  } catch (error) {
    process.stderr.write(`[verify:package] ERROR: ${formatError(error)}\n`);
    process.exitCode = 1;
  }
}
