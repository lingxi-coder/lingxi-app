import { copyFileSync, existsSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { execFileSync } from 'node:child_process';

import { assertArm64Executable, formatError, repoRoot } from './package-support.mjs';

export const AUDIO_HELPER_APP = 'LingXiAudioHelper.app';
export const AUDIO_HELPER_EXECUTABLE = 'LingXiAudioHelper';
export const AUDIO_HELPER_BUNDLE_ID = 'com.lingxi.code.audio-helper';
export const AUDIO_HELPER_MINIMUM_SYSTEM_VERSION = '13.0';
export const AUDIO_HELPER_USAGE_DESCRIPTIONS = Object.freeze({
  microphone: 'LingXi Audio Helper records audio only while you actively use Desktop voice features.',
  speechRecognition: 'LingXi Audio Helper uses speech recognition to transcribe your spoken prompts on this Mac.',
});

export function audioHelperIdentifiers(channel = 'production') {
  if (channel !== 'production' && channel !== 'development') {
    throw new Error(`unsupported audio helper channel: ${channel}`);
  }
  const suffix = channel === 'production' ? '' : '.development';
  return {
    bundleId: `${AUDIO_HELPER_BUNDLE_ID}${suffix}`,
    desktopBundleId: `com.lingxi.code${suffix}`,
  };
}

const HELPER_SOURCE_ROOT = resolve(repoRoot, 'clients', 'electron', 'native', 'audio-helper');

const SWIFT_TARGET_BY_TRIPLE = Object.freeze({
  'aarch64-apple-darwin': 'arm64-apple-macos13.0',
  'x86_64-apple-darwin': 'x86_64-apple-macos13.0',
});

function run(command, args) {
  execFileSync(command, args, { cwd: HELPER_SOURCE_ROOT, stdio: 'inherit' });
}

function plistEscape(value) {
  return value.replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;').replaceAll('"', '&quot;');
}

function resolveSwiftTarget(targetTriple, fallbackArch = process.arch) {
  if (targetTriple) return SWIFT_TARGET_BY_TRIPLE[targetTriple] ?? null;
  if (fallbackArch === 'arm64') return SWIFT_TARGET_BY_TRIPLE['aarch64-apple-darwin'];
  if (fallbackArch === 'x64') return SWIFT_TARGET_BY_TRIPLE['x86_64-apple-darwin'];
  return null;
}

export function writeAudioHelperInfoPlist(contentsRoot, { bundleId, version }) {
  writeFileSync(join(contentsRoot, 'Info.plist'), `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "https://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleExecutable</key><string>${plistEscape(AUDIO_HELPER_EXECUTABLE)}</string>
  <key>CFBundleIdentifier</key><string>${plistEscape(bundleId)}</string>
  <key>CFBundleName</key><string>LingXiAudioHelper</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${plistEscape(version)}</string>
  <key>CFBundleVersion</key><string>${plistEscape(version)}</string>
  <key>LSMinimumSystemVersion</key><string>${plistEscape(AUDIO_HELPER_MINIMUM_SYSTEM_VERSION)}</string>
  <key>LSUIElement</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>${plistEscape(AUDIO_HELPER_USAGE_DESCRIPTIONS.microphone)}</string>
  <key>NSSpeechRecognitionUsageDescription</key><string>${plistEscape(AUDIO_HELPER_USAGE_DESCRIPTIONS.speechRecognition)}</string>
</dict></plist>
`, 'utf8');
}

export function buildAudioHelperApp(outputRoot, {
  channel = 'production',
  profilePath,
  version,
  targetTriple,
  target,
}) {
  rmSync(outputRoot, { recursive: true, force: true });
  const appRoot = join(outputRoot, 'LingXiAudioHelper.app');
  const contents = join(appRoot, 'Contents');
  const macOS = join(contents, 'MacOS');
  const resources = join(contents, 'Resources');
  mkdirSync(macOS, { recursive: true });
  mkdirSync(resources, { recursive: true });

  try {
    const helperBinary = join(macOS, AUDIO_HELPER_EXECUTABLE);
    const swiftTarget = resolveSwiftTarget(targetTriple, target);
    run('/usr/bin/xcrun', [
      'swift',
      'build',
      '-c',
      'release',
      '-Xswiftc',
      '-file-prefix-map',
      '-Xswiftc',
      `${repoRoot}=/workspace`,
      '-Xswiftc',
      '-debug-prefix-map',
      '-Xswiftc',
      `${repoRoot}=/workspace`,
      ...(swiftTarget ? ['-Xswiftc', '-target', '-Xswiftc', swiftTarget] : []),
    ]);
    const builtBinary = join(HELPER_SOURCE_ROOT, '.build', 'release', AUDIO_HELPER_EXECUTABLE);
    assertArm64Executable(builtBinary, 'audio helper executable');
    copyFileSync(builtBinary, helperBinary);
    // SwiftPM leaves local object-file symbols (N_OSO) in release binaries;
    // strip them before signing so the packaged helper cannot disclose the
    // developer checkout while retaining externally useful symbols.
    run('/usr/bin/strip', ['-S', '-x', helperBinary]);

    const identifiers = audioHelperIdentifiers(channel);
    if (profilePath) copyFileSync(profilePath, join(contents, 'embedded.provisionprofile'));
    writeAudioHelperInfoPlist(contents, {
      bundleId: identifiers.bundleId,
      version,
    });
    return appRoot;
  } catch (error) {
    throw new Error(`build audio helper failed: ${formatError(error)}`);
  }
}

export function ensureAudioHelperResourceLayout(root) {
  const appPath = join(root, AUDIO_HELPER_APP);
  if (!existsSync(join(appPath, 'Contents', 'MacOS', AUDIO_HELPER_EXECUTABLE))) {
    throw new Error(`audio helper resource is missing: ${appPath}`);
  }
}
