import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { test, type TestContext } from 'node:test';
import type { PermissionModeId } from '@lingxi/bridge-client';
import { SettingsStore } from '../src/main/settings';
import { parseSettings } from '../src/main/host-utils';

async function directory(t: TestContext) {
  const path = await mkdtemp(join(tmpdir(), 'lingxi-permission-settings-'));
  t.after(() => rm(path, { recursive: true, force: true }));
  return path;
}

test('last permission selection survives restarting and replaces the previous selection', async (t) => {
  const path = await directory(t);
  for (const mode of ['default', 'acceptEdits', 'plan', 'auto', 'dontAsk'] as const) {
    new SettingsStore(path).setLastPermissionMode(mode);
    const reopened = new SettingsStore(path);
    assert.equal(reopened.getLastPermissionMode(), mode);
    assert.equal('lastPermissionMode' in reopened.getPublic(), false);
  }
});

test('old settings retain no permission preference', async (t) => {
  const path = await directory(t);
  assert.equal(new SettingsStore(path).getLastPermissionMode(), undefined);
  await writeFile(join(path, 'settings.v1.json'), JSON.stringify({ version: 1, theme: 'dark' }));
  assert.equal(new SettingsStore(path).getLastPermissionMode(), undefined);
});

test('invalid persisted and runtime permission modes are rejected', async (t) => {
  const path = await directory(t);
  const settings = new SettingsStore(path);
  settings.setLastPermissionMode('plan');
  for (const invalid of ['unknown', 'PLAN', '', ' plan', null, 1, {}, true]) {
    assert.equal(parseSettings({ version: 1, lastPermissionMode: invalid }).lastPermissionMode, undefined);
    assert.throws(() => settings.setLastPermissionMode(invalid as PermissionModeId), /invalid permission mode/);
    assert.equal(settings.getLastPermissionMode(), 'plan');
  }
  assert.equal(new SettingsStore(path).getLastPermissionMode(), 'plan');
});

test('bypass restores only when the one-time acknowledgement is persisted', async (t) => {
  const path = await directory(t);
  const settings = new SettingsStore(path);
  settings.setLastPermissionMode('bypassPermissions');
  assert.equal(settings.getLastPermissionMode(), undefined);
  assert.equal(new SettingsStore(path).getLastPermissionMode(), undefined);
  settings.setBypassPermissionsAccepted(true);
  assert.equal(new SettingsStore(path).getLastPermissionMode(), 'bypassPermissions');
  settings.setBypassPermissionsAccepted(false);
  assert.equal(new SettingsStore(path).getLastPermissionMode(), undefined);
});

test('failed persistence rolls back the in-memory preference and retains the saved mode', async (t) => {
  const path = await directory(t);
  const settings = new SettingsStore(path);
  settings.setLastPermissionMode('acceptEdits');
  const previousFile = await readFile(settings.settingsPath, 'utf8');
  await mkdir(`${settings.settingsPath}.tmp`);
  assert.throws(() => settings.setLastPermissionMode('plan'));
  assert.equal(settings.getLastPermissionMode(), 'acceptEdits');
  assert.equal(await readFile(settings.settingsPath, 'utf8'), previousFile);
  assert.equal(new SettingsStore(path).getLastPermissionMode(), 'acceptEdits');
});

test('generic settings updates cannot change the permission preference', async (t) => {
  const settings = new SettingsStore(await directory(t));
  settings.setLastPermissionMode('plan');
  settings.update({ lastPermissionMode: 'bypassPermissions', theme: 'dark' } as Parameters<SettingsStore['update']>[0]);
  assert.equal(settings.getLastPermissionMode(), 'plan');
});
