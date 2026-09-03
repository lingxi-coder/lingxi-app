import { createHash } from 'node:crypto';
import {
  chmodSync,
  closeSync,
  cpSync,
  existsSync,
  lstatSync,
  lutimesSync,
  mkdirSync,
  openSync,
  readFileSync,
  readlinkSync,
  readSync,
  readdirSync,
  realpathSync,
  renameSync,
  rmSync,
  statSync,
  symlinkSync,
  unlinkSync,
  utimesSync,
  writeFileSync,
} from 'node:fs';
import { homedir } from 'node:os';
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync, spawnSync } from 'node:child_process';

export const APP_NAME = 'LingXi Code';
export const BUNDLE_ID = 'com.lingxi.code';
export const TARGET_ARCH = 'arm64';
export const DEFAULT_SECRET_CANARY = 'LINGXI_SECRET_CANARY_DO_NOT_PACKAGE';

/**
 * macOS shows this string verbatim in the microphone permission dialog, so
 * it has to be concrete about what the app actually does — vague copy
 * ("this app needs microphone access") is both bad UX and a common App
 * Store rejection reason.
 *
 * It deliberately does NOT claim speech recognition/dictation happens:
 * desktop speech recognition is unconditionally unavailable in this build
 * (`renderer/audio/requests.ts`'s `serviceAudioOp` answers every
 * `AudioOpDto::Transcribe` with `failed`/`unavailable` regardless of
 * microphone permission or which provider is configured — see that file's
 * `DESKTOP_TRANSCRIPTION_UNAVAILABLE_MESSAGE` and `Voice.tsx`'s
 * `RECOGNITION_UNAVAILABLE_NOTICE`, which says the same thing in the
 * settings UI). Recording and speech OUTPUT (synthesis) both really work;
 * this string only has to be honest about recording, since synthesis does
 * not touch the microphone.
 *
 * Exported (rather than inlined into `rewriteInfoPlist`'s replacement map)
 * so `verify-package.mjs`'s static check on the ACTUAL packaged
 * `Info.plist` asserts against this same constant instead of a second,
 * independently-typed copy that could silently drift from it.
 */
export const NS_MICROPHONE_USAGE_DESCRIPTION =
  'LingXi Code only uses the microphone to record a short audio clip when you choose to start recording, '
  + 'for example by pressing the microphone button in the composer. It never listens in the background, '
  + 'and this build does not transcribe the recording into text, whether on this device or on a server.';
export const FIXED_MTIME_SECONDS = Number(process.env['SOURCE_DATE_EPOCH'] ?? 946684800);
export const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export const repoRoot = resolve(packageRoot, '..', '..');

export function readJson(path) {
  return JSON.parse(readFileSync(path, 'utf8'));
}

