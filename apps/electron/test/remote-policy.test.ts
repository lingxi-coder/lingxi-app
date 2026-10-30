import assert from 'node:assert/strict';
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { productNamespace } from '../src/main/productNamespace.js';

import { MAX_REMOTE_SESSIONS, parseSettings } from '../src/main/host-utils.js';
import { RemotePolicy, managedSettingsDirectory } from '../src/main/remoteControl/policy.js';
import { SettingsStore } from '../src/main/settings.js';
import { FakeSettings, PROJECT, SESSION_A, SESSION_B } from './remote-session-helpers.js';

/** A settings store that knows the projects the tests use. */
function storeIn(directory: string): SettingsStore {
  const store = new SettingsStore(directory);
  for (const project of [PROJECT, '/other']) store.addProject(project);
  return store;
}

function withDirectory(run: (directory: string) => void): void {
  const directory = mkdtempSync(join(tmpdir(), 'lingxi-remote-policy-'));
  try {
    run(directory);
  } finally {
    chmodSync(directory, 0o700);
    rmSync(directory, { recursive: true, force: true });
  }
}

const ref = (sessionId: string, projectPath = PROJECT) => ({ projectPath, sessionId });

// ─────────────────────────────────────────────────────────────────────────────
// The settings file
// ─────────────────────────────────────────────────────────────────────────────

test('the per-session switches, remoteControlAtStartup and the always-allow setting are read defensively', () => {
  const parsed = parseSettings({
    version: 1,
    remoteSessions: [
      { projectPath: '/a', sessionId: SESSION_A },
      { projectPath: '/a', sessionId: SESSION_A },
      { projectPath: '/b', sessionId: 'not-a-session-id' },
      { projectPath: '', sessionId: SESSION_B },
      'junk',
      null,
      { projectPath: '/c', sessionId: SESSION_B },
    ],
    remoteControlAtStartup: true,
    remoteAllowAlwaysAllow: 'yes',
  });
  assert.deepEqual(parsed.remoteSessions, [{ projectPath: '/a', sessionId: SESSION_A }, { projectPath: '/c', sessionId: SESSION_B }]);
  assert.equal(parsed.remoteControlAtStartup, true);
  assert.equal(parsed.remoteAllowAlwaysAllow, undefined);
  assert.equal(parseSettings({ version: 1 }).remoteSessions, undefined);
  const many = Array.from({ length: MAX_REMOTE_SESSIONS + 50 }, (_, i) => ({ projectPath: '/p', sessionId: `12121212-3434-4567-8899-${String(i).padStart(12, '0')}` }));
  assert.equal(parseSettings({ version: 1, remoteSessions: many }).remoteSessions!.length, MAX_REMOTE_SESSIONS);
});

test('a session is switched on and off, survives a restart, and none of it is shown to the window', () => {
  withDirectory((directory) => {
    const store = storeIn(directory);
    assert.deepEqual(store.getRemoteSessions(), []);
    store.setRemoteSession(ref(SESSION_A), true);
    store.setRemoteSession(ref(SESSION_B, '/other'), true);
    store.setRemoteSession(ref(SESSION_A), true); // again: no duplicate
    assert.equal(store.getRemoteSessions().length, 2);
    assert.equal(store.isRemoteSession(SESSION_A), true);
    const reopened = storeIn(directory);
    assert.deepEqual(reopened.getRemoteSessions(), [ref(SESSION_B, '/other'), ref(SESSION_A)]);
    reopened.setRemoteSession(ref(SESSION_A), false);
    assert.deepEqual(new SettingsStore(directory).getRemoteSessions(), [ref(SESSION_B, '/other')]);
    const publicSettings = JSON.stringify(store.getPublic());
    assert.ok(!publicSettings.includes('remoteSessions'));
    assert.ok(!publicSettings.includes('remoteControlAtStartup'));
    assert.ok(!publicSettings.includes('remoteAllowAlwaysAllow'));
  });
});

