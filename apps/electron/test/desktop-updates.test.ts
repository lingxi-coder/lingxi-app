import assert from 'node:assert/strict';
import { test } from 'node:test';
import { compareVersions, DesktopUpdateService, isHttpsUpdateUrl, selectDesktopUpdate, UPDATE_CHECK_INTERVAL_MS, UPDATE_REMINDER_DELAY_MS, type UpdateSnooze } from '../src/main/desktopUpdateService.js';
import { translate, DESKTOP_LOCALES } from '../src/renderer/i18n/locale.js';
import { updateCatalog } from '../src/renderer/i18n/updateCatalog.js';
import type { DesktopUpdateState } from '../src/shared/desktopUpdates.js';

const release = (version = '0.2.0', platform = 'darwin', arch = 'arm64', downloadUrl = 'https://lingxios.net/packages/darwin-arm64.zip') => ({ version, platform, arch, downloadUrl });
const manifest = (...releases: ReturnType<typeof release>[]) => ({ schemaVersion: 1, releases });

test('SemVer precedence handles stable, prerelease, multi-digit and metadata versions', () => {
  for (const [a, b, result] of [
    ['0.10.0', '0.9.9', 1], ['v1.0.0', '1.0.0+build.4', 0], ['1.0.0', '1.0.0-rc.1', 1],
    ['1.0.0-rc.10', '1.0.0-rc.2', 1], ['1.0.0-alpha', '1.0.0-alpha.1', -1],
    ['1.0.0-1', '1.0.0-alpha', -1], ['1.0.0-rc.1', '1.0.0', -1],
    ['1.0.0-alpha', '1.0.0-beta', -1], ['1.0.0-beta.2', '1.0.0-beta.2', 0],
  ] as const) assert.equal(compareVersions(a, b), result, `${a} / ${b}`);
  for (const invalid of ['1.0', '01.0.0', '1.0.0-01', 'garbage', '9007199254740992.0.0']) {
    assert.equal(compareVersions(invalid, '0.1.0'), null);
  }
});

test('only the newest stable package for the installed platform and architecture is advertised', () => {
  const chosen = selectDesktopUpdate(manifest(
    release('9.0.0', 'win32', 'x64'), release('8.0.0', 'darwin', 'x64'), release('7.0.0-beta.1'),
    release('0.2.0'), release('0.10.0'), release('0.3.0'), release('0.1.0'),
  ), '0.1.0', 'darwin', 'arm64');
  assert.deepEqual(chosen, { version: '0.10.0', downloadUrl: release().downloadUrl });
  assert.equal(selectDesktopUpdate(manifest(release('0.1.0'), release('0.0.9')), '0.1.0', 'darwin', 'arm64'), null);
  assert.equal(selectDesktopUpdate(manifest(release()), '0.1.0', 'linux', 'x64'), null);
  assert.equal(selectDesktopUpdate(manifest(), '0.1.0', 'darwin', 'arm64'), null);
  assert.throws(() => selectDesktopUpdate({ schemaVersion: 2, releases: [] }, '0.1.0', 'darwin', 'arm64'));
});

test('invalid versions, credential URLs and non-HTTPS download schemes are ignored', () => {
  for (const url of ['http://example.com/a.zip', 'file:///tmp/a', 'javascript:alert(1)', 'https://name:secret@example.com', 'invalid']) {
    assert.equal(isHttpsUpdateUrl(url), false);
    assert.equal(selectDesktopUpdate(manifest(release('2.0.0', 'darwin', 'arm64', url)), '0.1.0', 'darwin', 'arm64'), null);
  }
  assert.equal(selectDesktopUpdate(manifest(release('tomorrow')), '0.1.0', 'darwin', 'arm64'), null);
});

function fixture(overrides: { fetchManifest?: () => Promise<unknown>; saved?: unknown; saveSnooze?: (value: UpdateSnooze) => void } = {}) {
  let time = Date.UTC(2026, 9, 28);
  let calls = 0;
  let saved = overrides.saved;
  const changes: DesktopUpdateState[] = []; const errors: unknown[] = []; const opened: string[] = [];
  const service = new DesktopUpdateService({
    currentVersion: '0.1.0', platform: 'darwin', arch: 'arm64', now: () => time,
    fetchManifest: async () => { calls++; return overrides.fetchManifest ? overrides.fetchManifest() : manifest(release()); },
    openExternal: async url => { opened.push(url); },
    readSnooze: () => saved,
    saveSnooze: overrides.saveSnooze ?? (value => { saved = value; }),
    onChanged: state => changes.push(state), onError: error => errors.push(error),
  });
  return { service, changes, errors, opened, calls: () => calls, saved: () => saved, advance: (ms: number) => { time += ms; } };
}

