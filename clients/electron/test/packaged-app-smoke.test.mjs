import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, test } from 'node:test';

import {
  APP_USER_DATA_SUBPATH,
  createPackagedSettings,
  runtimePathsForHome,
  sanitizePackagedAppEnvironment,
  workspaceTrustFingerprint,
} from '../scripts/packaged-app-smoke.mjs';
import { workspaceFingerprint as hostWorkspaceFingerprint } from '../src/main/host-utils.ts';

const temporaryDirectories = [];

function temporaryDirectory() {
  const path = mkdtempSync(join(tmpdir(), 'lingxi-packaged-smoke-test-'));
  temporaryDirectories.push(path);
  return path;
}

afterEach(() => {
  for (const path of temporaryDirectories.splice(0)) {
    rmSync(path, { recursive: true, force: true });
  }
});

test('launch environment strips development renderer, sidecar, and credential overrides', () => {
  const env = sanitizePackagedAppEnvironment({
    HOME: '/Users/tester',
    PATH: '/usr/bin:/bin',
    TMPDIR: '/tmp/source',
    ELECTRON_RENDERER_URL: 'http://127.0.0.1:5173',
    LINGXI_BRIDGE_SERVER_BIN: '/tmp/dev-bridge-server',
    LINGXI_API_BASE_URL: 'https://api.example.test',
    ANTHROPIC_API_KEY: 'secret',
    OPENAI_API_KEY: 'secret-2',
    RANDOM_KEEP: 'value',
  }, {
    HOME: '/tmp/isolated-home',
    TMPDIR: '/tmp/isolated-tmp',
    TEST_ONLY: '1',
  });

  assert.deepEqual(env, {
    HOME: '/tmp/isolated-home',
    PATH: '/usr/bin:/bin',
    RANDOM_KEEP: 'value',
    TEST_ONLY: '1',
    TMPDIR: '/tmp/isolated-tmp',
  });
});

test('runtime paths stay inside the isolated HOME tree', () => {
  const home = '/tmp/isolated-home';
  const paths = runtimePathsForHome(home);
  assert.equal(paths.homeDir, home);
  assert.equal(paths.userDataDir, join(home, APP_USER_DATA_SUBPATH));
  assert.equal(paths.bridgeRuntimeDir, join(home, APP_USER_DATA_SUBPATH, 'bridge-runtime'));
  assert.equal(paths.settingsPath, join(home, APP_USER_DATA_SUBPATH, 'settings.v1.json'));
  assert.equal(paths.diagnosticsPath, join(home, APP_USER_DATA_SUBPATH, 'logs', 'desktop.jsonl'));
});

test('packaged settings preseed a trusted recent workspace without secrets', () => {
  const workspace = temporaryDirectory();
  const settings = createPackagedSettings({ workspace, theme: 'dark', now: new Date('2026-07-21T00:00:00Z') });
  const canonical = realpathSync.native(workspace);

  assert.equal(settings.version, 1);
  assert.equal(settings.theme, 'dark');
  assert.equal(settings.lastWorkspace, canonical);
  assert.deepEqual(settings.recentWorkspaces, [canonical]);
  assert.deepEqual(Object.keys(settings.trustedWorkspaces), [canonical]);
  assert.equal(settings.trustedWorkspaces[canonical]?.fingerprint, workspaceTrustFingerprint(workspace));
  assert.equal(settings.apiBaseUrl, undefined);
});

test('workspace trust fingerprint changes when executable config changes', () => {
  const workspace = temporaryDirectory();
  mkdirSync(join(workspace, '.claude'), { recursive: true });
  const before = workspaceTrustFingerprint(workspace);
  writeFileSync(join(workspace, '.claude', 'settings.json'), '{"mcpServers":{"demo":{"command":"echo"}}}');
  const after = workspaceTrustFingerprint(workspace);
  assert.notEqual(after, before);
  assert.equal(after, hostWorkspaceFingerprint(workspace));
});
