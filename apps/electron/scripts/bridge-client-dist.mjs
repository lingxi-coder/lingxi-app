import { cpSync, lstatSync, readFileSync, realpathSync, rmSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { isDeepStrictEqual } from 'node:util';

function checkInstalledPackage(sourceRoot, installedRoot) {
  const source = JSON.parse(readFileSync(join(sourceRoot, 'package.json'), 'utf8'));
  if (typeof source.name !== 'string' || !source.name || typeof source.version !== 'string' || !source.version) {
    throw new Error('shared SDK source manifest is invalid');
  }
  if (realpathSync(sourceRoot) === realpathSync(installedRoot)) return false;

  // Do not replace files through a link to an unrelated directory.
  const installedStat = lstatSync(installedRoot);
  if (installedStat.isSymbolicLink() || !installedStat.isDirectory()) {
    throw new Error('installed shared SDK points to an unexpected location');
  }
  const installed = JSON.parse(readFileSync(join(installedRoot, 'package.json'), 'utf8'));
  if (!isDeepStrictEqual(source, installed)) {
    throw new Error('installed shared SDK manifest differs from the source package; run npm ci in apps/electron');
  }
  return true;
}

// npm can install a file: dependency as either a link or an independent copy.
// Keep the latter in sync when packaging rebuilds the shared SDK.
export function syncBuiltBridgeClient(sourceRoot, installedRoot) {
  const copy = checkInstalledPackage(sourceRoot, installedRoot);
  const sourceDist = join(sourceRoot, 'dist');
  if (!statSync(sourceDist).isDirectory() || !statSync(join(sourceDist, 'index.js')).isFile()) {
    throw new Error('shared SDK build output is missing');
  }
  if (!copy) return;
  const installedDist = join(installedRoot, 'dist');
  rmSync(installedDist, { recursive: true, force: true });
  cpSync(sourceDist, installedDist, { recursive: true });
}

export function buildFreshBridgeClient(sourceRoot, installedRoot, build) {
  // Refuse an outdated dependency graph before modifying either output.
  checkInstalledPackage(sourceRoot, installedRoot);
  // TypeScript does not remove outputs for source files that were deleted.
  rmSync(join(sourceRoot, 'dist'), { recursive: true, force: true });
  build();
  syncBuiltBridgeClient(sourceRoot, installedRoot);
}