export function writeJson(path, value) {
  writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`, 'utf8');
}

export function artifactPaths(root = packageRoot, packageJson = readJson(join(root, 'package.json'))) {
  const stem = `LingXi-Code-${packageJson.version}-mac-${TARGET_ARCH}`;
  const releaseRoot = join(root, 'dist');
  return {
    stem,
    releaseRoot,
    appContainer: join(releaseRoot, stem),
    appPath: join(releaseRoot, stem, `${APP_NAME}.app`),
    zipPath: join(releaseRoot, `${stem}.zip`),
    checksumPath: join(releaseRoot, `${stem}.zip.sha256`),
  };
}

export const DESKTOP_TARGETS = Object.freeze({
  'darwin-arm64': { platform: 'darwin', arch: 'arm64', extension: 'zip' },
  'darwin-x64': { platform: 'darwin', arch: 'x64', extension: 'zip' },
  'win32-x64': { platform: 'win32', arch: 'x64', extension: 'zip' },
  'linux-x64': { platform: 'linux', arch: 'x64', extension: 'tar.gz' },
});

export function desktopTarget(platform, arch) {
  const id = `${platform}-${arch}`;
  const target = DESKTOP_TARGETS[id];
  if (!target) {
    throw new Error(`unsupported Desktop target ${id}; expected ${Object.keys(DESKTOP_TARGETS).join(', ')}`);
  }
  return { ...target, id };
}

export function desktopArtifactPaths(
  root = packageRoot,
  platform,
  arch,
) {
  const target = desktopTarget(platform, arch);
  const releaseRoot = join(root, 'dist');
  const artifactName = `${target.id}.${target.extension}`;
  return {
    ...target,
    releaseRoot,
    stageRoot: join(releaseRoot, 'stage', target.id),
    payloadRoot: join(releaseRoot, 'stage', target.id, target.platform === 'darwin' ? `${APP_NAME}.app` : APP_NAME),
    artifactName,
    artifactPath: join(releaseRoot, artifactName),
    checksumPath: join(releaseRoot, `${artifactName}.sha256`),
    metadataPath: join(releaseRoot, `${target.id}.build-metadata.json`),
  };
}

export function parseDesktopTargetArgs(argv) {
  let platform;
  let arch;
  for (let index = 0; index < argv.length; index += 1) {
    const value = argv[index];
    if (value === '--platform') platform = argv[++index];
    else if (value === '--arch') arch = argv[++index];
    else throw new Error(`unknown package argument: ${value}`);
  }
  if (!platform || !arch) throw new Error('both --platform and --arch are required');
  return desktopTarget(platform, arch);
}

export function runtimePackageJson(source) {
  const {
    devDependencies: _devDependencies,
    scripts: _scripts,
    files: _files,
    packageManager: _packageManager,
    ...runtime
  } = source;
  return runtime;
}

export function assertArm64Architecture(architectures, label) {
  const values = Array.isArray(architectures)
    ? architectures
    : String(architectures).trim().split(/\s+/).filter(Boolean);
  if (!values.includes(TARGET_ARCH)) {
    throw new Error(`${label} has architecture ${values.join(', ') || 'unknown'}; ${TARGET_ARCH} is required`);
  }
}

export function binaryArchitectures(path) {
  try {
    return execFileSync('/usr/bin/lipo', ['-archs', path], { encoding: 'utf8' }).trim().split(/\s+/);
  } catch (error) {
    const detail = error?.stderr?.toString().trim() || error?.message || String(error);
    throw new Error(`cannot inspect Mach-O architecture for ${path}: ${detail}`);
  }
}

export function assertArm64Executable(path, label) {
  if (!existsSync(path)) {
    throw new Error(`${label} is missing: ${path}`);
  }
  const mode = statSync(path).mode;
  if ((mode & 0o111) === 0) {
    throw new Error(`${label} is not executable: ${path}`);
  }
  assertArm64Architecture(binaryArchitectures(path), label);
}

export function commandAvailable(path) {
  try {
    return existsSync(path) && (statSync(path).mode & 0o111) !== 0;
  } catch {
    return false;
  }
}

function packageRootFor(name, fromRoot) {
  let candidate = realpathSync(fromRoot);
  for (;;) {
    const dependencyRoot = join(candidate, 'node_modules', ...name.split('/'));
    const manifest = join(dependencyRoot, 'package.json');
    if (existsSync(manifest) && readJson(manifest).name === name) {
      return realpathSync(dependencyRoot);
    }
    const parent = dirname(candidate);
    if (parent === candidate) {
      throw new Error(`cannot locate package root for ${name} from ${fromRoot}`);
    }
    candidate = parent;
  }
}

const OMIT_FROM_RUNTIME_PACKAGE = new Set([
  '.git',
  '.github',
  '.npmrc',
  'coverage',
  'docs',
  'examples',
  'node_modules',
  'test',
  'tests',
]);

function copyPath(source, destination) {
  const sourceStat = lstatSync(source);
  mkdirSync(dirname(destination), { recursive: true });
  if (sourceStat.isSymbolicLink()) {
    symlinkSync(readlinkSync(source), destination);
    return;
  }
  cpSync(source, destination, {
    dereference: false,
    errorOnExist: true,
    force: false,
    preserveTimestamps: true,
    recursive: sourceStat.isDirectory(),
    verbatimSymlinks: true,
  });
}

function copyPackagePayload(sourceRoot, destinationRoot, manifest) {
  mkdirSync(destinationRoot, { recursive: true });
  // npm's `files` field accepts glob patterns (for example `lib/*.js`). Copy
  // the complete top-level directory containing a nested glob so the runtime
  // cannot be silently truncated, while still honoring a package's allowlist.
  const entries = Array.isArray(manifest.files) && manifest.files.length > 0
    ? [...new Set(manifest.files.flatMap((entry) => {
        const normalized = String(entry).replaceAll('\\', '/').replace(/^\.\//, '');
        if (!normalized || normalized.startsWith('/') || normalized.split('/').includes('..')) {
          throw new Error(`unsafe package files entry in ${manifest.name}: ${entry}`);
        }
        if (!/[?*[]/.test(normalized)) return [normalized];
        const [topLevel] = normalized.split('/');
        if (!/[?*[]/.test(topLevel)) return [topLevel];
        const pattern = new RegExp(`^${topLevel
          .replace(/[.+^${}()|\\]/g, '\\$&')
          .replaceAll('*', '.*')
          .replaceAll('?', '.')}$`);
        return readdirSync(sourceRoot).filter((candidate) => pattern.test(candidate));
      }))]
    : readdirSync(sourceRoot).filter((entry) => !OMIT_FROM_RUNTIME_PACKAGE.has(entry));

  for (const entry of entries) {
    const source = join(sourceRoot, entry);
    if (existsSync(source) && basename(source) !== 'package.json') {
      copyPath(source, join(destinationRoot, entry));
    }
  }
  for (const entry of readdirSync(sourceRoot)) {
    if (/^(?:licen[cs]e|notice|readme)(?:\..*)?$/i.test(entry)) {
      const destination = join(destinationRoot, entry);
      if (!existsSync(destination)) {
        copyPath(join(sourceRoot, entry), destination);
      }
    }
  }
  writeJson(join(destinationRoot, 'package.json'), runtimePackageJson(manifest));
}

export function copyProductionDependencies(dependencies, sourceRoot, targetNodeModules) {
  const copied = new Map();

  function copyOne(name, fromRoot, optional = false) {
    let dependencyRoot;
    try {
      dependencyRoot = packageRootFor(name, fromRoot);
    } catch (error) {
      if (optional) return;
      throw error;
    }
    const manifest = readJson(join(dependencyRoot, 'package.json'));
    const key = `${manifest.name}@${manifest.version}`;
    if (copied.has(name)) {
      if (copied.get(name) !== key) {
        throw new Error(`cannot flatten conflicting runtime versions for ${name}`);
      }
      return;
    }
    copied.set(name, key);
    copyPackagePayload(dependencyRoot, join(targetNodeModules, ...name.split('/')), manifest);
    for (const child of Object.keys(manifest.dependencies ?? {}).sort()) {
      copyOne(child, dependencyRoot);
    }
    for (const child of Object.keys(manifest.optionalDependencies ?? {}).sort()) {
      copyOne(child, dependencyRoot, true);
    }
  }

  mkdirSync(targetNodeModules, { recursive: true });
  for (const name of Object.keys(dependencies ?? {}).sort()) {
    copyOne(name, sourceRoot);
  }
  return copied;
}

export function resetDirectory(path) {
  rmSync(path, { force: true, recursive: true });
  mkdirSync(path, { recursive: true });
}

export function renameElectronExecutable(appPath) {
  const oldPath = join(appPath, 'Contents', 'MacOS', 'Electron');
  const newPath = join(appPath, 'Contents', 'MacOS', APP_NAME);
  if (!existsSync(oldPath)) {
    throw new Error(`Electron executable is missing: ${oldPath}`);
  }
  renameSync(oldPath, newPath);
  chmodSync(newPath, 0o755);
  return newPath;
}

export function rewriteInfoPlist(plistPath, version, bundleId = BUNDLE_ID) {
  const replacements = {
    CFBundleDisplayName: APP_NAME,
    CFBundleExecutable: APP_NAME,
    CFBundleIconFile: 'icon.icns',
    CFBundleIdentifier: bundleId,
    CFBundleName: APP_NAME,
    CFBundleShortVersionString: version,
    CFBundleVersion: version,
    NSMicrophoneUsageDescription: NS_MICROPHONE_USAGE_DESCRIPTION,
  };
  for (const [key, value] of Object.entries(replacements)) {
    execFileSync('/usr/bin/plutil', ['-replace', key, '-string', value, plistPath], { stdio: 'pipe' });
  }
  try {
    execFileSync('/usr/bin/plutil', ['-remove', 'ElectronAsarIntegrity', plistPath], { stdio: 'pipe' });
  } catch {
    // Electron may eventually omit this key; its absence is the desired state.
  }
  for (const key of [
    'NSAppTransportSecurity',
    'NSAudioCaptureUsageDescription',
    'NSBluetoothAlwaysUsageDescription',
    'NSBluetoothPeripheralUsageDescription',
    'NSCameraUsageDescription',
  ]) {
    try {
      execFileSync('/usr/bin/plutil', ['-remove', key, plistPath], { stdio: 'pipe' });
    } catch {
      // These keys come from the Electron template and may disappear upstream.
    }
  }
}

export function walkTree(root) {
  const result = [];
  function visit(path) {
    result.push(path);
    if (!lstatSync(path).isDirectory()) return;
    for (const entry of readdirSync(path).sort()) {
      visit(join(path, entry));
    }
  }
  visit(root);
  return result;
}

export function normalizeTimestamps(root, epochSeconds = FIXED_MTIME_SECONDS) {
  for (const path of walkTree(root).reverse()) {
    normalizeTimestamp(path, epochSeconds);
  }
}

export function normalizeTimestamp(path, epochSeconds = FIXED_MTIME_SECONDS) {
  const time = new Date(epochSeconds * 1000);
  const stat = lstatSync(path);
  if (stat.isSymbolicLink()) lutimesSync(path, time, time);
  else utimesSync(path, time, time);
}

export function createDeterministicZip(appPath, zipPath) {
  if (!commandAvailable('/usr/bin/zip')) {
    throw new Error('/usr/bin/zip is required to produce the macOS artifact');
  }
  unlinkIfPresent(zipPath);
  const parent = dirname(appPath);
  const entries = walkTree(appPath).map((path) => relative(parent, path).split(sep).join('/'));
  const result = spawnSync('/usr/bin/zip', ['-q', '-X', '-y', zipPath, '-@'], {
    cwd: parent,
    encoding: 'utf8',
    input: `${entries.join('\n')}\n`,
  });
  if (result.status !== 0) {
    throw new Error(`zip failed: ${(result.stderr || result.stdout).trim()}`);
  }
}

export function sha256File(path) {
  const hash = createHash('sha256');
  const fd = openSync(path, 'r');
  const buffer = Buffer.allocUnsafe(1024 * 1024);
  try {
    for (;;) {
      const count = readSync(fd, buffer, 0, buffer.length, null);
      if (count === 0) break;
      hash.update(buffer.subarray(0, count));
    }
  } finally {
    closeSync(fd);
  }
  return hash.digest('hex');
}

export function unlinkIfPresent(path) {
  try {
    unlinkSync(path);
  } catch (error) {
    if (error?.code !== 'ENOENT') throw error;
  }
}

export function forbiddenContentRules(root = packageRoot) {
  const escaped = (value) => value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const canary = process.env['LINGXI_PACKAGE_SECRET_CANARY'] || DEFAULT_SECRET_CANARY;
  const extraPaths = (process.env['LINGXI_PACKAGE_FORBIDDEN_PATHS'] ?? '')
    .split(':')
    .map((value) => value.trim())
    .filter(Boolean);
  const paths = [root, ...extraPaths];
  const home = homedir();
  if (home && home !== '/') {
    paths.push(join(home, 'Projects'), join(home, 'Developer'), join(home, 'workspace'), join(home, 'worktrees'));
  }
  return [
    ...new Set(paths.map((value) => resolve(value))),
  ].map((value) => ({ label: `absolute developer path ${value}`, pattern: new RegExp(escaped(value), 'g') })).concat([
    // The official prebuilt sherpa-onnx static archive embeds its public
    // GitHub Actions checkout prefix in C++ assertion strings. It identifies
    // upstream source, not a local developer or secret-bearing path.
    { label: 'absolute macOS user path', pattern: /\/Users\/(?!runner\/work\/(?:sherpa-onnx\/sherpa-onnx|onnxruntime-libs\/onnxruntime-libs)\/)[A-Za-z0-9._-]+\//g },
    { label: 'package secret canary', pattern: new RegExp(escaped(canary), 'g') },
    { label: 'API secret', pattern: /(?:sk-ant-api\d{2}|sk-proj|sk-svcacct)-[A-Za-z0-9_-]{16,}/g },
    { label: 'API secret assignment', pattern: /(?:ANTHROPIC|OPENAI|LINGXI)_API_KEY\s*[=:]\s*["']?[A-Za-z0-9_-]{12,}/g },
  ]);
}

export function scanTreeForForbiddenContent(root, rules = forbiddenContentRules()) {
  for (const path of walkTree(root)) {
    const stat = lstatSync(path);
    if (stat.isSymbolicLink()) {
      const target = readlinkSync(path);
      if (isAbsolute(target)) {
        throw new Error(`absolute symlink target found in ${path}: ${target}`);
      }
      continue;
    }
    if (!stat.isFile()) continue;
    const fd = openSync(path, 'r');
    const buffer = Buffer.allocUnsafe(1024 * 1024);
    let carry = '';
    try {
      for (;;) {
        const count = readSync(fd, buffer, 0, buffer.length, null);
        if (count === 0) break;
        const text = carry + buffer.subarray(0, count).toString('utf8');
        for (const rule of rules) {
          rule.pattern.lastIndex = 0;
          if (rule.pattern.test(text)) {
            throw new Error(`${rule.label} found in ${path}`);
          }
        }
        carry = text.slice(-4096);
      }
    } finally {
      closeSync(fd);
    }
  }
}

export function validateZipEntries(entries, appName = `${APP_NAME}.app`) {
  if (entries.length === 0) throw new Error('ZIP contains no entries');
  for (const entry of entries) {
    if (entry.startsWith('/') || entry.includes('\\') || entry.split('/').includes('..')) {
      throw new Error(`unsafe ZIP entry: ${entry}`);
    }
    if (entry !== appName && !entry.startsWith(`${appName}/`)) {
      throw new Error(`ZIP entry is outside ${appName}: ${entry}`);
    }
  }
}

export function copyElectronApp(source, destination) {
  if (!existsSync(source)) {
    throw new Error(`Electron.app is missing; run npm install first: ${source}`);
  }
  cpSync(source, destination, {
    dereference: false,
    errorOnExist: true,
    force: false,
    preserveTimestamps: true,
    recursive: true,
    verbatimSymlinks: true,
  });
}

export function formatError(error) {
  return error instanceof Error ? error.message : String(error);
}
