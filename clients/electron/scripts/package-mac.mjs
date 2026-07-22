#!/usr/bin/env node

import { chmodSync, copyFileSync, cpSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { execFileSync } from 'node:child_process';

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

function main() {
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

  if (commandAvailable('/usr/bin/codesign')) {
    log('applying deterministic ad-hoc code signature');
    execFileSync('/usr/bin/codesign', [
      '--force',
      '--deep',
      '--sign',
      '-',
      '--timestamp=none',
      paths.appPath,
    ], { stdio: 'inherit' });
  } else {
    log('codesign not found; artifact remains unsigned');
  }

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

try {
  main();
} catch (error) {
  process.stderr.write(`[package:mac] ERROR: ${formatError(error)}\n`);
  process.exitCode = 1;
}
