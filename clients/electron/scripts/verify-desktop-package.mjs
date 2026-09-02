#!/usr/bin/env node

import { extractAll } from '@electron/asar';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  APP_NAME,
  desktopArtifactPaths,
  formatError,
  packageRoot,
  parseDesktopTargetArgs,
  readJson,
  scanTreeForForbiddenContent,
  sha256File,
  walkTree,
} from './package-support.mjs';

function requirePath(path, label) {
  if (!existsSync(path)) throw new Error(`${label} is missing: ${path}`);
}

function executablePath(paths) {
  if (paths.platform === 'darwin') return join(paths.payloadRoot, 'Contents', 'MacOS', APP_NAME);
  if (paths.platform === 'win32') return join(paths.payloadRoot, `${APP_NAME}.exe`);
  return join(paths.payloadRoot, 'lingxi-code');
}

function resourcesPath(paths) {
  return paths.platform === 'darwin'
    ? join(paths.payloadRoot, 'Contents', 'Resources')
    : join(paths.payloadRoot, 'resources');
}

export function verifyDesktopPackage(root, platform, arch) {
  const paths = desktopArtifactPaths(root, platform, arch);
  requirePath(paths.payloadRoot, 'packaged payload');
  requirePath(paths.artifactPath, 'Desktop artifact');
  requirePath(paths.checksumPath, 'SHA-256 file');
  requirePath(paths.metadataPath, 'build metadata');
  const resources = resourcesPath(paths);
  const asar = join(resources, 'app.asar');
  const sidecar = join(resources, 'bin', platform === 'win32' ? 'bridge-server.exe' : 'bridge-server');
  const executable = executablePath(paths);
  requirePath(asar, 'application asar');
  requirePath(sidecar, 'bridge-server sidecar');
  requirePath(executable, 'Electron executable');
  if (existsSync(join(resources, 'default_app.asar'))) throw new Error('Electron default_app.asar remains in the package');
  if (existsSync(join(resources, 'app'))) throw new Error('unpacked resources/app remains beside app.asar');

  const credentialFiles = walkTree(paths.payloadRoot)
    .map((path) => basename(path).toLowerCase())
    .filter((name) => name === '.credentials.json' || name.includes('plaintext-credential'));
  if (credentialFiles.length > 0) throw new Error(`plaintext credential files were packaged: ${credentialFiles.join(', ')}`);
  scanTreeForForbiddenContent(paths.payloadRoot);

  const checksum = sha256File(paths.artifactPath);
  const checksumLine = readFileSync(paths.checksumPath, 'utf8').trim();
  if (checksumLine !== `${checksum}  ${paths.artifactName}`) throw new Error('SHA-256 file does not match the artifact');
  const metadata = readJson(paths.metadataPath);
  if (
    metadata.artifact !== paths.artifactName
    || metadata.architecture !== arch
    || metadata.platform !== platform
    || metadata.sha256 !== checksum
  ) throw new Error('build metadata does not match the requested target/artifact');

  const archiveEntries = execFileSync('tar', ['-tf', paths.artifactPath], { encoding: 'utf8' })
    .split(/\r?\n/)
    .filter(Boolean);
  const expectedRoot = platform === 'darwin' ? `${APP_NAME}.app` : APP_NAME;
  for (const entry of archiveEntries) {
    const normalized = entry.replace(/^\.\//, '').replace(/\/$/, '');
    if (
      normalized.startsWith('/')
      || normalized.includes('\\')
      || normalized.split('/').includes('..')
      || (normalized !== expectedRoot && !normalized.startsWith(`${expectedRoot}/`))
    ) throw new Error(`unsafe archive entry: ${entry}`);
  }

  const extracted = mkdtempSync(join(tmpdir(), 'lingxi-desktop-asar-'));
  try {
    extractAll(asar, extracted);
    for (const path of [
      join(extracted, 'package.json'),
      join(extracted, 'out', 'main', 'index.js'),
      join(extracted, 'out', 'preload', 'index.cjs'),
      join(extracted, 'out', 'renderer', 'index.html'),
      join(extracted, 'node_modules', '@lingxi', 'bridge-client', 'dist', 'index.js'),
    ]) requirePath(path, 'runtime file');
    const runtimeMetadata = readJson(join(extracted, 'package.json'));
    if ('devDependencies' in runtimeMetadata || 'scripts' in runtimeMetadata) {
      throw new Error('runtime package contains development-only metadata');
    }
    execFileSync(executable, [
      '--input-type=module',
      '-e',
      'import("@lingxi/bridge-client").then(({ BridgeClient }) => { if (typeof BridgeClient !== "function") process.exit(2); })',
    ], {
      cwd: extracted,
      env: { ...process.env, ELECTRON_RUN_AS_NODE: '1' },
      stdio: 'pipe',
    });
  } finally {
    rmSync(extracted, { recursive: true, force: true });
  }

  return { artifact: paths.artifactPath, bytes: statSync(paths.artifactPath).size, checksum };
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  try {
    const target = parseDesktopTargetArgs(process.argv.slice(2));
    const result = verifyDesktopPackage(packageRoot, target.platform, target.arch);
    process.stdout.write(`[verify:desktop-package] OK ${result.artifact} (${result.bytes} bytes) ${result.checksum}\n`);
  } catch (error) {
    process.stderr.write(`[verify:desktop-package] ERROR: ${formatError(error)}\n`);
    process.exitCode = 1;
  }
}
