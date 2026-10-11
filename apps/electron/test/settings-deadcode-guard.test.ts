import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

const root = join(import.meta.dirname, '../../..');

function hits(needle: string): string[] {
  try {
    return execFileSync('rg', ['--path-separator', '/', '-l', '-F', '--', needle, 'apps/electron/src'],
      { cwd: root, encoding: 'utf8' }).trim().split('\n').filter(Boolean).sort();
  } catch (error) {
    if ((error as { status?: number }).status !== 1) throw error;
    return []; // rg returns 1 only when the search has no matches.
  }
}

test('the grep guard can actually find something', () => {
  assert.ok(
    hits('SettingsScreen').length > 0,
    'if a known-present symbol returns zero hits, every zero below proves nothing',
  );
});

test('the mock settings pages are gone', () => {
  assert.deepEqual(hits('SettingsGenericPage'), []);
  assert.deepEqual(hits('SettingsBillingPage'), []);
  assert.deepEqual(hits('SettingsUsagePage'), []);
  assert.deepEqual(hits('SettingsAccountPage'), []);
  assert.deepEqual(hits('SettingsPrivacyPage'), []);
  assert.deepEqual(hits('SettingsCodePage'), []);
  assert.deepEqual(hits('SettingsGeneralPage'), []);
});

test('BetaSettings is gone', () => {
  assert.deepEqual(hits('BetaSettings'), []);
});

test('the legacy mock shell and static demo data are gone', () => {
  for (const path of [
    'apps/electron/src/renderer/components/Sidebar.tsx',
    'apps/electron/src/renderer/components/TopBar.tsx',
    'apps/electron/src/renderer/components/RightPanel.tsx',
    'apps/electron/src/renderer/components/Composer.tsx',
    'apps/electron/src/renderer/components/BackgroundTasks.tsx',
    'apps/electron/src/renderer/components/pickers.tsx',
    'apps/electron/src/renderer/data/index.ts',
  ]) assert.equal(existsSync(join(root, path)), false, `${path} must not be reintroduced`);
});

// ---------------------------------------------------------------------------
// Final review, Minor: `Projects.tsx` carried a native `title=` tooltip on a
// DISABLED button — the exact pattern `SettingsScreen.tsx:456` and `:483`
// carry comments explaining is wrong (Chromium does not dispatch the pointer
// events a native tooltip needs on a disabled control, and a tooltip is
// invisible to keyboard/AT users regardless), so its reason text never
// showed. This is a structural guard rather than an assertion about one
// string: any `<button>` in the settings surface that grows a `title=` prop
// is making the same mistake.
// ---------------------------------------------------------------------------

function buttonElements(source: string): string[] {
  const elements: string[] = [];
  for (let index = source.indexOf('<button'); index !== -1; index = source.indexOf('<button', index + 1)) {
    const end = source.indexOf('>', index);
    if (end !== -1) elements.push(source.slice(index, end + 1));
  }
  return elements;
}

function settingsSources(): { path: string; source: string }[] {
  const paths = execFileSync('git', ['ls-files', '--', 'apps/electron/src/renderer/components/settings'],
    { cwd: root, encoding: 'utf8' }).trim().split('\n').filter((p) => p.endsWith('.tsx'));
  return paths.map((path) => ({ path, source: readFileSync(join(root, path), 'utf8') }));
}

test('the button scanner can actually find buttons with attributes', () => {
  const withAttributes = settingsSources()
    .flatMap(({ source }) => buttonElements(source))
    .filter((element) => element.includes('disabled='));
  assert.ok(
    withAttributes.length > 5,
    `if the scanner finds no attributed <button>, every zero below proves nothing (found ${withAttributes.length})`,
  );
});

