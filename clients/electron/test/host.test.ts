import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, realpathSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { HostController } from '../src/main/host';
import { DiagnosticBuffer } from '../src/main/host-utils';
import { SettingsStore } from '../src/main/settings';

function deferred<T = void>(): { promise: Promise<T>; resolve(value?: T): void; reject(error: unknown): void } {
  let resolvePromise!: (value: T) => void;
  let rejectPromise!: (error: unknown) => void;
  const promise = new Promise<T>((resolve, reject) => {
    resolvePromise = resolve;
    rejectPromise = reject;
  });
  return { promise, resolve: (value?: T) => resolvePromise(value as T), reject: rejectPromise };
}

test('adding the first project uses the real settings store, trusts it, and starts one session without restarting', async () => {
  const userData = mkdtempSync(join(tmpdir(), 'lingxi-auto-trust-settings-'));
  const workspace = mkdtempSync(join(tmpdir(), 'lingxi-auto-trust-project-'));
  const calls: string[] = [];
  const settings = new SettingsStore(userData);
  const sessionId = '11111111-2222-4333-8444-555555555555';
  const bridge = {
    newSession: async (projectPath: string) => {
      calls.push(`new:${projectPath}`);
      return { projectPath, sessionId };
    },
  };
  const host = new HostController(settings as any, bridge as any, new DiagnosticBuffer());

  try {
    const result = await (host as any).selectWorkspace(workspace, true);
    const canonical = realpathSync.native(workspace);
    assert.deepEqual(calls, [`new:${canonical}`]);
    assert.equal(result.trusted, true);
    assert.deepEqual(settings.getPublic().activeSession, { projectPath: canonical, sessionId });
  } finally {
    rmSync(userData, { recursive: true, force: true });
    rmSync(workspace, { recursive: true, force: true });
  }
});

test('activating an existing project always returns membership-backed trust', async () => {
  const userData = mkdtempSync(join(tmpdir(), 'lingxi-existing-project-settings-'));
  const projectDirectory = mkdtempSync(join(tmpdir(), 'lingxi-existing-project-'));
  const project = realpathSync.native(projectDirectory);
  const settings = new SettingsStore(userData);
  settings.addProject(project);
  const host = new HostController(settings, {} as any, new DiagnosticBuffer());
  try {
    const metadata = await (host as any).selectWorkspace(project, false);
    assert.equal(metadata.path, project);
    assert.equal(metadata.trusted, true);
  } finally {
    rmSync(userData, { recursive: true, force: true });
    rmSync(projectDirectory, { recursive: true, force: true });
  }
});

test('failed session resume does not change the selected project or active session', async () => {
  const userData = mkdtempSync(join(tmpdir(), 'lingxi-transactional-open-settings-'));
  const firstProject = mkdtempSync(join(tmpdir(), 'lingxi-first-project-'));
  const targetProject = mkdtempSync(join(tmpdir(), 'lingxi-target-project-'));
  const settings = new SettingsStore(userData);
  const first = realpathSync.native(firstProject);
  const target = realpathSync.native(targetProject);
  const firstSession = '11111111-2222-4333-8444-555555555555';
  const targetSession = '22222222-3333-4444-8555-666666666666';
  settings.addProject(first);
  settings.addProject(target);
  settings.activateProject(first);
  settings.setActiveSession({ projectPath: first, sessionId: firstSession });
  const bridge = {
    get: () => undefined,
    openSession: async () => { throw new Error('resume failed'); },
  };
  const catalog = {
    list: async () => ({ sessions: [{ uuid: targetSession, title: 'Target', modified_rfc3339: '', message_count: 1, path: 'target.jsonl' }] }),
    find: async (_projectPath: string, requestedId: string) => requestedId === targetSession
      ? { uuid: targetSession, title: 'Target', modified_rfc3339: '', message_count: 1, path: 'target.jsonl' }
      : undefined,
  };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), catalog as any);
  try {
    await assert.rejects(
      host.openSessionAndActivate({ projectPath: target, sessionId: targetSession }),
      /resume failed/,
    );
    assert.equal(settings.getPublic().activeProject, first);
    assert.deepEqual(settings.getPublic().activeSession, { projectPath: first, sessionId: firstSession });
  } finally {
    rmSync(userData, { recursive: true, force: true });
    rmSync(firstProject, { recursive: true, force: true });
    rmSync(targetProject, { recursive: true, force: true });
  }
});

