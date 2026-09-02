import { copyFileSync, cpSync, existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { execFileSync, spawnSync } from 'node:child_process';

import { formatError, repoRoot } from './package-support.mjs';

export const BROKER_BUNDLE_ID = 'com.lingxi.code.credential-broker';
export const BROKER_CLIENT_IDENTIFIER = 'com.lingxi.code.credential-client';
export const BRIDGE_SERVER_IDENTIFIER = 'com.lingxi.code.bridge-server';
export const BROKER_EXECUTABLE = 'LingXiCredentialBroker';
export const BROKER_RESOURCE_DIRNAME = 'credential-broker';
export const BROKER_MACH_SERVICE = BROKER_BUNDLE_ID;
export const BROKER_ALLOWED_CALLERS = Object.freeze([
  'com.lingxi.code',
  'com.lingxi.code.cli',
]);
export const BROKER_CHANNEL = 'production';

export function brokerIdentifiers(channel = BROKER_CHANNEL) {
  if (channel !== 'production' && channel !== 'development') {
    throw new Error(`unsupported credential broker channel: ${channel}`);
  }
  const suffix = channel === 'production' ? '' : '.development';
  return {
    brokerBundleId: `${BROKER_BUNDLE_ID}${suffix}`,
    clientIdentifier: `${BROKER_CLIENT_IDENTIFIER}${suffix}`,
    bridgeServerIdentifier: `${BRIDGE_SERVER_IDENTIFIER}${suffix}`,
    desktopBundleId: `com.lingxi.code${suffix}`,
    machService: `${BROKER_MACH_SERVICE}${suffix}`,
    allowedCallers: BROKER_ALLOWED_CALLERS.map((identifier) => `${identifier}${suffix}`),
  };
}

const SWIFT_SOURCE_ROOT = resolve(repoRoot, 'lingxi-code', 'platforms', 'macos-credential-broker');
const SWIFT_TARGET_BY_TRIPLE = Object.freeze({
  'aarch64-apple-darwin': 'arm64-apple-macos13.0',
  'x86_64-apple-darwin': 'x86_64-apple-macos13.0',
});

function run(command, args) {
  execFileSync(command, args, { stdio: 'inherit' });
}

function plistEscape(value) {
  return value.replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;').replaceAll('"', '&quot;');
}

export function renderLaunchAgentTemplate(
  executablePath = '@BROKER_EXECUTABLE_PATH@',
  channel = BROKER_CHANNEL,
) {
  const { machService } = brokerIdentifiers(channel);
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "https://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>${plistEscape(machService)}</string>
  <key>MachServices</key><dict><key>${plistEscape(machService)}</key><true/></dict>
  <key>ProgramArguments</key><array><string>${plistEscape(executablePath)}</string></array>
  <key>RunAtLoad</key><false/>
  <key>ProcessType</key><string>Background</string>
</dict></plist>
`;
}

export function brokerManifest(version, channel = BROKER_CHANNEL) {
  return {
    version,
    protocol_version: 1,
    channel,
  };
}

function expectedArchitecture(targetTriple, fallbackArch = process.arch) {
  if (targetTriple === 'aarch64-apple-darwin') return 'arm64';
  if (targetTriple === 'x86_64-apple-darwin') return 'x86_64';
  if (fallbackArch === 'arm64') return 'arm64';
  if (fallbackArch === 'x64') return 'x86_64';
  return null;
}

function binaryArchitectures(path) {
  return execFileSync('/usr/bin/lipo', ['-archs', path], { encoding: 'utf8' }).trim().split(/\s+/).filter(Boolean);
}

function assertExpectedArchitecture(path, label, targetTriple, fallbackArch) {
  const expected = expectedArchitecture(targetTriple, fallbackArch);
  if (!expected) return;
  const actual = binaryArchitectures(path);
  if (!actual.includes(expected)) {
    throw new Error(`${label} has architecture ${actual.join(', ') || 'unknown'}; expected ${expected}`);
  }
}

export function buildCredentialBrokerResources(outputRoot, {
  channel = BROKER_CHANNEL,
  profilePath,
  version,
  targetTriple,
  target,
}) {
  rmSync(outputRoot, { recursive: true, force: true });
  mkdirSync(outputRoot, { recursive: true });
  const binDir = join(outputRoot, 'bin');
  const brokerApp = join(outputRoot, 'LingXiCredentialBroker.app');
  const brokerContents = join(brokerApp, 'Contents');
  const brokerMacOs = join(brokerContents, 'MacOS');
  const brokerResources = join(brokerContents, 'Resources');
  mkdirSync(binDir, { recursive: true });
  mkdirSync(brokerMacOs, { recursive: true });
  mkdirSync(brokerResources, { recursive: true });

  const clientPath = join(binDir, 'lingxi-credential-client');
  const brokerExecutablePath = join(brokerMacOs, BROKER_EXECUTABLE);
  const swiftTarget = resolveSwiftTarget(targetTriple, target);
  run('/usr/bin/xcrun', [
    'swiftc',
    '-O',
    ...(swiftTarget ? ['-target', swiftTarget] : []),
    '-framework', 'Foundation',
    '-framework', 'Security',
    join(SWIFT_SOURCE_ROOT, 'BrokerCommon.swift'),
    join(SWIFT_SOURCE_ROOT, 'CredentialClientMain.swift'),
    '-o',
    clientPath,
  ]);
  assertExpectedArchitecture(clientPath, 'credential broker client', targetTriple, target);
  run('/usr/bin/xcrun', [
    'swiftc',
    '-O',
    ...(swiftTarget ? ['-target', swiftTarget] : []),
    '-framework', 'Foundation',
    '-framework', 'Security',
    join(SWIFT_SOURCE_ROOT, 'BrokerCommon.swift'),
    join(SWIFT_SOURCE_ROOT, 'CredentialBrokerMain.swift'),
    '-o',
    brokerExecutablePath,
  ]);
  assertExpectedArchitecture(brokerExecutablePath, 'credential broker app', targetTriple, target);

  const identifiers = brokerIdentifiers(channel);
  const manifest = brokerManifest(version, channel);
  writeFileSync(join(outputRoot, 'broker-manifest.json'), `${JSON.stringify(manifest, null, 2)}\n`, 'utf8');
  copyFileSync(join(outputRoot, 'broker-manifest.json'), join(brokerResources, 'broker-manifest.json'));
  writeFileSync(
    join(outputRoot, 'LingXiCredentialBroker.launchd.plist'),
    renderLaunchAgentTemplate('@BROKER_EXECUTABLE_PATH@', channel),
    'utf8',
  );
  if (profilePath) {
    copyFileSync(profilePath, join(brokerContents, 'embedded.provisionprofile'));
  }
  writeFileSync(join(brokerContents, 'Info.plist'), `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "https://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleExecutable</key><string>${plistEscape(BROKER_EXECUTABLE)}</string>
  <key>CFBundleIdentifier</key><string>${plistEscape(identifiers.brokerBundleId)}</string>
  <key>CFBundleName</key><string>LingXiCredentialBroker</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${plistEscape(version)}</string>
  <key>CFBundleVersion</key><string>${plistEscape(version)}</string>
  <key>LSBackgroundOnly</key><true/>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
</dict></plist>
`, 'utf8');
}

export function resolveSwiftTarget(targetTriple, fallbackArch = process.arch) {
  if (targetTriple) {
    return SWIFT_TARGET_BY_TRIPLE[targetTriple] ?? null;
  }
  if (fallbackArch === 'arm64') return SWIFT_TARGET_BY_TRIPLE['aarch64-apple-darwin'];
  if (fallbackArch === 'x64') return SWIFT_TARGET_BY_TRIPLE['x86_64-apple-darwin'];
  return null;
}

export function readProvisioningProfile(path) {
  const plist = execFileSync('/usr/bin/security', ['cms', '-D', '-i', path]);
  const scratch = mkdtempSync(join(tmpdir(), 'lingxi-profile-'));
  const decodedPath = join(scratch, 'profile.plist');
  try {
    writeFileSync(decodedPath, plist, { mode: 0o600 });
    const read = (key) => execFileSync('/usr/libexec/PlistBuddy', ['-c', `Print :${key}`, decodedPath], {
      encoding: 'utf8',
    }).trim();
    return {
      TeamIdentifier: [read('TeamIdentifier:0')],
      ExpirationDate: read('ExpirationDate'),
      Entitlements: {
        'com.apple.application-identifier': read('Entitlements:com.apple.application-identifier'),
      },
    };
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

export function validateProvisioningProfile(profilePath, teamId, bundleId) {
  const profile = readProvisioningProfile(profilePath);
  const profileTeams = Array.isArray(profile.TeamIdentifier) ? profile.TeamIdentifier : [];
  const applicationIdentifier = profile.Entitlements?.['com.apple.application-identifier'];
  const expected = `${teamId}.${bundleId}`;
  const expiration = Date.parse(profile.ExpirationDate);
  if (!Number.isFinite(expiration) || expiration <= Date.now()) {
    throw new Error(`provisioning profile ${profilePath} is expired or has no valid expiration date`);
  }
  if (!profileTeams.includes(teamId)) {
    throw new Error(`provisioning profile ${profilePath} team does not match ${teamId}`);
  }
  if (applicationIdentifier !== expected && applicationIdentifier !== `${teamId}.*`) {
    throw new Error(`provisioning profile ${profilePath} does not authorize ${expected}`);
  }
}

export function validateSignedEntitlements(entitlements, teamId, bundleId) {
  const expectedApplicationIdentifier = `${teamId}.${bundleId}`;
  if (entitlements?.['com.apple.application-identifier'] !== expectedApplicationIdentifier) {
    throw new Error(`signed entitlements do not contain ${expectedApplicationIdentifier}`);
  }
  if (entitlements?.['com.apple.developer.team-identifier'] !== teamId) {
    throw new Error(`signed entitlements do not contain TeamIdentifier ${teamId}`);
  }
}

export function verifySignedEntitlements(path, teamId, bundleId) {
  const plist = execFileSync('/usr/bin/codesign', ['--display', '--xml', '--entitlements', '-', path]);
  const entitlements = JSON.parse(execFileSync('/usr/bin/plutil', ['-convert', 'json', '-o', '-', '-'], {
    input: plist,
    encoding: 'utf8',
  }));
  validateSignedEntitlements(entitlements, teamId, bundleId);
}

export function writeEntitlements(path, { teamId, bundleId, allowJit = false, disableLibraryValidation = false }) {
  const rows = [
    ['com.apple.application-identifier', `${teamId}.${bundleId}`],
    ['com.apple.developer.team-identifier', teamId],
  ];
  if (allowJit) rows.push(['com.apple.security.cs.allow-jit', true]);
  if (allowJit) rows.push(['com.apple.security.cs.allow-unsigned-executable-memory', true]);
  if (disableLibraryValidation) rows.push(['com.apple.security.cs.disable-library-validation', true]);
  const body = rows.map(([key, value]) => {
    const rendered = value === true ? '<true/>' : `<string>${plistEscape(String(value))}</string>`;
    return `  <key>${plistEscape(String(key))}</key>${rendered}`;
  }).join('\n');
  writeFileSync(path, `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "https://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
${body}
</dict></plist>
`, 'utf8');
}

export function signBinary(path, identity, identifier, entitlementsPath) {
  const args = [
    '--force',
    '--sign', identity,
    identity === '-' ? '--timestamp=none' : '--timestamp',
    ...(identity === '-' ? [] : ['--options', 'runtime']),
    '-i', identifier,
  ];
  if (entitlementsPath) args.push('--entitlements', entitlementsPath);
  args.push(path);
  run('/usr/bin/codesign', args);
}

export function signAppBundle(path, identity, entitlementsPath) {
  run('/usr/bin/codesign', [
    '--force',
    '--sign',
    identity,
    '--options', 'runtime',
    identity === '-' ? '--timestamp=none' : '--timestamp',
    '--entitlements', entitlementsPath,
    path,
  ]);
}

export function verifyIdentifier(path, expectedIdentifier, expectedTeamId) {
  const result = spawnSync('/usr/bin/codesign', ['--display', '--verbose=4', path], { encoding: 'utf8' });
  const combined = `${result.stdout || ''}${result.stderr || ''}`;
  if (!combined.includes(`Identifier=${expectedIdentifier}`)) {
    throw new Error(`unexpected identifier for ${path}: expected ${expectedIdentifier}`);
  }
  if (!combined.includes(`TeamIdentifier=${expectedTeamId}`)) {
    throw new Error(`unexpected TeamIdentifier for ${path}: expected ${expectedTeamId}`);
  }
}

export function ensureBrokerResourceLayout(root) {
  const files = [
    join(root, 'broker-manifest.json'),
    join(root, 'LingXiCredentialBroker.launchd.plist'),
    join(root, 'bin', 'lingxi-credential-client'),
    join(root, 'LingXiCredentialBroker.app'),
  ];
  for (const file of files) {
    if (!existsSync(file)) throw new Error(`credential broker resource is missing: ${file}`);
  }
}