test('checks deduplicate requests and rate-limit periodic/resume checks', async t => {
  let complete!: (value: unknown) => void;
  const f = fixture({ fetchManifest: () => new Promise(resolve => { complete = resolve; }) });
  t.after(() => f.service.dispose());
  const check = f.service.check();
  assert.equal(check, f.service.check());
  assert.equal(f.calls(), 1);
  complete(manifest(release())); await check;
  assert.equal(f.service.getState().release?.version, '0.2.0');
  await f.service.check(); assert.equal(f.calls(), 1);
  f.advance(UPDATE_CHECK_INTERVAL_MS);
  const next = f.service.check(); complete(manifest()); await next;
  assert.equal(f.calls(), 2); assert.equal(f.service.getState().release, null);
});

test('background failures preserve an available update, and downloads use the verified main-process URL', async t => {
  let fail = false;
  const f = fixture({ fetchManifest: async () => { if (fail) throw new Error('offline'); return manifest(release()); } });
  t.after(() => f.service.dispose());
  await assert.rejects(f.service.download(), /No Desktop update/);
  await f.service.check();
  fail = true; f.advance(UPDATE_CHECK_INTERVAL_MS); await f.service.check();
  assert.equal(f.errors.length, 1); assert.equal(f.service.getState().release?.version, '0.2.0');
  await f.service.download(); assert.deepEqual(f.opened, [release().downloadUrl]);
});

test('snooze persists for 24 hours, survives restart and only hides the selected version', async t => {
  let latest = '0.2.0';
  const f = fixture({ fetchManifest: async () => manifest(release(latest)) });
  t.after(() => f.service.dispose());
  await f.service.check(); f.service.remindLater();
  assert.equal(f.service.getState().release, null);
  const restored = fixture({ saved: f.saved() }); t.after(() => restored.service.dispose());
  await restored.service.check(); assert.equal(restored.service.getState().release, null);
  restored.advance(UPDATE_REMINDER_DELAY_MS); assert.equal(restored.service.getState().release?.version, '0.2.0');
  latest = '0.3.0'; f.advance(UPDATE_CHECK_INTERVAL_MS); await f.service.check();
  assert.equal(f.service.getState().release?.version, '0.3.0');
});

test('a persistence failure keeps the reminder visible; disposing ignores an in-flight response', async t => {
  const f = fixture({ saveSnooze: () => { throw new Error('disk unavailable'); } });
  t.after(() => f.service.dispose());
  await f.service.check(); assert.throws(() => f.service.remindLater());
  assert.equal(f.service.getState().release?.version, '0.2.0');
  let complete!: (value: unknown) => void;
  const pending = fixture({ fetchManifest: () => new Promise(resolve => { complete = resolve; }) });
  const check = pending.service.check(); pending.service.dispose();
  complete(manifest(release())); await check;
  assert.equal(pending.changes.length, 0);
});

test('startup and periodic timers stop on disposal', async t => {
  t.mock.timers.enable({ apis: ['setTimeout', 'setInterval', 'Date'], now: Date.UTC(2026, 9, 28) });
  const f = fixture();
  f.service.start(); f.service.start();
  t.mock.timers.tick(19_999); assert.equal(f.calls(), 0);
  t.mock.timers.tick(1); await f.service.check(); assert.equal(f.calls(), 1);
  f.advance(UPDATE_CHECK_INTERVAL_MS); t.mock.timers.tick(UPDATE_CHECK_INTERVAL_MS); await f.service.check();
  assert.equal(f.calls(), 2);
  f.service.dispose(); f.advance(UPDATE_CHECK_INTERVAL_MS); t.mock.timers.tick(UPDATE_CHECK_INTERVAL_MS);
  assert.equal(f.calls(), 2);
});

test('update reminder copy is translated for every Desktop locale', () => {
  for (const key of Object.keys(updateCatalog)) for (const locale of DESKTOP_LOCALES) {
    assert.notEqual(translate(key, locale, '0.2.0'), key);
    assert.ok(updateCatalog[key][locale]);
  }
});
