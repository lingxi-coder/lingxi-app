#!/usr/bin/env node

import { execFileSync, spawnSync } from 'node:child_process';
import { applyWindowsBrand } from './windows-brand.mjs';
import {
  chmodSync,
  copyFileSync,
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

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
import { writeProtocolRegistration } from './protocol-registration.mjs';
import { prepareModBun, sealModBun, stageModBun } from './mod-bun.mjs';
import { assertSafeStageRoot, assertSidecarOutsideOutputs, stageMacGStreamerRuntime, verifyMacAppSignature, verifyMacGStreamerRuntime } from './gstreamer-macos.mjs';
import { signNestedFrameworks, writeHelperEntitlements } from './package-mac.mjs';
import { bundlePortableGStreamerRuntime, preflightPortableGStreamerRuntime } from './gstreamer-portable.mjs';
import { buildFreshBridgeClient } from './bridge-client-dist.mjs';

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

export function assertBinaryArchitecture(path, target, label) {
  if (!existsSync(path)) throw new Error(`${label} is missing: ${path}`);
  if (target.platform !== 'win32' && (statSync(path).mode & 0o111) === 0) {
    throw new Error(`${label} is not executable: ${path}`);
  }
  if (target.platform === 'darwin') {
    const architectures = binaryArchitectures(path);
    // Node/Electron calls Intel "x64"; lipo reports the Mach-O name.
    const expectedArchitecture = target.arch === 'x64' ? 'x86_64' : target.arch;
    if (!architectures.includes(expectedArchitecture)) {
      throw new Error(`${label} has ${architectures.join(', ') || 'unknown'}; ${target.arch} is required`);
    }
    return;
  }
  const bytes = readFileSync(path);
  if (target.platform === 'linux') {
    if (bytes.length < 20 || bytes.subarray(0, 4).toString('hex') !== '7f454c46') {
      throw new Error(`${label} is not an ELF executable`);
    }
    const expectedMachine = target.arch === 'arm64' ? 0xb7 : 0x3e;
    if (bytes.readUInt16LE(18) !== expectedMachine) throw new Error(`${label} is not linux-${target.arch}`);
    return;
  }
  if (bytes.length < 64 || bytes.subarray(0, 2).toString('ascii') !== 'MZ') {
    throw new Error(`${label} is not a PE executable`);
  }
  const peOffset = bytes.readUInt32LE(0x3c);
  if (peOffset + 6 > bytes.length || bytes.subarray(peOffset, peOffset + 4).toString('ascii') !== 'PE\0\0') {
    throw new Error(`${label} has an invalid PE header`);
  }
  const expectedMachine = target.arch === 'arm64' ? 0xaa64 : 0x8664;
  if (bytes.readUInt16LE(peOffset + 4) !== expectedMachine) throw new Error(`${label} is not win32-${target.arch}`);
}

export function preflightDesktopSidecar(paths, source) {
  assertSafeStageRoot(paths.stageRoot, { allowMissing: true });
  assertSidecarOutsideOutputs(source, [paths.stageRoot, paths.artifactPath, paths.checksumPath, paths.metadataPath]);
}

export function resetDesktopOutputs(paths, source) {
  preflightDesktopSidecar(paths, source);
  resetDirectory(paths.stageRoot);
  unlinkIfPresent(paths.checksumPath);
  unlinkIfPresent(paths.metadataPath);
}

export function stageDarwinSidecar(source, destination, frameworks, arch, stageRoot) {
  return stageMacGStreamerRuntime(source, destination, frameworks, arch, stageRoot);
}

export function signGenericDesktopFrameworks(frameworks, temporaryDirectory) {
  const helperEntitlements = join(temporaryDirectory, 'helper.entitlements.plist');
  const pluginEntitlements = join(temporaryDirectory, 'plugin.entitlements.plist');
  const appEntitlements = join(temporaryDirectory, 'test-app.entitlements.plist');
  writeHelperEntitlements(helperEntitlements);
  writeHelperEntitlements(pluginEntitlements, true);
  // The generic package uses ad-hoc signatures and therefore has no Team ID
  // with which hardened runtime can match its own nested libraries.
  writeFileSync(appEntitlements, `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>com.apple.security.cs.allow-jit</key><true/>
  <key>com.apple.security.cs.disable-library-validation</key><true/>
</dict></plist>\n`, 'utf8');
  signNestedFrameworks(frameworks, '-', helperEntitlements, pluginEntitlements);
  return appEntitlements;
}

async function createRuntimeAsar(resources, metadata) {
  const unpacked = join(resources, 'app');
  mkdirSync(unpacked, { recursive: true });
  cpSync(join(packageRoot, 'out'), join(unpacked, 'out'), { recursive: true });
  cpSync(join(packageRoot, 'src/main/local-apps/upstream/DYAD-LICENSE'), join(unpacked, 'Dyad-LICENSE'));
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

async function assembleDarwin(paths, metadata, electronDist, sidecar, modCompiler) {
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
  const packagedSidecar = join(binDir, 'bridge-server');
  cpSync(join(packageRoot, 'browser-extension'), join(resources, 'browser-extension'), { recursive: true });
  stageDarwinSidecar(sidecar, packagedSidecar, join(contents, 'Frameworks'), paths.arch === 'arm64' ? 'arm64' : 'x86_64', paths.stageRoot);
  const packagedModCompiler = stageModBun(resources, modCompiler);
  const brokerResourceRoot = join(resources, BROKER_RESOURCE_DIRNAME);
  buildCredentialBrokerResources(brokerResourceRoot, {
    version: metadata.version,
    targetTriple: paths.arch === 'arm64' ? 'aarch64-apple-darwin' : 'x86_64-apple-darwin',
    target: paths.arch,
  });
  copyFileSync(join(packageRoot, 'assets', 'icons', 'icon.icns'), join(resources, 'icon.icns'));
  copyFileSync(join(packageRoot, 'assets', 'icons', 'icon.png'), join(resources, 'icon.png'));
  copyFileSync(join(packageRoot, 'INTERNAL_BETA.md'), join(resources, 'INTERNAL_BETA.md'));
  const executable = renameElectronExecutable(paths.payloadRoot);
  rewriteInfoPlist(join(contents, 'Info.plist'), metadata.version);
  assertBinaryArchitecture(executable, paths, `${APP_NAME} executable`);
  assertBinaryArchitecture(packagedSidecar, paths, 'packaged bridge-server sidecar');
  if (!commandAvailable('/usr/bin/codesign')) throw new Error('codesign is required for macOS package tests');
  const signingDirectory = mkdtempSync(join(paths.stageRoot, '.lingxi-signing-'));
  try {
    const helperEntitlements = signGenericDesktopFrameworks(join(contents, 'Frameworks'), signingDirectory);
    signBinary(packagedSidecar, '-', BRIDGE_SERVER_IDENTIFIER);
    signBinary(packagedModCompiler, '-', `${BUNDLE_ID}.mod-compiler`);
    sealModBun(resources);
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
      '--force', '--sign', '-', '--timestamp=none', '--options', 'runtime',
      '--entitlements', helperEntitlements, paths.payloadRoot,
    ], { stdio: 'inherit' });
  } finally {
    rmSync(signingDirectory, { recursive: true, force: true });
  }
  verifyMacGStreamerRuntime(join(contents, 'Frameworks'), packagedSidecar, {
    expectedArch: paths.arch === 'arm64' ? 'arm64' : 'x86_64',
    appRoot: paths.payloadRoot,
  });
  verifyMacAppSignature(paths.payloadRoot);
}

