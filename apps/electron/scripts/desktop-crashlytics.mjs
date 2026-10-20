import { execFileSync } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export const FIREBASE_PROJECT = 'lingxi-5e4b2';
export const FIREBASE_SDK_VERSION = '12.19.2';

export function validateCrashlyticsConfig(config, bundleId) {
  if (config.PROJECT_ID !== FIREBASE_PROJECT || config.BUNDLE_ID !== bundleId
      || !/^1:55470483519:ios:[a-f0-9]+$/.test(config.GOOGLE_APP_ID ?? '')) {
    throw new Error('Crashlytics requires the matching Apple app in lingxi-5e4b2; a Web app configuration is not valid');
  }
  return config;
}

export function buildCrashlyticsBridge() {
  const source = join(root, 'native', 'crashlytics');
  const scratch = join(root, '.native-cache', 'crashlytics');
  mkdirSync(scratch, { recursive: true });
  const derived = join(root, '.native-cache', 'crashlytics-xcode');
  execFileSync('xcodegen', ['generate', '--spec', join(source, 'project.yml')], { stdio: 'inherit' });
  const resolvedDirectory = join(source, 'LingXiDesktopCrashlytics.xcodeproj', 'project.xcworkspace', 'xcshareddata', 'swiftpm');
  mkdirSync(resolvedDirectory, { recursive: true });
  cpSync(join(source, 'Package.resolved'), join(resolvedDirectory, 'Package.resolved'));
  execFileSync('xcodebuild', ['-project', join(source, 'LingXiDesktopCrashlytics.xcodeproj'),
    '-scheme', 'LingXiDesktopCrashlytics', '-configuration', 'Release',
    '-destination', 'generic/platform=macOS', '-derivedDataPath', derived,
    '-clonedSourcePackagesDirPath', scratch, '-onlyUsePackageVersionsFromResolvedFile',
    `OTHER_CFLAGS=$(inherited) -ffile-prefix-map=${root}=.`,
    `OTHER_SWIFT_FLAGS=$(inherited) -debug-prefix-map ${root}=.`,
    'ARCHS=arm64', 'CODE_SIGNING_ALLOWED=NO', 'build'], { stdio: 'inherit' });
  const bin = join(derived, 'Build', 'Products', 'Release');
  // N-API is ABI-stable; no V8/Electron C++ ABI is used by this addon.
  const headers = process.env.LINGXI_NODE_API_HEADERS ?? resolve(dirname(process.execPath), '..', 'include', 'node');
  if (!headers || !existsSync(join(headers, 'node_api.h'))) {
    throw new Error('Set LINGXI_NODE_API_HEADERS to a Node-API include directory (node_api.h)');
  }
  const addon = join(bin, 'crashlytics.node');
  const object = join(scratch, 'addon.o');
  execFileSync('xcrun', ['clang', '-arch', 'arm64', '-mmacosx-version-min=12.0', '-g',
    '-DNAPI_VERSION=8', '-I', headers, '-c', join(source, 'addon.c'), '-o', object], { stdio: 'inherit' });
  execFileSync('xcrun', ['clang', '-arch', 'arm64', '-mmacosx-version-min=12.0', '-g',
    '-bundle', '-undefined', 'dynamic_lookup', object,
    '-F', bin, '-framework', 'LingXiDesktopCrashlytics', '-Wl,-rpath,@loader_path', '-o', addon], { stdio: 'inherit' });
  execFileSync('xcrun', ['dsymutil', addon, '-o', join(bin, 'crashlytics.node.dSYM')], { stdio: 'inherit' });
  return { bin, scratch };
}

export function stageCrashlytics(resources, { channel, bundleId, symbolsDirectory }) {
  const configPath = join(root, 'native', 'crashlytics', 'Firebase', channel, 'GoogleService-Info.plist');
  if (!existsSync(configPath)) throw new Error(`Missing Desktop Apple Firebase configuration: ${configPath}`);
  validateCrashlyticsConfig(JSON.parse(execFileSync('/usr/bin/plutil', ['-convert', 'json', '-o', '-', configPath], { encoding: 'utf8' })), bundleId);
  const { bin, scratch } = buildCrashlyticsBridge();
  mkdirSync(symbolsDirectory, { recursive: true });
  for (const name of ['LingXiDesktopCrashlytics.framework', 'crashlytics.node']) cpSync(join(bin, `${name}.dSYM`), join(symbolsDirectory, `${name}.dSYM`), { recursive: true });
  const destination = join(resources, 'crashlytics');
  mkdirSync(destination, { recursive: true });
  for (const name of ['LingXiDesktopCrashlytics.framework', 'crashlytics.node']) cpSync(join(bin, name), join(destination, name), { recursive: true, verbatimSymlinks: true });
  // The C ABI needs no Swift import metadata or development headers. Keep
  // source-location metadata in the archived symbols, not the runtime bundle.
  for (const name of ['Modules', 'Headers']) {
    rmSync(join(destination, 'LingXiDesktopCrashlytics.framework', name), { recursive: true, force: true });
    rmSync(join(destination, 'LingXiDesktopCrashlytics.framework', 'Versions', 'A', name), { recursive: true, force: true });
  }
  for (const path of [join(destination, 'LingXiDesktopCrashlytics.framework', 'LingXiDesktopCrashlytics'), join(destination, 'crashlytics.node')]) {
    execFileSync('xcrun', ['strip', '-S', path], { stdio: 'inherit' });
  }
  // SPM resource bundles include the SDKs' Apple privacy manifests.
  for (const name of readdirSync(bin).filter(name => name.endsWith('.bundle'))) cpSync(join(bin, name), join(resources, name), { recursive: true });
  cpSync(configPath, join(destination, 'GoogleService-Info.plist'));
  writeFileSync(join(destination, 'configuration.json'), JSON.stringify({ channel, sdkVersion: FIREBASE_SDK_VERSION }) + '\n');
  return { destination, scratch };
}

export function signCrashlytics(resources, identity) {
  const directory = join(resources, 'crashlytics');
  for (const name of ['LingXiDesktopCrashlytics.framework', 'crashlytics.node']) {
    execFileSync('/usr/bin/codesign', ['--force', '--sign', identity, '--options', 'runtime', '--timestamp', join(directory, name)], { stdio: 'inherit' });
  }
}

export function verifyCrashlytics(resources, bundleId) {
  const directory = join(resources, 'crashlytics');
  for (const name of ['LingXiDesktopCrashlytics.framework', 'crashlytics.node', 'configuration.json']) {
    if (!existsSync(join(directory, name))) throw new Error(`Missing Crashlytics resource: ${name}`);
  }
  const config = JSON.parse(execFileSync('/usr/bin/plutil', ['-convert', 'json', '-o', '-', join(directory, 'GoogleService-Info.plist')], { encoding: 'utf8' }));
  validateCrashlyticsConfig(config, bundleId);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) buildCrashlyticsBridge();
