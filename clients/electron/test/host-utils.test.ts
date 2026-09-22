import { afterEach, test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import {
  DiagnosticBuffer,
  MAX_DIAGNOSTICS,
  MAX_PINNED_SESSIONS,
  MAX_PROJECTS,
  buildBridgeArguments,
  buildBridgeEnvironment,
  buildCredentialEnvelope,
  canonicalWorkspace,
  defaultSettings,
  parseSettings,
  publicSettings,
  sanitizeDiagnostic,
  setWorkspaceTrust,
  withActiveProject,
  withAddedProject,
  withoutProject,
  withSessionPinned,
  workspaceFingerprint,
  workspaceTrust,
  type VoicePreferences,
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

test('settings parser fails closed and migrates legacy workspaces into bounded projects', () => {
  assert.deepEqual(parseSettings({ version: 999, lastWorkspace: '/unsafe' }), defaultSettings());
  const parsed = parseSettings({
    version: 1,
    lastWorkspace: '/p/active',
    recentWorkspaces: ['/p/0', '/p/active', ...Array.from({ length: 80 }, (_, i) => `/p/${i + 1}`)],
  });
  assert.equal(parsed.projects.length, MAX_PROJECTS);
  assert.equal(parsed.activeProject, '/p/active');
  assert.deepEqual(parsed.projects.slice(0, 2), ['/p/active', '/p/0']);
  assert.deepEqual(parsed.pinnedSessions, []);

  const explicitlyEmpty = parseSettings({
    version: 1,
    projects: [],
    lastWorkspace: '/legacy/should-not-return',
    recentWorkspaces: ['/legacy/should-not-return'],
  });
  assert.deepEqual(explicitlyEmpty.projects, []);
  assert.equal(explicitlyEmpty.activeProject, undefined);
});

test("parseSettings keeps 'system' and still rejects garbage", () => {
  assert.equal(parseSettings({ version: 1, theme: 'system', projects: [] }).theme, 'system');
  assert.equal(parseSettings({ version: 1, theme: 'dark', projects: [] }).theme, 'dark');
  assert.equal(
    parseSettings({ version: 1, theme: 'chartreuse', projects: [] }).theme, undefined,
    'an unknown theme must still be dropped, not passed through',
  );
});

test('sidebar preferences persist only valid manual session orders', () => {
  const project = '/project';
  const sessionId = '123e4567-e89b-42d3-a456-426614174000';
  const settings = parseSettings({
    version: 1,
    projects: [project],
    sidebar: {
      organization: 'list',
      chatSort: 'manual',
      manualSessionOrder: {
        [project]: [sessionId, sessionId, 'not-a-session-id'],
        '/not-in-projects': [sessionId],
      },
    },
  });
  assert.deepEqual(settings.sidebar, {
    organization: 'list',
    chatSort: 'manual',
    manualSessionOrder: { [project]: [sessionId] },
  });
  const publicCopy = publicSettings(settings);
  publicCopy.sidebar!.manualSessionOrder[project].push('123e4567-e89b-42d3-a456-426614174001');
  assert.deepEqual(settings.sidebar!.manualSessionOrder[project], [sessionId]);
});

test('thought collapse preference is omitted for legacy settings and rejects invalid stored values', () => {
  assert.equal(new SettingsStore(temporaryDirectory()).getPublic().collapseThoughtsByDefault, undefined);
  for (const value of [undefined, null, 'false', 'true', 0, 1, {}, []]) {
    const userData = temporaryDirectory();
    writeFileSync(join(userData, 'settings.v1.json'), JSON.stringify({
      version: 1, theme: 'light', collapseThoughtsByDefault: value,
    }));
    const settings = new SettingsStore(userData).getPublic();
    assert.equal(settings.collapseThoughtsByDefault, undefined);
    assert.equal(settings.theme, 'light', 'invalid preference must not discard other settings');
  }
});

test('thought collapse preference preserves explicit false and true across restarts and unrelated updates', () => {
  const userData = temporaryDirectory();
  let store = new SettingsStore(userData);
  for (const collapseThoughtsByDefault of [false, true, false]) {
    assert.equal(store.update({ collapseThoughtsByDefault }).collapseThoughtsByDefault, collapseThoughtsByDefault);
    assert.equal(store.update({ theme: 'dark' }).collapseThoughtsByDefault, collapseThoughtsByDefault);
    const persisted = JSON.parse(readFileSync(store.settingsPath, 'utf8'));
    assert.equal(persisted.collapseThoughtsByDefault, collapseThoughtsByDefault);
    store = new SettingsStore(userData);
    assert.equal(store.getPublic().collapseThoughtsByDefault, collapseThoughtsByDefault);
  }
});

test('thought collapse preference rejects non-boolean writes without changing saved state', () => {
  const userData = temporaryDirectory();
  const store = new SettingsStore(userData);
  store.update({ collapseThoughtsByDefault: false, theme: 'light' });
  const saved = readFileSync(store.settingsPath, 'utf8');
  for (const value of [undefined, null, 'false', 'true', 0, 1, {}, []]) {
    assert.throws(() => store.update({
      collapseThoughtsByDefault: value as boolean,
      theme: 'dark',
    }), /invalid collapseThoughtsByDefault/);
    assert.equal(store.getPublic().collapseThoughtsByDefault, false);
    assert.equal(store.getPublic().theme, 'light');
    assert.equal(readFileSync(store.settingsPath, 'utf8'), saved);
  }
});

test('model picker visibility defaults open, dedupes ids, and persists empty allowlists', () => {
  const parsed = parseSettings({
    version: 1,
    projects: [],
    modelPickerVisibility: {
      openai: { visibleModelIds: ['gpt-5.6-sol', 'gpt-5.6-sol', '', 'gpt-5.7-preview'] },
      deepseek: { showInModelPicker: false, visibleModelIds: [] },
    },
  });
  assert.deepEqual(parsed.modelPickerVisibility, {
    openai: { visibleModelIds: ['gpt-5.6-sol', 'gpt-5.7-preview'] },
    deepseek: { showInModelPicker: false, visibleModelIds: [] },
  });
  assert.deepEqual(publicSettings(parsed).modelPickerVisibility, parsed.modelPickerVisibility);

  const userData = temporaryDirectory();
  const store = new SettingsStore(userData);
  store.update({
    modelPickerVisibility: {
      openai: { visibleModelIds: ['gpt-5.6-sol'] },
      anthropic: { showInModelPicker: false, visibleModelIds: [] },
    },
  });
  assert.deepEqual(new SettingsStore(userData).getPublic().modelPickerVisibility, {
    openai: { visibleModelIds: ['gpt-5.6-sol'] },
    anthropic: { showInModelPicker: false, visibleModelIds: [] },
  });
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

test('projects are canonical, stable across activation, and new projects are prepended once', () => {
  const first = canonicalWorkspace(join(temporaryDirectory(), '.'));
  const second = canonicalWorkspace(join(temporaryDirectory(), '.'));
  let settings = withAddedProject(defaultSettings(), first);
  settings = withAddedProject(settings, second);
  settings = withActiveProject(settings, first);
  settings = withAddedProject(settings, first);
  assert.equal(settings.activeProject, first);
  assert.deepEqual(settings.projects, [second, first]);
});

test('pin records dedupe by project and session and project removal clears pins and trust', () => {
  const first = temporaryDirectory();
  const second = temporaryDirectory();
  const sessionId = '11111111-1111-4111-8111-111111111111';
  let settings = withAddedProject(defaultSettings(), first);
  settings = withAddedProject(settings, second);
  settings = setWorkspaceTrust(settings, first, true, new Date('2026-01-01T00:00:00Z'));
  settings = withSessionPinned(settings, {
    projectPath: first,
    sessionId,
    title: 'First title',
    pinnedAt: '2026-01-01T00:00:00Z',
  }, true);
  settings = withSessionPinned(settings, {
    projectPath: first,
    sessionId,
    title: 'Updated title',
    pinnedAt: '2026-01-02T00:00:00Z',
  }, true);
  assert.deepEqual(settings.pinnedSessions.map((session) => session.title), ['Updated title']);

  settings = withoutProject(settings, first);
  assert.deepEqual(settings.projects, [second]);
  assert.equal(settings.pinnedSessions.length, 0);
  assert.equal(settings.trustedWorkspaces[first], undefined);
  assert.equal(settings.activeProject, second);
});

test('project and pin limits reject additions instead of silently evicting records', () => {
  let settings = defaultSettings();
  for (let index = 0; index < MAX_PROJECTS; index += 1) {
    settings = withAddedProject(settings, `/project/${index}`);
  }
  assert.throws(() => withAddedProject(settings, '/project/overflow'), /Project limit reached/);

  const projectPath = settings.activeProject!;
  settings.pinnedSessions = Array.from({ length: MAX_PINNED_SESSIONS }, (_, index) => ({
    projectPath,
    sessionId: `${index.toString(16).padStart(8, '0')}-0000-4000-8000-000000000000`,
    title: `Session ${index}`,
    pinnedAt: new Date(index).toISOString(),
  }));
  assert.throws(() => withSessionPinned(settings, {
    projectPath,
    sessionId: 'ffffffff-ffff-4fff-8fff-ffffffffffff',
    title: 'Overflow',
    pinnedAt: new Date().toISOString(),
  }, true), /Pinned session limit reached/);
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
    pluginSecrets: { 'weather@official': { API_KEY: 'plugin-secret' } },
  });
  assert.deepEqual(JSON.parse(payload), {
    api_key: 'anthropic-secret',
    provider_keys: { deepseek: 'provider-secret' },
    plugin_secrets: { 'weather@official': { API_KEY: 'plugin-secret' } },
  });
  assert.doesNotMatch(payload, /apiKey|providerKeys|pluginSecrets/);
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

test('failure diagnostics survive live buffer eviction and rotate with redaction', () => {
  const logPath = join(temporaryDirectory(), 'desktop.jsonl');
  const buffer = new DiagnosticBuffer(logPath);
  buffer.add('error', 'bridge', 'fusion failed token=private-value', ['private-value']);
  for (let i = 0; i < MAX_DIAGNOSTICS; i++) buffer.add('info', 'bridge', `activity ${i}`);
  assert.equal(buffer.snapshot().some(entry => entry.level === 'error'), false);
  const saved = readFileSync(`${logPath}.errors`, 'utf8');
  assert.match(saved, /fusion failed/);
  assert.doesNotMatch(saved, /private-value|activity/);
  writeFileSync(`${logPath}.errors`, 'x'.repeat(2_000_000));
  buffer.add('warn', 'bridge', 'next failure');
  assert.equal(readFileSync(`${logPath}.errors.1`, 'utf8').length, 2_000_000);
  assert.equal(JSON.parse(readFileSync(`${logPath}.errors`, 'utf8')).message, 'next failure');
});

test('settings reject secret-bearing API URLs and only activate recorded projects', () => {
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
  assert.equal(store.hasProject(canonical), false);
  store.addProject(canonical);
  assert.equal(store.hasProject(canonical), true);
  assert.equal(store.getPublic().activeProject, canonical);
  store.activateProject(canonical);
  assert.deepEqual(new SettingsStore(userData).getPublic().projects, [canonical]);
});

test('settings store persists project activation, pins, and recoverable removal', () => {
  const userData = temporaryDirectory();
  const first = canonicalWorkspace(temporaryDirectory());
  const second = canonicalWorkspace(temporaryDirectory());
  const store = new SettingsStore(userData);
  store.addProject(first);
  store.addProject(second);
  store.activateProject(first);
  store.setTrust(second, true);
  store.setSessionPinned({
    projectPath: second,
    sessionId: '22222222-2222-4222-8222-222222222222',
    title: 'Pinned session',
    pinnedAt: '2026-08-26T00:00:00Z',
  }, true);

  let restored = new SettingsStore(userData);
  assert.equal(restored.getWorkspace(), first);
  assert.deepEqual(restored.getPublic().projects, [second, first]);
  assert.equal(restored.getPublic().pinnedSessions[0]?.title, 'Pinned session');

  restored.removeProject(second);
  restored = new SettingsStore(userData);
  assert.deepEqual(restored.getPublic().projects, [first]);
  assert.deepEqual(restored.getPublic().pinnedSessions, []);
  assert.equal(restored.getTrust(second).trusted, false);
});

test('settings store keeps draft active sessions volatile until committed', () => {
  const userData = temporaryDirectory();
  const first = canonicalWorkspace(temporaryDirectory());
  const second = canonicalWorkspace(temporaryDirectory());
  const firstRef = { projectPath: first, sessionId: 'aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa' };
  const secondRef = { projectPath: second, sessionId: 'bbbbbbbb-2222-4222-8222-bbbbbbbbbbbb' };
  const store = new SettingsStore(userData);
  store.addProject(first);
  store.addProject(second);

  store.setActiveSessionDraft(firstRef);
  assert.deepEqual(store.getPublic().activeSession, firstRef);
  assert.equal(JSON.parse(readFileSync(store.settingsPath, 'utf8')).activeSession, undefined);

  store.update({ theme: 'light' });
  assert.deepEqual(store.getPublic().activeSession, firstRef);
  assert.equal(JSON.parse(readFileSync(store.settingsPath, 'utf8')).activeSession, undefined);

  store.setActiveSession(firstRef);
  assert.deepEqual(store.getPublic().activeSession, firstRef);
  assert.deepEqual(JSON.parse(readFileSync(store.settingsPath, 'utf8')).activeSession, firstRef);

  store.setActiveSessionDraft(secondRef);
  assert.deepEqual(store.getPublic().activeSession, secondRef);
  const originalPersist = (store as any).persist;
  (store as any).persist = () => { throw new Error('settings disk is read-only'); };
  assert.throws(() => store.setActiveSession(secondRef), /settings disk is read-only/);
  assert.deepEqual(store.getPublic().activeSession, secondRef);
  (store as any).persist = originalPersist;
  store.removeProject(second);
  assert.deepEqual(store.getPublic().activeSession, firstRef);
  assert.equal(JSON.parse(readFileSync(store.settingsPath, 'utf8')).activeSession.projectPath, first);
});

// ---------------------------------------------------------------------------
// Task 4: voice preferences wired through parseSettings/publicSettings/update,
// on top of the pure-function contract pinned by test/voice-preferences.test.ts.
// ---------------------------------------------------------------------------

test('parseSettings leaves voice absent when never persisted, matching every other optional field', () => {
  const parsed = parseSettings({ version: 1, projects: [], pinnedSessions: [], trustedWorkspaces: {} });
  assert.equal(parsed.voice, undefined);
});

test('parseSettings normalizes a persisted voice value, repairing garbage rather than dropping it', () => {
  const parsed = parseSettings({
    version: 1,
    projects: [],
    pinnedSessions: [],
    trustedWorkspaces: {},
    // 'onDevice' is the Swift case name, not a persisted value on either
    // mobile platform; rate is out of range. Both must be repaired, not
    // rejected wholesale.
    voice: { recognitionMode: 'onDevice', rate: 99, voiceSelection: 'Alex' },
  });
  const expected: VoicePreferences = {
    schemaVersion: 2,
    recognitionMode: 'automatic',
    language: 'auto',
    voiceSelection: 'system:Alex',
    rate: 2,
    autoPlayReplies: false,
  };
  assert.deepEqual(parsed.voice, expected);
});

test('publicSettings passes voice through as an independent copy', () => {
  const parsed = parseSettings({
    version: 1, projects: [], pinnedSessions: [], trustedWorkspaces: {},
    voice: { recognitionMode: 'localOnly' },
  });
  const pub1 = publicSettings(parsed);
  assert.deepEqual(pub1.voice, parsed.voice);
  if (pub1.voice) pub1.voice.recognitionMode = 'automatic';
  assert.equal(parsed.voice?.recognitionMode, 'localOnly', 'mutating the returned copy must not affect stored settings');
});

test('SettingsStore.update writes and reloads voice preferences, normalized', () => {
  const userData = temporaryDirectory();
  const store = new SettingsStore(userData);
  assert.equal(store.getPublic().voice, undefined, 'a fresh store has no voice preferences yet');

  const result = store.update({ voice: { recognitionMode: 'localOnly', language: 'ZH-cn', rate: 0.1 } });
  const expected: VoicePreferences = {
    schemaVersion: 2,
    recognitionMode: 'localOnly',
    language: 'ZH-cn',
    voiceSelection: 'system:default',
    rate: 0.5,
    autoPlayReplies: false,
  };
  assert.deepEqual(result.voice, expected);
  assert.deepEqual(new SettingsStore(userData).getPublic().voice, expected, 'voice preferences must survive a reload');

  // A later update() replaces the whole snapshot, matching how both mobile
  // platforms persist it (never a partial per-field merge).
  const replaced = store.update({ voice: { rate: 1.75 } });
  assert.deepEqual(replaced.voice, {
    schemaVersion: 2,
    recognitionMode: 'automatic',
    language: 'auto',
    voiceSelection: 'system:default',
    rate: 1.75,
    autoPlayReplies: false,
  }, 'the earlier localOnly/ZH-cn values must be replaced wholesale, not merged into');
});

test('Codex OAuth credentials use only the private stdin envelope', () => {
  const session = { access_token: 'oauth-access-secret', refresh_token: 'oauth-refresh-secret', expires_at: 123, account_id: 'account', fedramp: false };
  assert.deepEqual(JSON.parse(buildCredentialEnvelope({ openaiOAuth: session })).openai_oauth, session);
  const args = buildBridgeArguments({ workspace: '/tmp', bridgeDir: '/tmp/bridge', hasApiKey: false, hasCredentialStdin: true, trusted: true });
  assert.ok(args.includes('--credential-stdin'));
  assert.ok(!JSON.stringify(args).includes('oauth-access-secret'));
  assert.equal(buildBridgeEnvironment({ OPENAI_ACCESS_TOKEN: session.access_token }).OPENAI_ACCESS_TOKEN, undefined);
});

test('only a controller session asks the engine to run the automation scheduler', () => {
  const base = { workspace: '/w', bridgeDir: '/w/bridge', hasApiKey: false, trusted: true };
  assert.ok(!buildBridgeArguments(base).includes('--scheduled-controller'));
  assert.ok(!buildBridgeArguments({ ...base, scheduledController: false }).includes('--scheduled-controller'));
  assert.ok(buildBridgeArguments({ ...base, scheduledController: true }).includes('--scheduled-controller'));
});