test('navigation mutations execute in invocation order and the last open wins', async () => {
  const userData = mkdtempSync(join(tmpdir(), 'lingxi-ordered-navigation-settings-'));
  const projectDirectoryA = mkdtempSync(join(tmpdir(), 'lingxi-ordered-navigation-a-'));
  const projectDirectoryB = mkdtempSync(join(tmpdir(), 'lingxi-ordered-navigation-b-'));
  const projectA = realpathSync.native(projectDirectoryA);
  const projectB = realpathSync.native(projectDirectoryB);
  const sessionA = '66666666-7777-4888-8999-aaaaaaaaaaaa';
  const sessionB = '77777777-8888-4999-8aaa-bbbbbbbbbbbb';
  const settings = new SettingsStore(userData);
  settings.addProject(projectA);
  settings.addProject(projectB);
  settings.setTrust(projectA, true);
  settings.setTrust(projectB, true);
  const gates = new Map([[sessionA, deferred<void>()], [sessionB, deferred<void>()]]);
  const calls: string[] = [];
  const bridge = {
    get: () => undefined,
    isProjectClosing: () => false,
    openSession: async (ref: { sessionId: string }) => {
      calls.push(ref.sessionId);
      await gates.get(ref.sessionId)?.promise;
    },
  };
  const catalog = {
    find: async (_projectPath: string, sessionId: string) => ({
      uuid: sessionId,
      title: sessionId,
      modified_rfc3339: '',
      message_count: 1,
      path: `${sessionId}.jsonl`,
    }),
  };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), catalog as any);

  try {
    const first = host.openSessionAndActivate({ projectPath: projectA, sessionId: sessionA });
    await new Promise<void>((resolve) => setImmediate(resolve));
    const second = host.openSessionAndActivate({ projectPath: projectB, sessionId: sessionB });
    await new Promise<void>((resolve) => setImmediate(resolve));
    assert.deepEqual(calls, [sessionA]);

    gates.get(sessionA)?.resolve();
    await first;
    await new Promise<void>((resolve) => setImmediate(resolve));
    assert.deepEqual(calls, [sessionA, sessionB]);
    gates.get(sessionB)?.resolve();
    await second;

    assert.equal(settings.getPublic().activeProject, projectB);
    assert.deepEqual(settings.getPublic().activeSession, { projectPath: projectB, sessionId: sessionB });
  } finally {
    rmSync(userData, { recursive: true, force: true });
    rmSync(projectDirectoryA, { recursive: true, force: true });
    rmSync(projectDirectoryB, { recursive: true, force: true });
  }
});

test('historical startup opens the catalog session before persisting it active', async () => {
  const userData = mkdtempSync(join(tmpdir(), 'lingxi-restore-settings-'));
  const projectDirectory = mkdtempSync(join(tmpdir(), 'lingxi-restore-project-'));
  const project = realpathSync.native(projectDirectory);
  const sessionId = '33333333-4444-4555-8666-777777777777';
  const settings = new SettingsStore(userData);
  settings.addProject(project);
  const calls: string[] = [];
  const bridge = {
    get: () => undefined,
    openSession: async (ref: { sessionId: string }) => { calls.push(`open:${ref.sessionId}`); },
    newSession: async () => { throw new Error('must restore history instead of creating'); },
  };
  const catalog = {
    list: async () => ({ sessions: [{ uuid: sessionId, title: 'History', modified_rfc3339: '', message_count: 2, path: 'history.jsonl' }] }),
  };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), catalog as any);
  try {
    await host.restoreProjectSession(project);
    assert.deepEqual(calls, [`open:${sessionId}`]);
    assert.deepEqual(settings.getPublic().activeSession, { projectPath: project, sessionId });
  } finally {
    rmSync(userData, { recursive: true, force: true });
    rmSync(projectDirectory, { recursive: true, force: true });
  }
});

