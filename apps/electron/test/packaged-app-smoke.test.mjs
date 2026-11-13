import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, test } from 'node:test';

import {
  APP_USER_DATA_SUBPATH,
  assertTemporaryBrokerLaunchdIdentity,
  unregisterCredentialBrokerForColdStart,
  assertCustomProviderTurn,
  assertPackagedLocalAppCatalogSession,
  assertPackagedLocalAppAuthoring,
  assertPackagedLocalAppPendingAuthoring,
  confirmPackagedLocalAppSourceTrust,
  enterPackagedProjectName,
  createPackagedSettings,
  createPackagedConversationSpaces,
  isExpectedBootstrapPromptPlaceholder,
  isSafeProviderAccountSnapshot,
  runtimePathsForHome,
  runPackagedCredentialStability,
  sanitizePackagedAppEnvironment,
  waitForRendererCommit,
  waitForPackagedAppExit,
  workspaceTrustFingerprint,
} from '../scripts/packaged-app-smoke.mjs';
import { workspaceFingerprint as hostWorkspaceFingerprint } from '../src/main/host-utils.ts';
import { ConversationSpaceStore } from '../src/main/conversationSpaceStore.ts';

const temporaryDirectories = [];

test('packaged exit observes a clean process exit delivered during the quit request', async () => {
  const child = Object.assign(new EventEmitter(), { exitCode: null, signalCode: null });
  await waitForPackagedAppExit(child, async () => { child.exitCode = 0; child.emit('exit', 0, null); });
  assert.equal(child.listenerCount('exit'), 0);
});

test('packaged exit waits for the observed slow native profile flush and still requires a real exit', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const child = Object.assign(new EventEmitter(), { exitCode: null, signalCode: null });
  let settled = false;
  const waiting = waitForPackagedAppExit(child, async () => {});
  waiting.then(() => { settled = true; }, () => { settled = true; });
  await Promise.resolve();
  t.mock.timers.tick(18_334);
  await Promise.resolve();
  assert.equal(settled, false, 'the original 10s budget must not interrupt a native flush');
  child.exitCode = 0; child.emit('exit', 0, null);
  await waiting;
  assert.equal(child.listenerCount('exit'), 0);
});

test('packaged exit rejects a process that stays alive beyond the bounded deadline', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const child = Object.assign(new EventEmitter(), { exitCode: null, signalCode: null });
  const waiting = waitForPackagedAppExit(child, async () => {});
  const rejected = assert.rejects(waiting, /did not exit.*30000ms/);
  await Promise.resolve();
  t.mock.timers.tick(30_001);
  await rejected;
  assert.equal(child.listenerCount('exit'), 0);
});

test('packaged exit rejects crashes and signal termination instead of accepting process disappearance', async () => {
  for (const state of [{ exitCode: 1, signalCode: null }, { exitCode: null, signalCode: 'SIGTERM' }]) {
    const child = Object.assign(new EventEmitter(), state);
    await assert.rejects(waitForPackagedAppExit(child, async () => {}), /must exit/);
  }
});

test('packaged profile maps the fixture workspace into the current Code space', () => {
  const home = temporaryDirectory();
  const workspace = join(home, 'workspace');
  mkdirSync(workspace);
  const canonical = realpathSync(workspace);
  const paths = runtimePathsForHome(home);
  mkdirSync(paths.userDataDir, { recursive: true });
  writeFileSync(join(paths.userDataDir, 'conversation-spaces.v2.json'), JSON.stringify(createPackagedConversationSpaces({ workspace })));
  const store = new ConversationSpaceStore(paths.userDataDir, join(home, '.lingxi'));
  const snapshot = store.snapshot();
  assert.equal(snapshot.activeMode, 'code');
  assert.deepEqual(snapshot.projects[canonical], ['code']);
  assert.equal(snapshot.navigation.code.projectId, canonical);
  assert.deepEqual(store.trustedWorkspaces(), [canonical]);
  assert.deepEqual(snapshot.sessions, []);
});

