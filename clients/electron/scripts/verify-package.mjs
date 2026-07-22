#!/usr/bin/env node

import { existsSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';

import {
  APP_NAME,
  BUNDLE_ID,
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
  const packagedApp = join(resources, 'app');
  const expected = [
    join(contents, 'Info.plist'),
    join(contents, 'MacOS', APP_NAME),
    join(resources, 'bin', 'bridge-server'),
    join(resources, 'icon.icns'),
    join(resources, 'INTERNAL_BETA.md'),
    join(packagedApp, 'package.json'),
    join(packagedApp, 'out', 'main', 'index.js'),
    join(packagedApp, 'out', 'preload', 'index.cjs'),
    join(packagedApp, 'out', 'renderer', 'index.html'),
    join(packagedApp, 'node_modules', '@lingxi', 'bridge-client', 'dist', 'index.js'),
    join(packagedApp, 'node_modules', 'ws', 'package.json'),
    join(packagedApp, 'node_modules', 'ws', 'lib', 'stream.js'),
  ];
  for (const path of expected) requirePath(path);
  if (existsSync(join(resources, 'default_app.asar'))) {
    throw new Error('Electron default_app.asar must not be present');
  }

  assertArm64Executable(join(contents, 'MacOS', APP_NAME), `${APP_NAME} executable`);
  assertArm64Executable(join(resources, 'bin', 'bridge-server'), 'packaged bridge-server sidecar');

  const plistPath = join(contents, 'Info.plist');
  const expectedPlist = {
    CFBundleDisplayName: APP_NAME,
    CFBundleExecutable: APP_NAME,
    CFBundleIdentifier: BUNDLE_ID,
    CFBundleName: APP_NAME,
    CFBundleShortVersionString: metadata.version,
    CFBundleVersion: metadata.version,
    NSMicrophoneUsageDescription: 'LingXi uses your microphone only when you start voice input in the composer.',
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

  const runtimeMetadata = readJson(join(packagedApp, 'package.json'));
  if (runtimeMetadata.main !== 'out/main/index.js') throw new Error('runtime package main is incorrect');
  if ('devDependencies' in runtimeMetadata) throw new Error('runtime package contains devDependencies');
  if (Object.keys(runtimeMetadata.dependencies ?? {}).sort().join(',') !== '@lingxi/bridge-client') {
    throw new Error('runtime package contains unexpected production dependencies');
  }
  for (const dependency of ['@lingxi/bridge-client', 'ws']) {
    const dependencyMetadata = readJson(join(packagedApp, 'node_modules', ...dependency.split('/'), 'package.json'));
    if ('devDependencies' in dependencyMetadata) {
      throw new Error(`${dependency} package contains devDependencies`);
    }
  }

  execFileSync(join(contents, 'MacOS', APP_NAME), [
    '--input-type=module',
    '-e',
    'import("@lingxi/bridge-client").then(({ BridgeClient }) => { if (typeof BridgeClient !== "function") process.exit(2); })',
  ], {
    cwd: packagedApp,
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
    `${APP_NAME}.app/Contents/Resources/INTERNAL_BETA.md`,
    `${APP_NAME}.app/Contents/Resources/app/out/main/index.js`,
  ]) {
    if (!zipEntries.includes(path)) throw new Error(`required ZIP entry is missing: ${path}`);
  }

  const actualChecksum = sha256File(paths.zipPath);
  const checksumLine = readFileSync(paths.checksumPath, 'utf8').trim();
  const expectedChecksumLine = `${actualChecksum}  ${paths.zipPath.split('/').at(-1)}`;
  if (checksumLine !== expectedChecksumLine) throw new Error('SHA-256 file does not match ZIP artifact');

  if (commandAvailable('/usr/bin/codesign')) {
    execFileSync('/usr/bin/codesign', ['--verify', '--deep', '--strict', paths.appPath], { stdio: 'pipe' });
  }

  return {
    appPath: paths.appPath,
    appBytes: statSync(paths.appPath).size,
    checksum: actualChecksum,
    zipPath: paths.zipPath,
    zipBytes: statSync(paths.zipPath).size,
  };
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