test('session open rejects a UUID absent from the selected project catalog', async () => {
  const userData = mkdtempSync(join(tmpdir(), 'lingxi-session-owner-settings-'));
  const projectDirectory = mkdtempSync(join(tmpdir(), 'lingxi-session-owner-project-'));
  const project = realpathSync.native(projectDirectory);
  const settings = new SettingsStore(userData);
  settings.addProject(project);
  let opened = false;
  const host = new HostController(
    settings,
    { get: () => undefined, openSession: async () => { opened = true; } } as any,
    new DiagnosticBuffer(),
    { list: async () => ({ sessions: [] }), find: async () => undefined } as any,
  );
  try {
    await assert.rejects(
      host.openSessionAndActivate({ projectPath: project, sessionId: '44444444-5555-4666-8777-888888888888' }),
      /does not belong/,
    );
    assert.equal(opened, false);
  } finally {
    rmSync(userData, { recursive: true, force: true });
    rmSync(projectDirectory, { recursive: true, force: true });
  }
});

test('session open accepts a matching UUID outside the bounded project list', async () => {
  const userData = mkdtempSync(join(tmpdir(), 'lingxi-session-owner-long-settings-'));
  const projectDirectory = mkdtempSync(join(tmpdir(), 'lingxi-session-owner-long-project-'));
  const project = realpathSync.native(projectDirectory);
  const sessionId = '55555555-6666-4777-8888-999999999999';
  const settings = new SettingsStore(userData);
  settings.addProject(project);
  let opened: { projectPath: string; sessionId: string } | undefined;
  const host = new HostController(
    settings,
    {
      get: () => undefined,
      openSession: async (ref: { projectPath: string; sessionId: string }) => { opened = ref; },
    } as any,
    new DiagnosticBuffer(),
    {
      list: async () => ({ sessions: [] }),
      find: async (_projectPath: string, requestedId: string) => requestedId === sessionId
        ? { uuid: sessionId, title: 'Old session', modified_rfc3339: '', message_count: 1, path: 'old.jsonl' }
        : undefined,
    } as any,
  );
  try {
    await host.openSessionAndActivate({ projectPath: project, sessionId });
    assert.deepEqual(opened, { projectPath: project, sessionId });
    assert.deepEqual(settings.getPublic().activeSession, { projectPath: project, sessionId });
  } finally {
    rmSync(userData, { recursive: true, force: true });
    rmSync(projectDirectory, { recursive: true, force: true });
  }
});

test('host passes zero-count catalog rows to the empty-session open path', async () => {
  const userData = mkdtempSync(join(tmpdir(), 'lingxi-empty-session-settings-'));
  const projectDirectory = mkdtempSync(join(tmpdir(), 'lingxi-empty-session-project-'));
  const project = realpathSync.native(projectDirectory);
  const sessionId = '88888888-9999-4aaa-8bbb-cccccccccccc';
  const settings = new SettingsStore(userData);
  settings.addProject(project);
  let opened: { ref: { projectPath: string; sessionId: string }; empty?: boolean } | undefined;
  const host = new HostController(
    settings,
    {
      get: () => undefined,
      openSession: async (ref: { projectPath: string; sessionId: string }, empty?: boolean) => { opened = { ref, empty }; },
    } as any,
    new DiagnosticBuffer(),
    {
      list: async () => ({ sessions: [] }),
      find: async () => ({ uuid: sessionId, title: 'Empty', modified_rfc3339: '', message_count: 0, path: 'empty.jsonl', empty_session: true }),
    } as any,
  );
  try {
    await host.openSessionAndActivate({ projectPath: project, sessionId });
    assert.deepEqual(opened, { ref: { projectPath: project, sessionId }, empty: true });
  } finally {
    rmSync(userData, { recursive: true, force: true });
    rmSync(projectDirectory, { recursive: true, force: true });
  }
});

