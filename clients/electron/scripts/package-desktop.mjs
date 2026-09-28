#!/usr/bin/env node

import { execFileSync, spawnSync } from 'node:child_process';
import {
  chmodSync,
  copyFileSync,
  cpSync,
  existsSync,
  mkdirSync,
  readFileSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { basename, dirname, join, resolve } from 'node:path';

import {
  APP_NAME,
  BUNDLE_ID,
  binaryArchitectures,
  commandAvailable,
  copyElectronApp,
  copyProductionDependencies,
  createAsarArchive,
  createDeterministicZip,
  desktopArtifactPaths,
  formatError,
  normalizeTimestamps,
  packageRoot,
  parseDesktopTargetArgs,
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
  BRIDGE_SERVER_IDENTIFIER,
  BROKER_CLIENT_IDENTIFIER,
  BROKER_RESOURCE_DIRNAME,
  buildCredentialBrokerResources,
  signBinary,
} from './credential-broker.mjs';

function log(message) {
  process.stdout.write(`[package:desktop] ${message}\n`);
}

function run(command, args, cwd = packageRoot) {
  log(`${command} ${args.join(' ')}`);
  execFileSync(command, args, { cwd, stdio: 'inherit' });
}

function assertNativeHost(target) {
  if (process.platform !== target.platform || process.arch !== target.arch) {
    throw new Error(
      `native ${target.id} host required; received ${process.platform}-${process.arch}. `
      + 'Desktop sidecars are never cross-packaged.',
    );
  }
}

function assertBinaryArchitecture(path, target, label) {
  if (!existsSync(path)) throw new Error(`${label} is missing: ${path}`);
  if (target.platform !== 'win32' && (statSync(path).mode & 0o111) === 0) {
    throw new Error(`${label} is not executable: ${path}`);
  }
  if (target.platform === 'darwin') {
    const architectures = binaryArchitectures(path);
    if (!architectures.includes(target.arch)) {
      throw new Error(`${label} has ${architectures.join(', ') || 'unknown'}; ${target.arch} is required`);
    }
    return;
  }
  const bytes = readFileSync(path);
  if (target.platform === 'linux') {
    if (bytes.length < 20 || bytes.subarray(0, 4).toString('hex') !== '7f454c46') {
      throw new Error(`${label} is not an ELF executable`);
    }
    if (bytes.readUInt16LE(18) !== 0x3e) throw new Error(`${label} is not linux-x64`);
    return;
  }
  if (bytes.length < 64 || bytes.subarray(0, 2).toString('ascii') !== 'MZ') {
    throw new Error(`${label} is not a PE executable`);
  }
  const peOffset = bytes.readUInt32LE(0x3c);
  if (peOffset + 6 > bytes.length || bytes.subarray(peOffset, peOffset + 4).toString('ascii') !== 'PE\0\0') {
    throw new Error(`${label} has an invalid PE header`);
  }
  if (bytes.readUInt16LE(peOffset + 4) !== 0x8664) throw new Error(`${label} is not win32-x64`);
}

async function createRuntimeAsar(resources, metadata) {
  const unpacked = join(resources, 'app');
  mkdirSync(unpacked, { recursive: true });
  cpSync(join(packageRoot, 'out'), join(unpacked, 'out'), { recursive: true });
  const dependencies = Object.fromEntries(
    Object.keys(metadata.dependencies ?? {}).sort().map((name) => {
      const dependency = readJson(join(packageRoot, 'node_modules', ...name.split('/'), 'package.json'));
      return [name, dependency.version];
    }),
  );
  writeJson(join(unpacked, 'package.json'), {
    name: metadata.name,
    version: metadata.version,
    description: metadata.description,
    license: metadata.license,
    private: true,
    type: metadata.type,
    main: metadata.main,
    dependencies,
  });
  copyProductionDependencies(metadata.dependencies, packageRoot, join(unpacked, 'node_modules'));
  await createAsarArchive(unpacked, join(resources, 'app.asar'));
  rmSync(unpacked, { recursive: true, force: true });
}

function createArchive(paths) {
  unlinkIfPresent(paths.artifactPath);
  if (paths.platform === 'darwin') {
    createDeterministicZip(paths.payloadRoot, paths.artifactPath);
    return;
  }
  const args = paths.platform === 'win32'
    ? ['-a', '-c', '-f', paths.artifactPath, basename(paths.payloadRoot)]
    : ['-czf', paths.artifactPath, basename(paths.payloadRoot)];
  const result = spawnSync('tar', args, { cwd: dirname(paths.payloadRoot), encoding: 'utf8' });
  if (result.status !== 0) {
    throw new Error(`archive creation failed: ${(result.stderr || result.stdout || '').trim()}`);
  }
}

async function assembleDarwin(paths, metadata, electronDist, sidecar) {
  if (process.env['LINGXI_CREDENTIAL_BROKER_TEST_MOCK'] !== '1') {
    throw new Error(
      'generic macOS packaging is test-only; use npm run package:mac with an Apple signing identity and provisioning profile',
    );
  }
  const electronApp = join(electronDist, 'Electron.app');
  const sourceExecutable = join(electronApp, 'Contents', 'MacOS', 'Electron');
  assertBinaryArchitecture(sourceExecutable, paths, 'Electron runtime');
  copyElectronApp(electronApp, paths.payloadRoot);
  const contents = join(paths.payloadRoot, 'Contents');
  const resources = join(contents, 'Resources');
  rmSync(join(resources, 'default_app.asar'), { force: true });
  await createRuntimeAsar(resources, metadata);
  const binDir = join(resources, 'bin');
  mkdirSync(binDir, { recursive: true });
  const packagedSidecar = join(binDir, 'bridge-server');
  copyFileSync(sidecar, packagedSidecar);
  chmodSync(packagedSidecar, 0o755);
  const brokerResourceRoot = join(resources, BROKER_RESOURCE_DIRNAME);
  buildCredentialBrokerResources(brokerResourceRoot, {
    version: metadata.version,
    targetTriple: paths.arch === 'arm64' ? 'aarch64-apple-darwin' : 'x86_64-apple-darwin',
    target: paths.arch,
  });
  copyFileSync(join(packageRoot, 'assets', 'icons', 'icon.icns'), join(resources, 'icon.icns'));
  copyFileSync(join(packageRoot, 'INTERNAL_BETA.md'), join(resources, 'INTERNAL_BETA.md'));
  const executable = renameElectronExecutable(paths.payloadRoot);
  rewriteInfoPlist(join(contents, 'Info.plist'), metadata.version);
  assertBinaryArchitecture(executable, paths, `${APP_NAME} executable`);
  assertBinaryArchitecture(packagedSidecar, paths, 'packaged bridge-server sidecar');
  if (!commandAvailable('/usr/bin/codesign')) throw new Error('codesign is required for macOS package tests');
  signBinary(packagedSidecar, '-', BRIDGE_SERVER_IDENTIFIER);
  signBinary(
    join(brokerResourceRoot, 'bin', 'lingxi-credential-client'),
    '-',
    BROKER_CLIENT_IDENTIFIER,
  );
  execFileSync('/usr/bin/codesign', [
    '--force', '--sign', '-', '--timestamp=none',
    join(brokerResourceRoot, 'LingXiCredentialBroker.app'),
  ], { stdio: 'inherit' });
  execFileSync('/usr/bin/codesign', [
    '--force', '--sign', '-', '--timestamp=none', paths.payloadRoot,
  ], { stdio: 'inherit' });
}

async function assemblePortable(paths, metadata, electronDist, sidecar) {
  cpSync(electronDist, paths.payloadRoot, { recursive: true });
  const resources = join(paths.payloadRoot, 'resources');
  rmSync(join(resources, 'default_app.asar'), { force: true });
  await createRuntimeAsar(resources, metadata);
  const binDir = join(resources, 'bin');
  mkdirSync(binDir, { recursive: true });
  const sidecarName = paths.platform === 'win32' ? 'bridge-server.exe' : 'bridge-server';
  const packagedSidecar = join(binDir, sidecarName);
  copyFileSync(sidecar, packagedSidecar);
  copyFileSync(join(packageRoot, 'INTERNAL_BETA.md'), join(resources, 'INTERNAL_BETA.md'));
  const sourceExecutable = join(paths.payloadRoot, paths.platform === 'win32' ? 'electron.exe' : 'electron');
  const executable = join(paths.payloadRoot, paths.platform === 'win32' ? `${APP_NAME}.exe` : 'lingxi-code');
  renameSync(sourceExecutable, executable);
  if (paths.platform !== 'win32') {
    chmodSync(executable, 0o755);
    chmodSync(packagedSidecar, 0o755);
  }
  assertBinaryArchitecture(executable, paths, `${APP_NAME} executable`);
  assertBinaryArchitecture(packagedSidecar, paths, 'packaged bridge-server sidecar');
}

async function main() {
  const target = parseDesktopTargetArgs(process.argv.slice(2));
  assertNativeHost(target);
  const metadata = readJson(join(packageRoot, 'package.json'));
  const paths = desktopArtifactPaths(packageRoot, target.platform, target.arch);
  const electronDist = join(packageRoot, 'node_modules', 'electron', 'dist');
  const sidecar = resolve(
    process.env['LINGXI_BRIDGE_SERVER_BIN']
      ?? join(repoRoot, 'target', 'release', target.platform === 'win32' ? 'bridge-server.exe' : 'bridge-server'),
  );
  assertBinaryArchitecture(sidecar, target, 'release bridge-server sidecar');
  scanTreeForForbiddenContent(sidecar);

  run('npm', ['run', 'build'], join(packageRoot, '..', 'shared'));
  run('npm', ['run', 'build'], packageRoot);
  resetDirectory(paths.stageRoot);
  unlinkIfPresent(paths.checksumPath);
  unlinkIfPresent(paths.metadataPath);

  if (target.platform === 'darwin') await assembleDarwin(paths, metadata, electronDist, sidecar);
  else await assemblePortable(paths, metadata, electronDist, sidecar);

  scanTreeForForbiddenContent(paths.payloadRoot);
  normalizeTimestamps(paths.payloadRoot);
  createArchive(paths);
  const checksum = sha256File(paths.artifactPath);
  writeFileSync(paths.checksumPath, `${checksum}  ${paths.artifactName}\n`, 'utf8');
  writeJson(paths.metadataPath, {
    app: APP_NAME,
    artifact: paths.artifactName,
    architecture: target.arch,
    bundleId: BUNDLE_ID,
    platform: target.platform,
    sha256: checksum,
    sidecar: target.platform === 'win32' ? 'bridge-server.exe' : 'bridge-server',
    version: metadata.version,
  });
  log(`artifact: ${paths.artifactPath}`);
  log(`sha256: ${checksum}`);
}

main().catch((error) => {
  process.stderr.write(`[package:desktop] ERROR: ${formatError(error)}\n`);
  process.exitCode = 1;
});
