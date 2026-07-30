import { afterEach, test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import {
  DiagnosticBuffer,
  MAX_DIAGNOSTICS,
  buildBridgeArguments,
  buildBridgeEnvironment,
  buildCredentialEnvelope,
  canonicalWorkspace,
  defaultSettings,
  parseSettings,
  sanitizeDiagnostic,
  setWorkspaceTrust,
  withRecentWorkspace,
  workspaceFingerprint,
  workspaceTrust,
} from '../src/main/host-utils';
import { SettingsStore } from '../src/main/settings';

const temporaryDirectories: string[] = [];
function temporaryDirectory(): string {
  const path = mkdtempSync(join(tmpdir(), 'lingxi-electron-test-'));
  temporaryDirectories.push(path);
  return path;
}
afterEach(() => {
  for (const path of temporaryDirectories.splice(0)) rmSync(path, { recursive: true, force: true });
});

test('settings parser fails closed to the current version and bounds recent workspaces', () => {
  assert.deepEqual(parseSettings({ version: 999, lastWorkspace: '/unsafe' }), defaultSettings());
  const parsed = parseSettings({ version: 1, recentWorkspaces: Array.from({ length: 20 }, (_, i) => `/p/${i}`) });
  assert.equal(parsed.recentWorkspaces.length, 10);
});

test('bypassPermissionsModeAccepted round-trips only for a strict true', () => {
  // Persisted acceptance survives a parse.
  assert.equal(
    parseSettings({ version: 1, bypassPermissionsModeAccepted: true }).bypassPermissionsModeAccepted,
    true,
  );
  // Absent / falsy / non-boolean never fabricates acceptance (fail-closed).
  assert.equal(parseSettings({ version: 1 }).bypassPermissionsModeAccepted, undefined);
  for (const bad of [false, 'true', 1, null]) {
    assert.equal(
      parseSettings({ version: 1, bypassPermissionsModeAccepted: bad }).bypassPermissionsModeAccepted,
      undefined,
      `${JSON.stringify(bad)} must not be read as acceptance`,
    );
  }
});

test('workspace paths are canonical and recents are unique most-recent-first', () => {
  const workspace = temporaryDirectory();
  const canonical = canonicalWorkspace(join(workspace, '.'));
  let settings = withRecentWorkspace(defaultSettings(), canonical);
  settings = withRecentWorkspace(settings, canonical);
  assert.equal(settings.lastWorkspace, canonical);
  assert.deepEqual(settings.recentWorkspaces, [canonical]);
});

test('trust is revoked when project config, agents, plugins, or memory content changes', () => {
  const workspace = temporaryDirectory();
  mkdirSync(join(workspace, '.claude'));
  mkdirSync(join(workspace, '.claude/agents'), { recursive: true });
  mkdirSync(join(workspace, '.lingxi'));
  mkdirSync(join(workspace, '.lingxi/plugins/example'), { recursive: true });
  writeFileSync(join(workspace, '.mcp.json'), '{}');
  writeFileSync(join(workspace, '.claude/settings.json'), '{}');
  writeFileSync(join(workspace, '.lingxi/settings.local.json'), '{}');
  writeFileSync(join(workspace, '.claude/agents/reviewer.md'), '# reviewer\n');
  writeFileSync(join(workspace, '.lingxi/plugins/example/plugin.json'), '{"name":"example"}\n');
  writeFileSync(join(workspace, 'LINGXI.md'), '# memory\n');
  const trusted = setWorkspaceTrust(defaultSettings(), workspace, true, new Date('2026-01-01T00:00:00Z'));
  assert.equal(workspaceTrust(trusted, workspace).trusted, true);

  const before = workspaceFingerprint(workspace);
  writeFileSync(join(workspace, '.claude/agents/reviewer.md'), '# reviewer updated\n');
  writeFileSync(join(workspace, '.lingxi/plugins/example/plugin.json'), '{"name":"example","version":"2"}\n');
  writeFileSync(join(workspace, 'LINGXI.md'), '# memory updated\n');
  assert.notEqual(workspaceFingerprint(workspace), before);
  assert.equal(workspaceTrust(trusted, workspace).trusted, false);
});

test('trust follows symlink targets and fails closed when executable config exceeds its budget', () => {
  const workspace = temporaryDirectory();
  const target = join(temporaryDirectory(), 'settings-target.json');
  mkdirSync(join(workspace, '.claude'), { recursive: true });
  writeFileSync(target, '{"hooks":{}}');
  symlinkSync(target, join(workspace, '.claude/settings.json'));
  const before = workspaceFingerprint(workspace);
  writeFileSync(target, '{"hooks":{"x":1}}');
  assert.notEqual(workspaceFingerprint(workspace), before);

  writeFileSync(target, 'x'.repeat(512 * 1024 + 1));
  assert.throws(
    () => workspaceFingerprint(workspace),
    /exceeds the safe trust fingerprint limit/,
  );
});

test('bridge environment is allowlisted and never inherits credentials', () => {
  const result = buildBridgeEnvironment({ PATH: '/bin', HOME: '/home/user', ANTHROPIC_API_KEY: 'secret', RANDOM_VAR: 'no' }, 'https://api.example.test');
  assert.deepEqual(result, { HOME: '/home/user', PATH: '/bin', LINGXI_API_BASE_URL: 'https://api.example.test' });
});

test('bridge arguments carry trust and stdin intent but never credential material', () => {
  const trusted = buildBridgeArguments({
    workspace: '/workspace', bridgeDir: '/private/bridge', model: 'model-id', hasApiKey: true, trusted: true,
  });
  assert.deepEqual(trusted, [
    '--cwd', '/workspace', '--bridge-dir', '/private/bridge', '--model', 'model-id',
    '--api-key-stdin', '--trusted-workspace',
  ]);
  assert.equal(trusted.some((argument) => argument.includes('credential-value')), false);
  assert.equal(buildBridgeArguments({ workspace: '/w', bridgeDir: '/b', hasApiKey: false, trusted: false }).includes('--trusted-workspace'), false);
});

test('packaged bridge arguments enforce stdin-only credentials without embedding secrets', () => {
  const packaged = buildBridgeArguments({
    workspace: '/workspace',
    bridgeDir: '/private/bridge',
    hasApiKey: true,
    trusted: true,
    packagedCredentialBoundary: true,
  });
  assert.deepEqual(packaged, [
    '--cwd', '/workspace',
    '--bridge-dir', '/private/bridge',
    '--api-key-stdin',
    '--trusted-workspace',
    '--packaged-credential-stdin-only',
  ]);
});

test('provider bridge arguments request a JSON credential envelope without embedding keys', () => {
  const args = buildBridgeArguments({
    workspace: '/workspace', bridgeDir: '/private/bridge', hasApiKey: true,
    hasCredentialStdin: true, trusted: true, packagedCredentialBoundary: true,
  });
  assert.ok(args.includes('--credential-stdin'));
  assert.equal(args.includes('--api-key-stdin'), false);
  assert.equal(args.join(' ').includes('sk-provider-secret'), false);
});

test('credential envelope uses the bridge-server snake_case contract', () => {
  const payload = buildCredentialEnvelope({
    apiKey: 'anthropic-secret',
    providerCredentials: { deepseek: 'provider-secret' },
  });
  assert.deepEqual(JSON.parse(payload), {
    api_key: 'anthropic-secret',
    provider_keys: { deepseek: 'provider-secret' },
  });
  assert.doesNotMatch(payload, /apiKey|providerKeys/);
});

test('diagnostics redact secrets, strip control characters, truncate, and remain bounded', () => {
  const secret = 'sk-super-secret-value';
  const sanitized = sanitizeDiagnostic(`token=${secret}\u0001 ${'x'.repeat(3_000)}`, [secret]);
  assert.doesNotMatch(sanitized, /super-secret/);
  assert.ok(sanitized.length <= 2_000);
  const buffer = new DiagnosticBuffer();
  for (let i = 0; i < MAX_DIAGNOSTICS + 5; i += 1) buffer.add('info', 'host', i);
  assert.equal(buffer.snapshot().length, MAX_DIAGNOSTICS);
  assert.equal(buffer.snapshot()[0]?.message, '5');
});

test('sanitized diagnostics persist across host restarts without secret plaintext', () => {
  const logPath = join(temporaryDirectory(), 'logs', 'desktop.jsonl');
  const secret = 'sk-persisted-secret-value';
  const first = new DiagnosticBuffer(logPath);
  first.add('error', 'bridge', `authorization=${secret}`, [secret]);
  const persisted = readFileSync(logPath, 'utf8');
  assert.doesNotMatch(persisted, /persisted-secret/);
  const second = new DiagnosticBuffer(logPath);
  assert.equal(second.snapshot().length, 1);
  assert.match(second.snapshot()[0]?.message ?? '', /REDACTED/);
});

test('settings reject secret-bearing API URLs and only reopen recorded workspaces', () => {
  const userData = temporaryDirectory();
  writeFileSync(join(userData, 'settings.v1.json'), JSON.stringify({
    version: 1,
    apiBaseUrl: 'https://api.example.test/v1?key=secret',
    recentWorkspaces: [],
    trustedWorkspaces: {},
  }));
  const store = new SettingsStore(userData);
  assert.equal(store.getPublic().apiBaseUrl, undefined);
  assert.equal(store.update({ theme: 'light' }).theme, 'light');
  assert.equal(new SettingsStore(userData).getPublic().theme, 'light');
  assert.throws(() => store.update({ apiBaseUrl: 'https://user:secret@api.example.test/v1' }), /must not contain/);
  assert.throws(() => store.update({ apiBaseUrl: 'https://api.example.test/v1#token' }), /must not contain/);

  const workspace = temporaryDirectory();
  const canonical = canonicalWorkspace(workspace);
  assert.equal(store.isRecentWorkspace(canonical), false);
  store.setWorkspace(canonical);
  assert.equal(store.isRecentWorkspace(canonical), true);
});
