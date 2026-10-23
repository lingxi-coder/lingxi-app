import assert from 'node:assert/strict';
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

import { parseSettings } from '../src/main/host-utils.js';
import { SettingsStore } from '../src/main/settings.js';

function withDirectory(run: (directory: string) => void): void {
  const directory = mkdtempSync(join(tmpdir(), 'lingxi-remote-settings-'));
  try {
    run(directory);
  } finally {
    chmodSync(directory, 0o700);
    rmSync(directory, { recursive: true, force: true });
  }
}

test('Remote Control is off by default, and only a real boolean is read from the file', () => {
  assert.equal(parseSettings({ version: 1 }).remoteControlEnabled, undefined);
  assert.equal(parseSettings({ version: 1, remoteControlEnabled: true }).remoteControlEnabled, true);
  assert.equal(parseSettings({ version: 1, remoteControlEnabled: false }).remoteControlEnabled, false);
  for (const value of ['true', 1, null, {}, []]) {
    assert.equal(parseSettings({ version: 1, remoteControlEnabled: value }).remoteControlEnabled, undefined, JSON.stringify(value));
  }
});

test('the setting is saved, survives a restart, and is not part of the settings the window sees', () => {
  withDirectory((directory) => {
    const store = new SettingsStore(directory);
    assert.equal(store.getRemoteControlEnabled(), false);
    store.setRemoteControlEnabled(true);
    assert.equal(store.getRemoteControlEnabled(), true);
    assert.equal(JSON.parse(readFileSync(join(directory, 'settings.v1.json'), 'utf8')).remoteControlEnabled, true);
    assert.equal(new SettingsStore(directory).getRemoteControlEnabled(), true);
    assert.ok(!('remoteControlEnabled' in store.getPublic()));

    store.setRemoteControlEnabled(false);
    assert.equal(new SettingsStore(directory).getRemoteControlEnabled(), false);
  });
});

test('a value that is not a boolean is refused', () => {
  withDirectory((directory) => {
    const store = new SettingsStore(directory);
    assert.throws(() => store.setRemoteControlEnabled('yes' as unknown as boolean), /invalid/);
    assert.equal(store.getRemoteControlEnabled(), false);
  });
});

test('when the file cannot be written the setting stays as it was and the caller is told', () => {
  withDirectory((directory) => {
    const store = new SettingsStore(directory);
    store.setRemoteControlEnabled(true);
    chmodSync(directory, 0o500);
    try {
      assert.throws(() => store.setRemoteControlEnabled(false));
      assert.equal(store.getRemoteControlEnabled(), true);
    } finally {
      chmodSync(directory, 0o700);
    }
  });
});

test('other settings keep their place when this one is saved', () => {
  withDirectory((directory) => {
    const store = new SettingsStore(directory);
    store.setLastFastMode(true);
    store.setRemoteControlEnabled(true);
    const reread = new SettingsStore(directory);
    assert.equal(reread.getLastFastMode(), true);
    assert.equal(reread.getRemoteControlEnabled(), true);
    writeFileSync(join(directory, 'unrelated'), '');
  });
});