test('zero-visible historical rows still use resume_session path', async () => {
  const userData = mkdtempSync(join(tmpdir(), 'lingxi-zero-visible-settings-'));
  const projectDirectory = mkdtempSync(join(tmpdir(), 'lingxi-zero-visible-project-'));
  const project = realpathSync.native(projectDirectory);
  const sessionId = '89898989-9999-4aaa-8bbb-cccccccccccc';
  const settings = new SettingsStore(userData);
  settings.addProject(project);
  let emptyMode: boolean | undefined;
  const host = new HostController(
    settings,
    {
      get: () => undefined,
      openSession: async (_ref: unknown, empty?: boolean) => { emptyMode = empty; },
    } as any,
    new DiagnosticBuffer(),
    {
      find: async () => ({
        uuid: sessionId,
        title: sessionId.slice(0, 8),
        modified_rfc3339: '',
        message_count: 0,
        path: 'system-only.jsonl',
        empty_session: false,
      }),
    } as any,
  );
  try {
    await host.openSessionAndActivate({ projectPath: project, sessionId });
    assert.equal(emptyMode, false);
  } finally {
    rmSync(userData, { recursive: true, force: true });
    rmSync(projectDirectory, { recursive: true, force: true });
  }
});

test('opening an already-connected runtime does not require a persisted catalog row', async () => {
  const userData = mkdtempSync(join(tmpdir(), 'lingxi-connected-session-settings-'));
  const projectDirectory = mkdtempSync(join(tmpdir(), 'lingxi-connected-session-project-'));
  const project = realpathSync.native(projectDirectory);
  const sessionId = '99999999-aaaa-4bbb-8ccc-dddddddddddd';
  const settings = new SettingsStore(userData);
  settings.addProject(project);
  let catalogLookups = 0;
  let opened = false;
  const host = new HostController(
    settings,
    {
      get: () => ({ projectPath: project, connectionState: { status: 'connected' } }),
      openSession: async () => { opened = true; },
    } as any,
    new DiagnosticBuffer(),
    {
      list: async () => ({ sessions: [] }),
      find: async () => { catalogLookups += 1; return undefined; },
    } as any,
  );
  try {
    await host.openSessionAndActivate({ projectPath: project, sessionId });
    assert.equal(opened, true);
    assert.equal(catalogLookups, 0);
    assert.deepEqual(settings.getPublic().activeSession, { projectPath: project, sessionId });
    assert.equal((host as any).bootstrap().workspace.trusted, true);
  } finally {
    rmSync(userData, { recursive: true, force: true });
    rmSync(projectDirectory, { recursive: true, force: true });
  }
});

test('bootstrap surfaces an explicit recovery state when the persisted workspace is missing', () => {
  const diagnostics = new DiagnosticBuffer();
  const workspace = '/missing/workspace';
  const missing = Object.assign(new Error('workspace path is not a directory'), { code: 'ENOENT' });
  const settings = {
    getWorkspace: () => workspace,
    getTrust: () => { throw missing; },
    getPublic: () => ({ version: 1, activeProject: workspace, projects: [workspace], pinnedSessions: [] }),
    credentialMetadata: () => ({ configured: false, encryptionAvailable: true }),
  };
  const bridge = {
    connectionState: { status: 'idle' as const },
    runtimeVersions: {
      serverName: 'lingxi-bridge-server/0.9.0',
      serverProtocol: '0.2.0',
      clientProtocol: '1.0.0',
    },
    turnActive: false,
    restart: async () => undefined,
  };
  const host = new HostController(settings as any, bridge as any, diagnostics);

  const bootstrap = (host as any).bootstrap();
  const report = JSON.parse((host as any).diagnosticReport());
  const nextBootstrap = (host as any).bootstrap();

  assert.equal(nextBootstrap.revision, bootstrap.revision + 1);

  assert.deepEqual(bootstrap.workspace, {
    path: workspace,
    trusted: false,
    recovery: {
      state: 'missing',
      message: `stored project is unavailable: ${workspace}`,
    },
  });
  assert.deepEqual(report.workspace.recovery, {
    state: 'missing',
    message: `stored project is unavailable: ${workspace}`,
  });
  assert.deepEqual(report.bridgeRuntime, bridge.runtimeVersions);
  assert.match(diagnostics.snapshot()[0]?.message ?? '', /workspace path is not a directory/);
});