test('a bad session reference or value is refused, and the cap holds', () => {
  withDirectory((directory) => {
    const store = storeIn(directory);
    assert.throws(() => store.setRemoteSession({ projectPath: PROJECT, sessionId: '../x' }, true));
    assert.throws(() => store.setRemoteSession(ref(SESSION_A), 'yes' as never), /invalid/);
    assert.deepEqual(store.getRemoteSessions(), []);
    for (let i = 0; i < MAX_REMOTE_SESSIONS; i += 1) store.setRemoteSession(ref(`12121212-3434-4567-8899-${String(i).padStart(12, '0')}`), true);
    assert.throws(() => store.setRemoteSession(ref(SESSION_A), true), /too many/);
    store.setRemoteSession(ref('12121212-3434-4567-8899-000000000000'), false);
    store.setRemoteSession(ref(SESSION_A), true);
    assert.equal(store.getRemoteSessions().length, MAX_REMOTE_SESSIONS);
  });
});

test('when the file cannot be written the switch stays as it was and the caller is told', () => {
  withDirectory((directory) => {
    const store = storeIn(directory);
    store.setRemoteSession(ref(SESSION_A), true);
    chmodSync(directory, 0o500);
    try {
      assert.throws(() => store.setRemoteSession(ref(SESSION_B), true));
      assert.throws(() => store.setRemoteControlAtStartup(true));
      assert.deepEqual(store.getRemoteSessions(), [ref(SESSION_A)]);
      assert.equal(store.getRemoteControlAtStartup(), false);
    } finally {
      chmodSync(directory, 0o700);
    }
  });
});

test('remoteControlAtStartup is off by default and saved; always-allow is read from the file and defaults off', () => {
  withDirectory((directory) => {
    const store = storeIn(directory);
    assert.equal(store.getRemoteControlAtStartup(), false);
    assert.equal(store.getRemoteAllowAlwaysAllow(), false);
    store.setRemoteControlAtStartup(true);
    assert.equal(new SettingsStore(directory).getRemoteControlAtStartup(), true);
    assert.throws(() => store.setRemoteControlAtStartup('x' as never), /invalid/);
    const file = join(directory, 'settings.v1.json');
    const written = JSON.parse(readFileSync(file, 'utf8'));
    writeFileSync(file, JSON.stringify({ ...written, remoteAllowAlwaysAllow: true }));
    assert.equal(new SettingsStore(directory).getRemoteAllowAlwaysAllow(), true);
  });
});

// ─────────────────────────────────────────────────────────────────────────────
// The policy
// ─────────────────────────────────────────────────────────────────────────────

test('managed settings are looked for where the engine looks for them', () => {
  assert.equal(managedSettingsDirectory({}, 'darwin'), productNamespace.managedDirMacOS);
  assert.equal(managedSettingsDirectory({}, 'win32'), productNamespace.managedDirWindows);
  assert.equal(managedSettingsDirectory({}, 'linux'), productNamespace.managedDirUnix);
  assert.equal(managedSettingsDirectory({ LINGXI_MANAGED_DIR: '/custom' }, 'linux'), '/custom');
});

function managedPolicy(files: Record<string, string | Error>, now: { t: number }) {
  return new RemotePolicy({
    settings: new FakeSettings(),
    managedDirectory: '/managed',
    readFile: (path) => {
      const content = files[path];
      if (content === undefined || content instanceof Error) throw content ?? new Error('ENOENT');
      return content;
    },
    now: () => now.t,
  });
}

test('only disableRemoteControl: true in the managed file vetoes Remote Control; anything doubtful does not', () => {
  const now = { t: 0 };
  const path = '/managed/managed-settings.json';
  const cases: Array<[string | Error | undefined, boolean]> = [
    ['{"disableRemoteControl":true}', true],
    ['{"disableRemoteControl":false}', false],
    ['{"disableRemoteControl":"true"}', false],
    ['{"other":1}', false],
    ['[true]', false],
    ['not json', false],
    ['x'.repeat(2 * 1024 * 1024), false],
    [new Error('EACCES'), false],
    [undefined, false],
  ];
  for (const [content, vetoed] of cases) {
    const policy = managedPolicy(content === undefined ? {} : { [path]: content }, now);
    assert.equal(policy.managedDisabled(), vetoed, String(content).slice(0, 40));
    assert.equal(policy.allows(), !vetoed);
  }
  assert.equal(new RemotePolicy({ settings: new FakeSettings() }).managedDisabled(), false, 'no directory, no veto');
});

