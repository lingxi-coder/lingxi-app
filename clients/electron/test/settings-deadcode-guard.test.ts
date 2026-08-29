import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { join } from 'node:path';

const root = join(import.meta.dirname, '../../..');

function hits(needle: string): string[] {
  try {
    return execFileSync('git', ['grep', '-l', '-F', needle, '--', 'clients/electron/src'],
      { cwd: root, encoding: 'utf8' }).trim().split('\n').filter(Boolean);
  } catch {
    return [];   // git grep 无命中时退出码为 1
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
  assert.deepEqual(hits('SettingsPage'), []);
});

test('BetaSettings is gone', () => {
  assert.deepEqual(hits('BetaSettings'), []);
});