test('bootstrap treats a credential already supplied to the running engine as configured', () => {
  const diagnostics = new DiagnosticBuffer();
  const settings = {
    getWorkspace: () => '/workspace',
    getTrust: () => ({ trusted: true, fingerprint: 'fingerprint' }),
    getPublic: () => ({ version: 1, activeProject: '/workspace', projects: ['/workspace'], pinnedSessions: [] }),
    credentialMetadata: () => ({ configured: false, encryptionAvailable: false }),
    providerCredentialMetadataFor: (providerIds: readonly string[]) => providerIds.map((providerId) => ({
      providerId,
      configured: false,
      encryptionAvailable: false,
    })),
  };
  const bridge = {
    connectionState: { status: 'connected' as const },
    activeCredentialProviderIds: ['deepseek'],
    turnActive: false,
  };
  const host = new HostController(settings as any, bridge as any, diagnostics);

  const bootstrap = (host as any).bootstrap();
  const deepseek = bootstrap.providerCredentials.find((entry: { providerId: string }) => entry.providerId === 'deepseek');

  assert.deepEqual(deepseek, {
    providerId: 'deepseek',
    configured: true,
    encryptionAvailable: false,
    runtimeOnly: true,
  });
});

test('bootstrap does not treat launch credentials as configured when the engine failed to start', () => {
  const diagnostics = new DiagnosticBuffer();
  const settings = {
    getWorkspace: () => '/workspace',
    getTrust: () => ({ trusted: true, fingerprint: 'fingerprint' }),
    getPublic: () => ({ version: 1, activeProject: '/workspace', projects: ['/workspace'], pinnedSessions: [] }),
    credentialMetadata: () => ({ configured: false, encryptionAvailable: false }),
    providerCredentialMetadataFor: (providerIds: readonly string[]) => providerIds.map((providerId) => ({
      providerId,
      configured: false,
      encryptionAvailable: false,
    })),
  };
  const bridge = {
    connectionState: { status: 'error' as const, message: 'bridge-server binary not found' },
    activeCredentialProviderIds: ['deepseek'],
    turnActive: false,
  };
  const host = new HostController(settings as any, bridge as any, diagnostics);

  const bootstrap = (host as any).bootstrap();
  const deepseek = bootstrap.providerCredentials.find((entry: { providerId: string }) => entry.providerId === 'deepseek');

  assert.deepEqual(deepseek, {
    providerId: 'deepseek',
    configured: false,
    encryptionAvailable: false,
  });
});

test('bootstrap reports CLI/TUI credentials discovered by the shared engine store as persisted', () => {
  const diagnostics = new DiagnosticBuffer();
  const settings = {
    getWorkspace: () => '/workspace',
    getTrust: () => ({ trusted: true, fingerprint: 'fingerprint' }),
    getPublic: () => ({ version: 1, activeProject: '/workspace', projects: ['/workspace'], pinnedSessions: [] }),
    credentialMetadata: () => ({ configured: false, encryptionAvailable: false }),
    providerCredentialMetadataFor: (providerIds: readonly string[]) => providerIds.map((providerId) => ({
      providerId,
      configured: false,
      encryptionAvailable: false,
    })),
  };
  const bridge = {
    connectionState: { status: 'connected' as const },
    activeCredentialProviderIds: ['deepseek'],
    persistedCredentialProviderIds: ['deepseek'],
    providerCredentialStorageEncrypted: true,
    turnActive: false,
  };
  const host = new HostController(settings as any, bridge as any, diagnostics);

  const bootstrap = (host as any).bootstrap();
  const deepseek = bootstrap.providerCredentials.find((entry: { providerId: string }) => entry.providerId === 'deepseek');

  assert.deepEqual(deepseek, {
    providerId: 'deepseek',
    configured: true,
    encryptionAvailable: true,
  });
});

