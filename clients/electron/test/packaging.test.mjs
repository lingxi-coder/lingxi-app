import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, rmSync, statSync, utimesSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

import {
  DEFAULT_SECRET_CANARY,
  FIXED_MTIME_SECONDS,
  artifactPaths,
  assertArm64Architecture,
  copyProductionDependencies,
  normalizeTimestamp,
  rewriteInfoPlist,
  runtimePackageJson,
  scanTreeForForbiddenContent,
  validateZipEntries,
} from '../scripts/package-support.mjs';

/**
 * A minimal but structurally real Info.plist — the same shape Electron's own
 * template has (an executable name, a bundle identifier, and a permissive
 * usage-description key that `rewriteInfoPlist` is expected to strip). Used
 * instead of copying the actual `node_modules/electron` template so this
 * test does not depend on Electron having been installed, while still
 * exercising the REAL `rewriteInfoPlist` export through the REAL `plutil`
 * binary — the same tool `package-mac.mjs` shells out to when it packages
 * the app for real, and the same tool macOS itself uses to read the bundle.
 */
const SEED_INFO_PLIST = `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleExecutable</key>
	<string>Electron</string>
	<key>CFBundleIdentifier</key>
	<string>com.github.Electron</string>
	<key>NSCameraUsageDescription</key>
	<string>placeholder camera copy from the Electron template</string>
</dict>
</plist>
`;

test('single-path timestamp normalization does not retouch signed descendants', () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-timestamp-test-'));
  try {
    const child = join(root, 'signed-resource');
    writeFileSync(child, 'signed payload');
    const childTime = new Date((FIXED_MTIME_SECONDS + 60) * 1000);
    utimesSync(child, childTime, childTime);

    normalizeTimestamp(root);

    assert.equal(Math.floor(statSync(root).mtimeMs / 1000), FIXED_MTIME_SECONDS);
    assert.equal(Math.floor(statSync(child).mtimeMs / 1000), FIXED_MTIME_SECONDS + 60);
  } finally {
    rmSync(root, { force: true, recursive: true });
  }
});

test('artifact names are versioned and architecture-specific', () => {
  const paths = artifactPaths('/tmp/desktop', { version: '1.2.3' });
  assert.equal(paths.appPath, '/tmp/desktop/dist/LingXi-Code-1.2.3-mac-arm64/LingXi Code.app');
  assert.equal(paths.zipPath, '/tmp/desktop/dist/LingXi-Code-1.2.3-mac-arm64.zip');
  assert.equal(paths.checksumPath, '/tmp/desktop/dist/LingXi-Code-1.2.3-mac-arm64.zip.sha256');
});

test('runtime dependency copy expands package file globs by copying installed payload', () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-dependency-test-'));
  try {
    const packageRoot = join(root, 'node_modules', 'fixture');
    mkdirSync(join(packageRoot, 'lib'), { recursive: true });
    writeFileSync(join(packageRoot, 'package.json'), JSON.stringify({
      name: 'fixture',
      version: '1.0.0',
      files: ['lib/*.js'],
    }));
    writeFileSync(join(packageRoot, 'lib', 'runtime.js'), 'export const ready = true;');

    const destination = join(root, 'output', 'node_modules');
    copyProductionDependencies({ fixture: '1.0.0' }, root, destination);

    assert.equal(existsSync(join(destination, 'fixture', 'lib', 'runtime.js')), true);
  } finally {
    rmSync(root, { force: true, recursive: true });
  }
});

test('runtime manifests discard development-only install metadata', () => {
  const runtime = runtimePackageJson({
    name: 'fixture',
    version: '1.0.0',
    main: 'index.js',
    files: ['dist'],
    scripts: { test: 'false' },
    dependencies: { ws: '1.0.0' },
    devDependencies: { typescript: '1.0.0' },
  });
  assert.deepEqual(runtime, {
    name: 'fixture',
    version: '1.0.0',
    main: 'index.js',
    dependencies: { ws: '1.0.0' },
  });
});

test('architecture validation accepts arm64 and rejects x86-only binaries', () => {
  assert.doesNotThrow(() => assertArm64Architecture(['arm64'], 'fixture'));
  assert.doesNotThrow(() => assertArm64Architecture(['x86_64', 'arm64'], 'fixture'));
  assert.throws(
    () => assertArm64Architecture(['x86_64'], 'fixture'),
    /arm64 is required/,
  );
});

test('ZIP validation rejects traversal and entries outside the app', () => {
  assert.doesNotThrow(() => validateZipEntries([
    'LingXi Code.app',
    'LingXi Code.app/Contents/Info.plist',
  ]));
  assert.throws(() => validateZipEntries(['../payload']), /unsafe ZIP entry/);
  assert.throws(() => validateZipEntries(['unrelated.txt']), /outside LingXi Code.app/);
});