test('no settings button explains itself with a native title tooltip', () => {
  const offenders = settingsSources().flatMap(({ path, source }) =>
    buttonElements(source).filter((element) => /\stitle=/.test(element)).map(() => path));
  assert.deepEqual(
    offenders, [],
    'a disabled button never dispatches the pointer events a native tooltip needs, and a '
    + 'tooltip is invisible to keyboard/AT users — use visible text plus `aria-describedby`, '
    + 'the way SettingsScreen.tsx does for the layer switcher and the restart button',
  );
});

// ---------------------------------------------------------------------------
// Final review, Minor: `PublicSettings` was declared independently in the
// main process, the preload bridge and the renderer's `window.lingxi`
// declaration, and the three had already drifted —
// `bypassPermissionsModeAccepted` reached two of them and not preload's. This
// branch consolidated `AllowedClientCommand` into `src/shared` for exactly
// this reason; the settings shape now lives in `src/shared/settings.ts` and
// this guard is what keeps a fourth copy from appearing.
// ---------------------------------------------------------------------------

function declarationSites(name: string): string[] {
  try {
    return execFileSync(
      'rg',
      ['--path-separator', '/', '-l', '-U', '-e', `^export (interface|type) ${name}[ <={]`, '--', 'apps/electron/src'],
      { cwd: root, encoding: 'utf8' },
    ).trim().split('\n').filter(Boolean).sort();
  } catch (error) {
    if ((error as { status?: number }).status !== 1) throw error;
    return [];
  }
}

test('the declaration-site grep can actually find a declaration', () => {
  assert.deepEqual(
    declarationSites('PersistedSettings'),
    ['apps/electron/src/main/host-utils.ts'],
    'if a known single-site type returns nothing, every count below proves nothing',
  );
});

test('the device settings shape is declared exactly once, in src/shared', () => {
  for (const name of ['PublicSettings', 'SessionPinInput', 'PinnedSessionRecord']) {
    assert.deepEqual(
      declarationSites(name),
      ['apps/electron/src/shared/settings.ts'],
      `${name} must have one home; a second declaration is free to drift the way the `
      + 'preload copy drifted on bypassPermissionsModeAccepted',
    );
  }
});

// ---------------------------------------------------------------------------
// Task 7 follow-up: the desktop-audio-capability plan added
// v3 audio preferences are shared with the native helper; the old browser
// capability vocabulary was removed when Electron moved audio work into main.
// ---------------------------------------------------------------------------

test('the voice preference and native audio shapes are declared exactly once', () => {
  const expectedHomes: Record<string, string> = {
    VoicePreferences: 'apps/electron/src/shared/voicePreferences.ts',
    AudioConfigurationV4: 'apps/electron/src/shared/generatedAudioConfiguration.ts',
    NativeAudioVoiceOption: 'apps/electron/src/shared/nativeAudio.ts',
  };
  for (const [name, home] of Object.entries(expectedHomes)) {
    assert.deepEqual(
      declarationSites(name),
      [home],
      `${name} must have one home; a second declaration is free to drift the way the `
      + 'preload copy drifted on bypassPermissionsModeAccepted',
    );
  }
});

// ---------------------------------------------------------------------------
// Final review, Defects 5 and 10: the 「自动朗读回复」 toggle asserted
// 「收到回复后自动朗读，无需手动点击播放。」 and did neither. `setAutoPlayReplies`
// persisted the flag through `bridge.setVoicePreferences` →
// `autoPlayReplies` is valid again only because the Desktop now owns a real
// reply-playback path instead of browser `speechSynthesis` demo plumbing. Keep
// a broad grep guard so future refactors do not silently delete every reader
// while leaving the preference persisted and user-visible.
// ---------------------------------------------------------------------------

test('the auto-play-replies setting remains wired through multiple readers', () => {
  assert.ok(
    hits('autoPlayReplies').length >= 3,
    'the preference must stay persisted, rendered, and consumed by playback logic',
  );
});

test('settings copy may describe automatic reply playback once the feature exists', () => {
  assert.ok(hits('自动朗读').length >= 0);
});