test('bootstrap replays pending AskUserQuestion requests after a renderer reload', () => {
  const diagnostics = new DiagnosticBuffer();
  const settings = {
    getWorkspace: () => '/workspace',
    getTrust: () => ({ trusted: true, fingerprint: 'fingerprint' }),
    getPublic: () => ({ version: 1, activeProject: '/workspace', projects: ['/workspace'], pinnedSessions: [] }),
    credentialMetadata: () => ({ configured: false, encryptionAvailable: false }),
    providerCredentialMetadataFor: () => [],
  };
  const bridge = {
    connectionState: { status: 'connected' as const },
    pendingAskUserQuestions: [{
      request_id: 7,
      questions: [{
        question: 'Choose a mode',
        header: 'Mode',
        options: [{ label: 'Safe', description: 'Keep safeguards enabled' }],
        multi_select: false,
      }],
      timeout_secs: 60,
    }],
    turnActive: false,
  };
  const host = new HostController(settings as any, bridge as any, diagnostics);

  const bootstrap = (host as any).bootstrap();

  assert.deepEqual(bootstrap.pendingAskUserQuestions, bridge.pendingAskUserQuestions);
});

test('project session catalogs are patched independently and preserved in bootstrap', async () => {
  const projectA = '/projects/a';
  const projectB = '/projects/b';
  const row = (uuid: string, title: string) => ({
    uuid,
    title,
    modified_rfc3339: '2026-08-26T00:00:00Z',
    message_count: 1,
    path: `/sessions/${uuid}.jsonl`,
    empty_session: false,
  });
  const settings = {
    getWorkspace: () => projectA,
    getTrust: () => ({ trusted: true, fingerprint: 'fingerprint' }),
    getPublic: () => ({ version: 1, activeProject: projectA, projects: [projectA, projectB], pinnedSessions: [] }),
    credentialMetadata: () => ({ configured: false, encryptionAvailable: false }),
    providerCredentialMetadataFor: () => [],
  };
  const calls: string[] = [];
  const catalog = {
    list: async (projectPath: string) => {
      calls.push(projectPath);
      return { sessions: [row(projectPath === projectA ? 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa' : 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb', projectPath)] };
    },
  };
  const host = new HostController(settings as any, { turnActive: false } as any, new DiagnosticBuffer(), catalog as any);

  await (host as any).loadProjectSessions(projectA);
  await (host as any).loadProjectSessions(projectB);

  const bootstrap = (host as any).bootstrap();
  assert.deepEqual(calls, [projectA, projectB]);
  assert.equal(bootstrap.projectCatalogs[projectA].sessions[0].title, projectA);
  assert.equal(bootstrap.projectCatalogs[projectB].sessions[0].title, projectB);
  assert.equal('empty_session' in bootstrap.projectCatalogs[projectA].sessions[0], false);
});

test('host catalog generations keep only the newest deferred response', async () => {
  const projectPath = '/projects/race';
  const first = deferred<{ sessions: Array<Record<string, unknown>> }>();
  const second = deferred<{ sessions: Array<Record<string, unknown>> }>();
  let calls = 0;
  const catalog = {
    list: async () => (++calls === 1 ? first.promise : second.promise),
  };
  const settings = {
    getWorkspace: () => projectPath,
    getTrust: () => ({ trusted: true, fingerprint: 'fingerprint' }),
    getPublic: () => ({ version: 1, activeProject: projectPath, projects: [projectPath], pinnedSessions: [] }),
    credentialMetadata: () => ({ configured: false, encryptionAvailable: false }),
    providerCredentialMetadataFor: () => [],
  };
  const host = new HostController(settings as any, { turnActive: false } as any, new DiagnosticBuffer(), catalog as any);
  const row = (title: string) => ({
    uuid: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
    title,
    modified_rfc3339: '2026-08-26T00:00:00Z',
    message_count: 1,
    path: '/session.jsonl',
    empty_session: false,
  });

  const firstLoad = (host as any).loadProjectSessions(projectPath);
  const secondLoad = (host as any).loadProjectSessions(projectPath);
  second.resolve({ sessions: [row('new')] });
  await secondLoad;
  first.resolve({ sessions: [row('old')] });
  await firstLoad;

  assert.equal((host as any).bootstrap().projectCatalogs[projectPath].sessions[0].title, 'new');
});