test('package scanning rejects developer paths and an obvious secret canary', () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-package-test-'));
  try {
    writeFileSync(join(root, 'safe.txt'), 'portable fixture');
    assert.doesNotThrow(() => scanTreeForForbiddenContent(root, [
      { label: 'secret canary', pattern: new RegExp(DEFAULT_SECRET_CANARY, 'g') },
    ]));
    writeFileSync(join(root, 'unsafe.txt'), DEFAULT_SECRET_CANARY);
    assert.throws(
      () => scanTreeForForbiddenContent(root, [
        { label: 'secret canary', pattern: new RegExp(DEFAULT_SECRET_CANARY, 'g') },
      ]),
      /secret canary found/,
    );
    writeFileSync(join(root, 'unsafe.txt'), '/Users/example/Projects/private/source.ts');
    assert.throws(
      () => scanTreeForForbiddenContent(root),
      /absolute macOS user path found/,
    );
    writeFileSync(join(root, 'unsafe.txt'), '/Users/example/.cargo/registry/src/private.rs');
    assert.throws(
      () => scanTreeForForbiddenContent(root),
      /absolute macOS user path found/,
    );
  } finally {
    rmSync(root, { force: true, recursive: true });
  }
});

test('the rewritten Info.plist declares a concrete, honest microphone usage string', () => {
  // This does not read back a constant the implementation just wrote: it
  // runs the REAL `rewriteInfoPlist` export against a REAL plist file
  // through the REAL `plutil` binary (the same tool `package-mac.mjs` shells
  // out to when it packages the app, and the same format macOS itself reads
  // at install time), then reads the RESULT back the same way macOS would.
  // A typo, an un-set key, or an XML-unsafe character would surface here as
  // a `plutil` failure or a missing/garbled value, not as a passing test
  // that only inspects source text.
  const root = mkdtempSync(join(tmpdir(), 'lingxi-plist-test-'));
  try {
    const plistPath = join(root, 'Info.plist');
    writeFileSync(plistPath, SEED_INFO_PLIST);

    rewriteInfoPlist(plistPath, '9.9.9');

    const rewritten = execFileSync('/usr/bin/plutil', ['-convert', 'json', '-o', '-', plistPath], { encoding: 'utf8' });
    const plist = JSON.parse(rewritten);

    const description = plist.NSMicrophoneUsageDescription;
    assert.equal(typeof description, 'string', 'macOS kills the app on first getUserMedia without this key');
    assert.ok(
      description.length > 10,
      'the string is shown to the user verbatim in the permission dialog and must actually explain the use',
    );
    assert.equal(
      description,
      'LingXi Code only uses the microphone to record a short audio clip when you choose to start recording, '
      + 'for example by pressing the microphone button in the composer. It never listens in the background, '
      + 'and this build does not transcribe the recording into text, whether on this device or on a server.',
    );
    // Desktop speech recognition is unconditionally unavailable in this build
    // (`renderer/audio/requests.ts`'s `DESKTOP_TRANSCRIPTION_UNAVAILABLE_MESSAGE`
    // answers every `transcribe` op with `failed`/`unavailable`, regardless of
    // permission or provider state — see `Voice.tsx`'s
    // `RECOGNITION_UNAVAILABLE_NOTICE`). A usage string that implied
    // dictation/transcription would mislead the user reading the system
    // permission dialog.
    //
    // The negation must sit NEXT TO the transcription word, not merely in the
    // same sentence. A same-sentence rule passed this false string:
    //   "It never listens in the background, and this build automatically
    //    transcribes the recording into text."
    // because `never` in an unrelated clause satisfied it. A blanket
    // "never mention transcription" ban is also wrong — the honest thing to
    // write is an explicit DENIAL, which such a ban would reject.
    const transcriptionWord = /transcri|dictat|speech.to.text|recogni[sz]e/gi;
    const negatesIt = /\b(does not|doesn't|do not|don't|never|cannot|can't|no|not)\b/i;
    const NEGATION_WINDOW = 30;
    let match;
    let checked = 0;
    while ((match = transcriptionWord.exec(description)) !== null) {
      checked += 1;
      const before = description.slice(Math.max(0, match.index - NEGATION_WINDOW), match.index);
      assert.ok(
        negatesIt.test(before),
        `"${match[0]}" is not denied within ${NEGATION_WINDOW} characters before it: "...${before}${match[0]}..."`,
      );
    }
    assert.ok(checked > 0, 'the string must address transcription explicitly, not by omission');

    // Sanity that this exercised the real rewrite, not a stub: other keys
    // landed, the version was substituted, and the permissive template key
    // that ships with Electron was actually removed.
    assert.equal(plist.CFBundleIdentifier, 'com.lingxi.code');
    assert.equal(plist.CFBundleShortVersionString, '9.9.9');
    assert.equal('NSCameraUsageDescription' in plist, false);
  } finally {
    rmSync(root, { force: true, recursive: true });
  }
});