test('the managed file is read again after five seconds, or at once by recheck, which tells the listeners', () => {
  const now = { t: 0 };
  const files: Record<string, string> = { '/managed/managed-settings.json': '{}' };
  const policy = managedPolicy(files, now);
  let heard = 0;
  policy.onChange(() => { heard += 1; });
  assert.equal(policy.allows(), true);
  files['/managed/managed-settings.json'] = '{"disableRemoteControl":true}';
  assert.equal(policy.allows(), true, 'cached');
  now.t = 5_000;
  assert.equal(policy.allows(), false);
  files['/managed/managed-settings.json'] = '{}';
  assert.equal(policy.recheck(), true);
  assert.equal(policy.allows(), true);
  assert.equal(heard, 1);
  assert.equal(policy.recheck(), false);
  assert.equal(heard, 1, 'no change, no news');
});

test('switching sessions on and off tells the listeners once each, and a failed save tells no one', () => {
  const settings = new FakeSettings();
  const policy = new RemotePolicy({ settings });
  let heard = 0;
  const off = policy.onChange(() => { heard += 1; });
  policy.setSessionEnabled(ref(SESSION_A), true);
  policy.setSessionEnabled(ref(SESSION_A), true);
  assert.equal(heard, 1);
  assert.equal(policy.isEnabled(SESSION_A), true);
  assert.deepEqual(policy.find(SESSION_A), ref(SESSION_A));
  settings.failWrites = true;
  assert.throws(() => policy.setSessionEnabled(ref(SESSION_B), true), /disk full/);
  assert.equal(heard, 1);
  assert.equal(policy.isEnabled(SESSION_B), false);
  settings.failWrites = false;
  policy.setSessionEnabled(ref(SESSION_A), false);
  assert.equal(heard, 2);
  assert.equal(policy.wasEnabled(SESSION_A), true, 'switched off is not unknown');
  assert.equal(policy.wasEnabled(SESSION_B), false);
  off();
  policy.setSessionEnabled(ref(SESSION_A), true);
  assert.equal(heard, 2);
});

test('archived sessions are not enabled sessions, and a listener that throws does not stop the others', () => {
  const settings = new FakeSettings();
  settings.sessions = [ref(SESSION_A), ref(SESSION_B)];
  settings.archived.add(SESSION_B);
  const policy = new RemotePolicy({ settings });
  assert.deepEqual(policy.enabledSessions(), [ref(SESSION_A)]);
  assert.equal(policy.wasEnabled(SESSION_B), true);
  const heard: string[] = [];
  policy.onChange(() => { throw new Error('bad listener'); });
  policy.onChange(() => { heard.push('ok'); });
  policy.notifySessionsChanged();
  assert.deepEqual(heard, ['ok']);
});

test('a new session is switched on at creation only when remoteControlAtStartup says so', () => {
  const settings = new FakeSettings();
  const policy = new RemotePolicy({ settings });
  policy.sessionCreated(ref(SESSION_A));
  assert.equal(policy.isEnabled(SESSION_A), false);
  policy.setAtStartup(true);
  assert.equal(policy.getAtStartup(), true);
  policy.sessionCreated(ref(SESSION_B));
  assert.equal(policy.isEnabled(SESSION_B), true);
  settings.allowAlways = true;
  assert.equal(policy.allowAlwaysAllow(), true);
});

test('the policy against a real settings file, across a restart', () => {
  withDirectory((directory) => {
    const first = new RemotePolicy({ settings: storeIn(directory) });
    first.setSessionEnabled(ref(SESSION_A), true);
    const second = new RemotePolicy({ settings: storeIn(directory) });
    assert.equal(second.isEnabled(SESSION_A), true);
    assert.equal(second.wasEnabled(SESSION_A), true);
    mkdirSync(join(directory, 'managed'));
    writeFileSync(join(directory, 'managed', 'managed-settings.json'), '{"disableRemoteControl":true}');
    const managed = new RemotePolicy({ settings: storeIn(directory), managedDirectory: join(directory, 'managed') });
    assert.equal(managed.allows(), false);
  });
});