async function assemblePortable(paths, metadata, electronDist, sidecar, modCompiler, gstreamerPlan) {
  cpSync(electronDist, paths.payloadRoot, { recursive: true });
  const resources = join(paths.payloadRoot, 'resources');
  rmSync(join(resources, 'default_app.asar'), { force: true });
  await createRuntimeAsar(resources, metadata);
  const binDir = join(resources, 'bin');
  mkdirSync(binDir, { recursive: true });
  const sidecarName = paths.platform === 'win32' ? 'bridge-server.exe' : 'bridge-server';
  const packagedSidecar = join(binDir, sidecarName);
  cpSync(join(packageRoot, 'browser-extension'), join(resources, 'browser-extension'), { recursive: true });
  copyFileSync(sidecar, packagedSidecar);
  bundlePortableGStreamerRuntime(gstreamerPlan, resources);
  stageModBun(resources, modCompiler);
  copyFileSync(join(packageRoot, 'INTERNAL_BETA.md'), join(resources, 'INTERNAL_BETA.md'));
  copyFileSync(join(packageRoot, 'assets', 'icons', 'icon.png'), join(resources, 'icon.png'));
  copyFileSync(join(packageRoot, 'assets', 'icons', 'icon.ico'), join(resources, 'icon.ico'));
  const sourceExecutable = join(paths.payloadRoot, paths.platform === 'win32' ? 'electron.exe' : 'electron');
  const executable = join(paths.payloadRoot, paths.platform === 'win32' ? `${APP_NAME}.exe` : 'lingxi-code');
  renameSync(sourceExecutable, executable);
  if (paths.platform === 'win32') applyWindowsBrand(executable, join(resources, 'icon.ico'), APP_NAME);
  if (paths.platform !== 'win32') {
    chmodSync(executable, 0o755);
    chmodSync(packagedSidecar, 0o755);
  }
  writeProtocolRegistration(paths.payloadRoot, paths.platform);
  assertBinaryArchitecture(executable, paths, `${APP_NAME} executable`);
  assertBinaryArchitecture(packagedSidecar, paths, 'packaged bridge-server sidecar');
}