test('packaged debugger waits for the real document commit before enabling Runtime', async () => {
  const rendererUrl = 'file:///tmp/LingXi%20Code.app/Contents/Resources/app.asar/out/renderer/index.html';
  const frames = [
    { url: 'about:blank', loaderId: 'initial' },
    { url: 'lingxi-app://other-app/', loaderId: 'guest' },
    { url: rendererUrl, loaderId: '' },
    { url: rendererUrl, loaderId: 'committed' },
  ];
  const calls = [];
  const page = { send: async method => {
    calls.push(method);
    if (method === 'Page.enable') return {};
    assert.equal(method, 'Page.getFrameTree', 'waiting must not create a JavaScript context');
    return { frameTree: { frame: frames.shift() } };
  } };
  const frame = await waitForRendererCommit(page, rendererUrl, { intervalMs: 0 });
  assert.equal(frame.loaderId, 'committed');
  assert.equal(frames.length, 0);
  assert.deepEqual(calls, ['Page.enable', ...Array(4).fill('Page.getFrameTree')]);
});

test('packaged debugger rejects a document that never commits the packaged renderer', async () => {
  const page = { send: async method => method === 'Page.enable' ? {} : {
    frameTree: { frame: { url: 'about:blank', loaderId: 'initial' } },
  } };
  await assert.rejects(waitForRendererCommit(page, 'file:///packaged/index.html', { timeoutMs: 0 }),
    /committed packaged renderer document did not become ready/);
});
test('credential stability cannot substitute a development package for production',
  { skip: process.platform !== 'darwin' }, async () => {
    const app = temporaryDirectory();
    const broker = join(app, 'Contents', 'Resources', 'credential-broker');
    mkdirSync(broker, { recursive: true });
    writeFileSync(join(broker, 'broker-manifest.json'), JSON.stringify({ channel: 'development' }));
    await assert.rejects(runPackagedCredentialStability(app), /production signed package required/);
  });

test('custom provider smoke requires a successful answer, not merely any terminal event', () => {
  const answer = { type: 'text_delta', text: 'Custom provider verified.' };
  const ended = { type: 'turn_ended', outcome: { type: 'end_turn' } };
  assert.doesNotThrow(() => assertCustomProviderTurn([answer, ended]));
  assert.throws(() => assertCustomProviderTurn([ended]));
  assert.throws(() => assertCustomProviderTurn([answer, { ...ended, outcome: { type: 'cancelled' } }]));
  assert.throws(() => assertCustomProviderTurn([answer, ended, { type: 'error', message: 'request failed' }]));
  assert.throws(() => assertCustomProviderTurn([answer, ended, { type: 'system_notice', is_error: true }]));
});