async function main() {
  const target = parseDesktopTargetArgs(process.argv.slice(2));
  assertNativeHost(target);
  const metadata = readJson(join(packageRoot, 'package.json'));
  const paths = desktopArtifactPaths(packageRoot, target.platform, target.arch);
  const electronDist = join(packageRoot, 'node_modules', 'electron', 'dist');
  const cargoTargetDir = resolve(repoRoot, process.env['CARGO_TARGET_DIR'] || 'target');
  const sidecar = resolve(
    process.env['LINGXI_BRIDGE_SERVER_BIN']
      ?? join(cargoTargetDir, 'release', target.platform === 'win32' ? 'bridge-server.exe' : 'bridge-server'),
  );
  assertBinaryArchitecture(sidecar, target, 'release bridge-server sidecar');
  preflightDesktopSidecar(paths, sidecar);
  // Validate the explicit portable SDK before downloads, artifact resets, or
  // staging. macOS uses its separately signed runtime bundle below.
  const portableGStreamerPlan = target.platform === 'darwin'
    ? null
    : preflightPortableGStreamerRuntime(sidecar, target.platform, target.arch);
  const modCompiler = await prepareModBun(target);
  assertBinaryArchitecture(modCompiler.executable, target, 'Mod Bun compiler');
  scanTreeForForbiddenContent(sidecar);

  const sharedSdkRoot = join(repoRoot, 'packages', 'bridge-client');
  const sharedSdkName = readJson(join(sharedSdkRoot, 'package.json')).name;
  buildFreshBridgeClient(sharedSdkRoot, join(packageRoot, 'node_modules', ...sharedSdkName.split('/')),
    () => run('npm', ['run', 'build'], sharedSdkRoot));
  run('npm', ['run', 'build'], packageRoot);
  resetDesktopOutputs(paths, sidecar);

  if (target.platform === 'darwin') await assembleDarwin(paths, metadata, electronDist, sidecar, modCompiler);
  else await assemblePortable(paths, metadata, electronDist, sidecar, modCompiler, portableGStreamerPlan);

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

function isMainModule() {
  if (!process.argv[1]) return false;
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
}

if (isMainModule()) {
  main().catch((error) => {
    process.stderr.write(`[package:desktop] ERROR: ${formatError(error)}\n`);
    process.exitCode = 1;
  });
}