test('packaged Local App smoke requires a native dedicated source Code binding', () => {
  const sourceSession = {
    projectPath: '/tmp/native-app-source',
    sessionId: '12345678-1234-4234-8234-123456789abc',
    mode: 'code',
  };
  const app = {
    id: 'a123456789abcdef0123456789abcdef',
    authoring: {
      workspaceId: '12345678-1234-4234-8234-abcdef123456',
      sourceDirectory: sourceSession.projectPath,
      projectRef: { kind: 'desktopPath', path: sourceSession.projectPath },
      authoringSessionId: sourceSession.sessionId,
      appId: 'a123456789abcdef0123456789abcdef',
    },
  };
  assert.doesNotThrow(() => assertPackagedLocalAppAuthoring(app, sourceSession));
  assert.throws(() => assertPackagedLocalAppAuthoring({ ...app, authoring: undefined }, sourceSession), /persist its host-verified authoring binding/);
  assert.throws(() => assertPackagedLocalAppAuthoring({ ...app, authoring: { ...app.authoring, authoringSessionId: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa' } }, sourceSession), /real Code session/);
  assert.throws(() => assertPackagedLocalAppAuthoring(app, { ...sourceSession, mode: 'chat' }), /must use a Code session/);
});

test('Native Create smoke requires the host-persisted pending binding before first publish', () => {
  const sourceSession = { projectPath: '/tmp/native-app-source', sessionId: '12345678-1234-4234-8234-123456789abc', mode: 'code' };
  const authoring = { workspaceId: '12345678-1234-4234-8234-abcdef123456', sourceDirectory: sourceSession.projectPath,
    projectRef: { kind: 'desktopPath', path: sourceSession.projectPath }, authoringSessionId: sourceSession.sessionId };
  const receipt = { authoring, sourceId: 'native-source', sourceIdentity: { device: 1, inode: 2 }, sessionMode: 'code' };
  assert.strictEqual(assertPackagedLocalAppPendingAuthoring([receipt], sourceSession), authoring);
  assert.throws(() => assertPackagedLocalAppPendingAuthoring(undefined, sourceSession), /host-owned authoring receipts/);
  assert.throws(() => assertPackagedLocalAppPendingAuthoring([], sourceSession), /exactly one pending binding/);
  assert.throws(() => assertPackagedLocalAppPendingAuthoring([receipt, receipt], sourceSession), /exactly one pending binding/);
  assert.throws(() => assertPackagedLocalAppPendingAuthoring([{ ...receipt, sessionMode: 'chat' }], sourceSession), /Code session/);
  assert.throws(() => assertPackagedLocalAppPendingAuthoring([{ ...receipt, sourceIdentity: null }], sourceSession), /rooted source identity/);
  assert.throws(() => assertPackagedLocalAppPendingAuthoring([{ ...receipt, authoring: { ...authoring, sourceDirectory: '/tmp/other' } }], sourceSession), /sourceDirectory/);
});

test('packaged Local App smoke checks the persisted public Code catalog after the first prompt', () => {
  const sourceSession = { projectPath: '/tmp/native-app-source', sessionId: '12345678-1234-4234-8234-123456789abc', mode: 'code' };
  const catalog = { sessions: [{ uuid: sourceSession.sessionId, mode: 'code' }] };
  assert.strictEqual(assertPackagedLocalAppCatalogSession(catalog, sourceSession), catalog.sessions[0]);
  assert.throws(() => assertPackagedLocalAppCatalogSession(undefined, sourceSession), /persisted catalog/);
  assert.throws(() => assertPackagedLocalAppCatalogSession({ sessions: [{ ...catalog.sessions[0], mode: 'chat' }] }, sourceSession), /Code session/);
  assert.throws(() => assertPackagedLocalAppCatalogSession({ sessions: [{ ...catalog.sessions[0], workspace_path: '/tmp/other' }] }, sourceSession), /must not expose private/);
  assert.throws(() => assertPackagedLocalAppCatalogSession({ sessions: [{ ...catalog.sessions[0], uuid: 'unrelated' }] }, sourceSession), /must persist the authoring session/);
});

test('packaged Local App creation waits for the default folder selection before typing the Project name', async () => {
  const projectName = 'Packaged Local App Source';
  let inputReady = false;
  let focused = false;
  let inputValue = '';
  const page = { send: async (method, params = {}) => {
    if (method === 'Runtime.evaluate') {
      const { expression } = params;
      if (expression.includes('!input.disabled && !input.readOnly')) return { result: { value: inputReady } };
      if (expression.endsWith('.focus()')) { focused = inputReady; return { result: { value: undefined } }; }
      if (expression.includes('document.activeElement ===')) return { result: { value: focused } };
      if (expression.includes('input?.value ===')) return { result: { value: inputReady && inputValue === projectName ? inputValue : undefined } };
      throw new Error(`Unexpected renderer expression in fixture test: ${expression}`);
    }
    if (method === 'Input.insertText') {
      assert.equal(inputReady, true, 'CDP must not type into the dialog while its default folder is still loading');
      assert.equal(focused, true, 'CDP must type into the focused Project name input');
      inputValue += params.text;
      return {};
    }
    throw new Error(`Unexpected CDP method in fixture test: ${method}`);
  } };
  setTimeout(() => { inputReady = true; }, 15);
  await enterPackagedProjectName(page, projectName);
  assert.equal(inputValue, projectName);
});

test('packaged Local App smoke confirms only its exact generated source folder', async () => {
  const expected = '/tmp/isolated-home/.lingxi-workspaces/apps/Smoke App';
  const makePage = (actual, clicked) => {
    const calls = [];
    return { calls, send: async (method, { expression }) => {
      assert.equal(method, 'Runtime.evaluate');
      calls.push(expression);
      return { result: { value: expression.includes('.trust-folder-dialog .trust-folder-path') ? actual : clicked } };
    } };
  };
  const matching = makePage(expected, true);
  await confirmPackagedLocalAppSourceTrust(matching, expected);
  assert.equal(matching.calls.length, 2);
  assert.match(matching.calls[1], /trust-folder-confirm/);

  const wrongFolder = makePage('/tmp/unrelated', true);
  await assert.rejects(confirmPackagedLocalAppSourceTrust(wrongFolder, expected), /must never trust a different folder/);
  assert.equal(wrongFolder.calls.length, 1, 'a mismatched folder must never be confirmed');

  const changedDialog = makePage(expected, false);
  await assert.rejects(confirmPackagedLocalAppSourceTrust(changedDialog, expected), /must remain available/);
});

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

test('packaged bootstrap validates account metadata and rejects nested secret payloads', () => {
  const account = { accountId: 'account-id', providerId: 'openai', displayName: 'Test account', authKind: 'oauth',
    executionBindingId: 'openai-chatgpt', credentialRef: { storeScope: 'test-scope', secretRef: 'acct.test' },
    enabled: true, userPriority: 0, configRevision: 1, identityGeneration: 1 };
  const snapshot = { storeScope: 'test-scope', configRevision: 1, accounts: [account], health: [], modelEntitlements: [], modelAvailability: [] };
  assert.equal(isSafeProviderAccountSnapshot(snapshot), true);
  const scopedHealth = [{ accountId: account.accountId, identityGeneration: 1, scopeKind: 'model', scopeId: 'test-model', unavailableUntilMs: 42, reason: 'rate_limited' }];
  assert.equal(isSafeProviderAccountSnapshot({ ...snapshot, scopedHealth }), true);
  assert.equal(isSafeProviderAccountSnapshot({ ...snapshot, scopedHealth: [{ ...scopedHealth[0], access_token: 'secret' }] }), false);
  assert.equal(isSafeProviderAccountSnapshot({ ...snapshot, accounts: [] }), true);
  assert.equal(isSafeProviderAccountSnapshot(undefined), false);
  assert.equal(isSafeProviderAccountSnapshot({ ...snapshot, access_token: 'secret' }), false);
  assert.equal(isSafeProviderAccountSnapshot({ ...snapshot, accounts: [{ ...account, refresh_token: 'secret' }] }), false);
  assert.equal(isSafeProviderAccountSnapshot({ ...snapshot, accounts: [{ ...account, credentialRef: { ...account.credentialRef, apiKey: 'secret' } }] }), false);
});

test('packaged renderer accepts bootstrap progress and verifies an already-ready composer', () => {
  assert.equal(isExpectedBootstrapPromptPlaceholder('Connect a provider in Settings to start coding…'), true);
  assert.equal(isExpectedBootstrapPromptPlaceholder('Waiting for the local engine…'), true);
  assert.equal(isExpectedBootstrapPromptPlaceholder('Ready'), false);
  assert.equal(isExpectedBootstrapPromptPlaceholder('Do anything'), false);
  assert.equal(isExpectedBootstrapPromptPlaceholder('Do anything', { connection: { status: 'connecting' }, workspace: { trusted: true } }), false);
  assert.equal(isExpectedBootstrapPromptPlaceholder('Do anything', { connection: { status: 'connected' }, workspace: { trusted: false } }), false);
  assert.equal(isExpectedBootstrapPromptPlaceholder('Do anything', { connection: { status: 'connected' }, workspace: { trusted: true } }), true);
});

test('packaged settings preseed a trusted active project without secrets', () => {
  const workspace = temporaryDirectory();
  const settings = createPackagedSettings({ workspace, theme: 'dark', now: new Date('2026-07-21T00:00:00Z') });
  const canonical = realpathSync.native(workspace);

  assert.equal(settings.version, 1);
  assert.equal(settings.theme, 'dark');
  assert.equal(settings.activeProject, canonical);
  assert.deepEqual(settings.projects, [canonical]);
  assert.deepEqual(settings.pinnedSessions, []);
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


test('cold-start waits for asynchronous launchd removal and refuses unrelated failures', {
  skip: typeof process.getuid !== 'function',
}, async () => {
  const calls = [];
  const results = [{ status: 0 }, { status: 0 }, { status: 113 }];
  await unregisterCredentialBrokerForColdStart('development', (_bin, args) => {
    calls.push(args);
    return results.shift();
  });
  assert.deepEqual(calls.map(args => args[0]), ['bootout', 'print', 'print']);
  assert.ok(calls.every(args => args[1].endsWith('/com.lingxi.code.credential-broker.development')));
  await unregisterCredentialBrokerForColdStart('development', (_bin, args) => ({ status: args[0] === 'bootout' ? 3 : 113 }));
  await assert.rejects(unregisterCredentialBrokerForColdStart('development', () => ({ status: null })), /bootout failed/);
  await assert.rejects(unregisterCredentialBrokerForColdStart('development', (_bin, args) => ({ status: args[0] === 'bootout' ? 0 : 1 })), /launchctl failed/);
});

test('temporary Broker guard rejects foreign and replaced launchd identities', {
  skip: typeof process.getuid !== 'function',
}, () => {
  const home = temporaryDirectory();
  const label = 'com.lingxi.code.credential-broker.development';
  const target = `gui/${process.getuid()}/${label}`;
  const plistPath = join(home, 'Library', 'LaunchAgents', `${label}.plist`);
  const programPath = join(home, 'Library', 'Application Support', 'LingXi', 'CredentialBroker',
    'development', 'LingXiCredentialBroker.app', 'Contents', 'MacOS', 'LingXiCredentialBroker');
  const replacementProgramPath = join(home, 'replacement', 'LingXiCredentialBroker');
  const outsideHome = temporaryDirectory();
  const outsideProgramPath = join(outsideHome, 'LingXiCredentialBroker');
  mkdirSync(join(home, 'Library', 'LaunchAgents'), { recursive: true });
  mkdirSync(join(programPath, '..'), { recursive: true });
  mkdirSync(join(replacementProgramPath, '..'), { recursive: true });
  writeFileSync(plistPath, 'fixture plist');
  writeFileSync(programPath, 'expected executable');
  writeFileSync(replacementProgramPath, 'replacement executable');
  writeFileSync(outsideProgramPath, 'outside executable');

  const paths = {
    target,
    plistPath: realpathSync.native(plistPath),
    programPath: realpathSync.native(programPath),
  };
  const output = (service, plist, program) => `${service} = {\n\tpath = ${plist}\n\tprogram = ${program}\n\tpid = 4242\n}\n`;

  assert.deepEqual(assertTemporaryBrokerLaunchdIdentity(output(target, plistPath, programPath), paths), {
    plistPath: paths.plistPath,
    programPath: paths.programPath,
    pid: 4242,
  });
  assert.throws(() => assertTemporaryBrokerLaunchdIdentity(
    output(target, plistPath, outsideProgramPath), paths,
  ), /does not match this isolated smoke home/);
  assert.throws(() => assertTemporaryBrokerLaunchdIdentity(
    output(target, plistPath, replacementProgramPath), paths,
  ), /does not match this isolated smoke home/);
  assert.throws(() => assertTemporaryBrokerLaunchdIdentity(
    output(`gui/${process.getuid()}/com.lingxi.code.credential-broker`, plistPath, programPath), paths,
  ), /service label must match/);
  const invalidPid = output(target, plistPath, programPath).replace('pid = 4242', 'pid = not-a-pid');
  assert.throws(() => assertTemporaryBrokerLaunchdIdentity(invalidPid, paths), /PID must be valid/);
});
