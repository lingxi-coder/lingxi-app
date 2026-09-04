import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import {
  CH_BRIDGE_RESTART,
  CH_PROVIDER_CREDENTIAL_CLEAR,
  CH_PROVIDER_CREDENTIAL_SET,
  CH_PROVIDER_CREDENTIALS_GET,
  CH_SESSION_CLEAR,
  CH_SETTINGS_UPDATE,
  CH_WORKSPACE_FILE_PREVIEW,
  HostController,
  readWorkspaceFilePreview,
} from '../src/main/host';
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

test('workspace preview is bounded UTF-8 and rejects traversal, binary content, and symlinks', () => {
  const projectDirectory = mkdtempSync(join(tmpdir(), 'lingxi-preview-project-'));
  const outsideDirectory = mkdtempSync(join(tmpdir(), 'lingxi-preview-outside-'));
  const project = realpathSync.native(projectDirectory);
  try {
    mkdirSync(join(project, 'src'));
    writeFileSync(join(project, 'src', 'app.ts'), 'export const answer = 42;\n');
    assert.deepEqual(readWorkspaceFilePreview(project, 'src/app.ts'), {
      kind: 'text',
      path: 'src/app.ts',
      size: 26,
      content: 'export const answer = 42;\n',
      truncated: false,
    });

    writeFileSync(join(project, 'invalid.bin'), Buffer.from([0x61, 0xff, 0x62]));
    assert.equal(readWorkspaceFilePreview(project, 'invalid.bin').kind, 'binary');
    writeFileSync(join(project, 'controls.bin'), Buffer.from([0, 1, 2, 3, 4, 5]));
    assert.equal(readWorkspaceFilePreview(project, 'controls.bin').kind, 'binary');

    const cap = 512 * 1024;
    writeFileSync(join(project, 'large.txt'), Buffer.concat([
      Buffer.alloc(cap - 1, 'a'),
      Buffer.from('😀tail'),
    ]));
    const large = readWorkspaceFilePreview(project, 'large.txt');
    assert.equal(large.kind, 'text');
    assert.equal(large.truncated, true);
    assert.equal(large.content?.length, cap - 1);

    writeFileSync(join(outsideDirectory, 'secret.txt'), 'outside');
    if (process.platform !== 'win32') {
      symlinkSync(join(outsideDirectory, 'secret.txt'), join(project, 'linked.txt'));
      symlinkSync(outsideDirectory, join(project, 'linked-dir'));
      assert.throws(() => readWorkspaceFilePreview(project, 'linked.txt'), /symlink/);
      assert.throws(() => readWorkspaceFilePreview(project, 'linked-dir/secret.txt'), /symlink/);
    }
    assert.throws(() => readWorkspaceFilePreview(project, '../secret.txt'), /traversal|outside/);
    assert.throws(() => readWorkspaceFilePreview(project, join(outsideDirectory, 'secret.txt')), /relative/);
    assert.throws(() => readWorkspaceFilePreview(project, 'bad\0path'), /invalid/);
  } finally {
    rmSync(projectDirectory, { recursive: true, force: true });
    rmSync(outsideDirectory, { recursive: true, force: true });
  }
});

test('workspace preview IPC is fenced to the exact active session and its project', async () => {
  const userData = mkdtempSync(join(tmpdir(), 'lingxi-preview-ipc-settings-'));
  const projectDirectory = mkdtempSync(join(tmpdir(), 'lingxi-preview-ipc-project-'));
  const project = realpathSync.native(projectDirectory);
  const sessionId = '11111111-2222-4333-8444-555555555555';
  const otherSessionId = '22222222-3333-4444-8555-666666666666';
  writeFileSync(join(project, 'readme.txt'), 'hello');
  const settings = new SettingsStore(userData);
  settings.addProject(project);
  settings.activateProject(project);
  settings.setActiveSession({ projectPath: project, sessionId: otherSessionId });
  const handlers = new Map<string, (...args: unknown[]) => unknown>();
  const ipc = {
    handle: (channel: string, handler: (...args: unknown[]) => unknown) => { handlers.set(channel, handler); },
    removeHandler: (channel: string) => { handlers.delete(channel); },
  };
  const bridge = {
    registerIpc: () => undefined,
    registerWindow: () => undefined,
    get: (requested: string) => requested === sessionId
      ? { projectPath: project, connectionState: { status: 'connected' as const } }
      : undefined,
  };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), undefined, ipc as any);
  const frame = { url: 'http://127.0.0.1:4242' };
  const sender = { mainFrame: frame, isDestroyed: () => false, once: () => undefined, removeListener: () => undefined };
  host.registerWindow(sender as any, frame.url);
  host.registerIpc();
  const preview = handlers.get(CH_WORKSPACE_FILE_PREVIEW);
  assert.ok(preview);
  const event = { sender, senderFrame: frame };

  try {
    await assert.rejects(() => Promise.resolve(preview!(event, sessionId, 'readme.txt')), /not active/);
    settings.setActiveSession({ projectPath: project, sessionId });
    assert.equal((await preview!(event, sessionId, 'readme.txt') as { content?: string }).content, 'hello');
  } finally {
    host.dispose();
    rmSync(userData, { recursive: true, force: true });
    rmSync(projectDirectory, { recursive: true, force: true });
  }
});

test('bridge restart IPC re-checks session ownership and active work at execution time', async () => {
  const userData = mkdtempSync(join(tmpdir(), 'lingxi-restart-ipc-settings-'));
  const projectDirectory = mkdtempSync(join(tmpdir(), 'lingxi-restart-ipc-project-'));
  const project = realpathSync.native(projectDirectory);
  const sessionId = '11111111-2222-4333-8444-555555555555';
  const otherSessionId = '22222222-3333-4444-8555-666666666666';
  const settings = new SettingsStore(userData);
  settings.addProject(project);
  settings.activateProject(project);
  settings.setActiveSession({ projectPath: project, sessionId });

  const handlers = new Map<string, (...args: unknown[]) => unknown>();
  const ipc = {
    handle: (channel: string, handler: (...args: unknown[]) => unknown) => { handlers.set(channel, handler); },
    removeHandler: (channel: string) => { handlers.delete(channel); },
  };
  const runtime = {
    projectPath: project,
    connectionState: { status: 'connected' as const },
    turnActive: false,
    pendingInteractions: 0,
  };
  let runtimeOpen = true;
  let restartCalls = 0;
  const bridge = {
    registerIpc: () => undefined,
    registerWindow: () => undefined,
    get: (requestedSessionId: string) => runtimeOpen && requestedSessionId === sessionId ? runtime : undefined,
    hasActiveWork: () => runtime.turnActive || runtime.pendingInteractions > 0,
    restart: async (_ref: unknown, beforeRestart?: () => void) => {
      // Simulate work arriving after the handler's first check but before the
      // manager's queued restart starts.
      runtime.turnActive = true;
      beforeRestart?.();
      restartCalls += 1;
    },
  };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), undefined, ipc as any);
  const frame = { url: 'http://127.0.0.1:4242' };
  const sender = {
    mainFrame: frame,
    isDestroyed: () => false,
    once: () => undefined,
    removeListener: () => undefined,
  };
  host.registerWindow(sender as any, frame.url);
  host.registerIpc();
  const restart = handlers.get(CH_BRIDGE_RESTART);
  assert.ok(restart);
  const event = { sender, senderFrame: frame };

  try {
    await assert.rejects(
      () => Promise.resolve(restart!(event, sessionId)),
      /cancel active turns and pending interactions/,
    );
    assert.equal(restartCalls, 0);

    runtime.turnActive = false;
    settings.setActiveSession({ projectPath: project, sessionId: otherSessionId });
    await assert.rejects(
      () => Promise.resolve(restart!(event, sessionId)),
      /no longer active/,
    );
    assert.equal(restartCalls, 0);

    settings.setActiveSession({ projectPath: project, sessionId });
    runtimeOpen = false;
    await assert.rejects(
      () => Promise.resolve(restart!(event, sessionId)),
      /session runtime is not open/,
    );
    assert.equal(restartCalls, 0);
  } finally {
    host.dispose();
    rmSync(userData, { recursive: true, force: true });
    rmSync(projectDirectory, { recursive: true, force: true });
  }
});

test('session clear uses dedicated IPC and re-checks active work plus pending interactions', async () => {
  const userData = mkdtempSync(join(tmpdir(), 'lingxi-clear-session-settings-'));
  const projectDirectory = mkdtempSync(join(tmpdir(), 'lingxi-clear-session-project-'));
  const project = realpathSync.native(projectDirectory);
  const sessionId = '11111111-2222-4333-8444-555555555555';
  const replacementId = 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee';
  const settings = new SettingsStore(userData);
  settings.addProject(project);
  settings.activateProject(project);
  settings.setActiveSession({ projectPath: project, sessionId });
  const handlers = new Map<string, (...args: unknown[]) => unknown>();
  const ipc = {
    handle: (channel: string, handler: (...args: unknown[]) => unknown) => { handlers.set(channel, handler); },
    removeHandler: (channel: string) => { handlers.delete(channel); },
  };
  const runtime = { pendingInteractions: 0, pendingAskUserQuestions: [] as unknown[] };
  let activeWork = true;
  let closeCalls = 0;
  let newCalls = 0;
  const bridge = {
    registerIpc: () => undefined,
    registerWindow: () => undefined,
    get: (requestedSessionId: string) => requestedSessionId === sessionId ? runtime : undefined,
    hasActiveWork: (projectPath: string) => projectPath === project && activeWork,
    closeSession: async () => { closeCalls += 1; },
    newSession: async () => {
      newCalls += 1;
      return { projectPath: project, sessionId: replacementId };
    },
  };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), undefined, ipc as any);
  const frame = { url: 'http://127.0.0.1:4242' };
  const sender = { mainFrame: frame, isDestroyed: () => false, once: () => undefined, removeListener: () => undefined };
  host.registerWindow(sender as any, frame.url);
  host.registerIpc();
  const clear = handlers.get(CH_SESSION_CLEAR);
  assert.ok(clear);
  const event = { sender, senderFrame: frame };

  try {
    await assert.rejects(() => Promise.resolve(clear!(event, sessionId)), /cancel the active turn/);
    activeWork = false;
    runtime.pendingInteractions = 1;
    await assert.rejects(() => Promise.resolve(clear!(event, sessionId)), /resolve pending interactions/);
    runtime.pendingInteractions = 0;
    runtime.pendingAskUserQuestions = [{}];
    await assert.rejects(() => Promise.resolve(clear!(event, sessionId)), /resolve pending interactions/);
    runtime.pendingAskUserQuestions = [];
    await clear!(event, sessionId);

    assert.equal(closeCalls, 1);
    assert.equal(newCalls, 1);
    assert.deepEqual(settings.getPublic().activeSession, { projectPath: project, sessionId: replacementId });
  } finally {
    host.dispose();
    rmSync(userData, { recursive: true, force: true });
    rmSync(projectDirectory, { recursive: true, force: true });
  }
});

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
    assert.equal(JSON.parse(readFileSync(settings.settingsPath, 'utf8')).activeSession, undefined);
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

test('history-less startup keeps the host-created session volatile until first prompt', async () => {
  const userData = mkdtempSync(join(tmpdir(), 'lingxi-restore-draft-settings-'));
  const projectDirectory = mkdtempSync(join(tmpdir(), 'lingxi-restore-draft-project-'));
  const project = realpathSync.native(projectDirectory);
  const sessionId = '34343434-4444-4555-8666-777777777777';
  const settings = new SettingsStore(userData);
  settings.addProject(project);
  const bridge = {
    newSession: async () => ({ projectPath: project, sessionId }),
  };
  const catalog = {
    list: async () => ({ sessions: [] }),
  };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), catalog as any);
  try {
    await host.restoreProjectSession(project);
    assert.deepEqual(settings.getPublic().activeSession, { projectPath: project, sessionId });
    assert.equal(JSON.parse(readFileSync(settings.settingsPath, 'utf8')).activeSession, undefined);
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
  let opened: {
    ref: { projectPath: string; sessionId: string };
    empty?: boolean;
    resumeModel?: string;
  } | undefined;
  const host = new HostController(
    settings,
    {
      get: () => undefined,
      openSession: async (
        ref: { projectPath: string; sessionId: string },
        empty?: boolean,
        resumeModel?: string,
      ) => { opened = { ref, empty, resumeModel }; },
    } as any,
    new DiagnosticBuffer(),
    {
      list: async () => ({ sessions: [] }),
      find: async (_projectPath: string, requestedId: string) => requestedId === sessionId
        ? {
          uuid: sessionId,
          title: 'Old session',
          modified_rfc3339: '',
          message_count: 1,
          path: 'old.jsonl',
          resume_model: 'openrouter/cohere/north-mini-code:free',
        }
        : undefined,
    } as any,
  );
  try {
    await host.openSessionAndActivate({ projectPath: project, sessionId });
    assert.deepEqual(opened, {
      ref: { projectPath: project, sessionId },
      empty: false,
      resumeModel: 'openrouter/cohere/north-mini-code:free',
    });
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
    assert.equal((await (host as any).bootstrap()).workspace.trusted, true);
  } finally {
    rmSync(userData, { recursive: true, force: true });
    rmSync(projectDirectory, { recursive: true, force: true });
  }
});

test('bootstrap surfaces an explicit recovery state when the persisted workspace is missing', async () => {
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

  const bootstrap = await (host as any).bootstrap();
  const report = JSON.parse((host as any).diagnosticReport());
  const nextBootstrap = await (host as any).bootstrap();

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

test('bootstrap treats a credential already supplied to the running engine as configured', async () => {
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

  const bootstrap = await (host as any).bootstrap();
  const deepseek = bootstrap.providerCredentials.find((entry: { providerId: string }) => entry.providerId === 'deepseek');

  assert.deepEqual(deepseek, {
    providerId: 'deepseek',
    configured: true,
    encryptionAvailable: false,
    runtimeOnly: true,
  });
});

test('bootstrap does not treat launch credentials as configured when the engine failed to start', async () => {
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

  const bootstrap = await (host as any).bootstrap();
  const deepseek = bootstrap.providerCredentials.find((entry: { providerId: string }) => entry.providerId === 'deepseek');

  assert.deepEqual(deepseek, {
    providerId: 'deepseek',
    configured: false,
    encryptionAvailable: false,
  });
});

test('bootstrap reports CLI/TUI credentials discovered by the shared engine store as persisted', async () => {
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
    providerCredentialPreviews: { deepseek: '••••abcd' },
    turnActive: false,
  };
  const host = new HostController(settings as any, bridge as any, diagnostics);

  const bootstrap = await (host as any).bootstrap();
  const deepseek = bootstrap.providerCredentials.find((entry: { providerId: string }) => entry.providerId === 'deepseek');

  assert.deepEqual(deepseek, {
    providerId: 'deepseek',
    configured: true,
    encryptionAvailable: true,
    credentialPreview: '••••abcd',
  });
});

test('bootstrap replays pending AskUserQuestion requests after a renderer reload', async () => {
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

  const bootstrap = await (host as any).bootstrap();

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

  const bootstrap = await (host as any).bootstrap();
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

  assert.equal((await (host as any).bootstrap()).projectCatalogs[projectPath].sessions[0].title, 'new');
});

test('provider credential IPC uses the broker path, hot-syncs cached sessions, and never returns the full secret', async () => {
  const handlers = new Map<string, (...args: unknown[]) => unknown>();
  const ipc = {
    handle: (channel: string, handler: (...args: unknown[]) => unknown) => { handlers.set(channel, handler); },
    removeHandler: (channel: string) => { handlers.delete(channel); },
  };
  const settings = {
    getWorkspace: () => '/workspace',
    getTrust: () => ({ trusted: true, fingerprint: 'fingerprint' }),
    getPublic: () => ({ version: 1, activeProject: '/workspace', projects: ['/workspace'], pinnedSessions: [] }),
  };
  const operations: string[] = [];
  const bridge = {
    registerIpc: () => undefined,
    registerWindow: () => undefined,
    turnActive: false,
    hasActiveWork: () => false,
    refreshCachedProviderCredential: async (providerId: string, credential: string) => {
      assert.equal(credential, 'sk-replacement-secret');
      operations.push(`runtime:set:${providerId}`);
    },
    clearCachedProviderCredential: async (providerId: string) => {
      operations.push(`runtime:delete:${providerId}`);
    },
  };
  const host = new HostController(
    settings as any,
    bridge as any,
    new DiagnosticBuffer(),
    undefined,
    ipc as any,
    undefined,
    {
      health: async () => ({ protocolVersion: 1, buildVersion: 'test' }),
      listStatus: async () => [{ providerId: 'anthropic', configured: true }],
      preview: async () => ({ providerId: 'anthropic', configured: true, maskedValue: '••••cret' }),
      resolve: async () => 'sk-test-secret',
      set: async (providerId: string) => {
        operations.push(`broker:set:${providerId}`);
        return { providerId, configured: true, maskedValue: '••••cret' };
      },
      delete: async (providerId: string) => { operations.push(`broker:delete:${providerId}`); },
    },
  );
  const frame = { url: 'http://127.0.0.1:4242' };
  const sender = { mainFrame: frame, isDestroyed: () => false, once: () => undefined, removeListener: () => undefined };
  host.registerWindow(sender as any, frame.url);
  host.registerIpc();

  const credentials = handlers.get(CH_PROVIDER_CREDENTIALS_GET);
  assert.ok(credentials);
  const result = await credentials!({ sender, senderFrame: frame }, 'anthropic') as Array<Record<string, unknown>>;
  const anthropic = result.find((entry) => entry['providerId'] === 'anthropic');

  assert.deepEqual(anthropic, {
    providerId: 'anthropic',
    configured: true,
    encryptionAvailable: true,
    credentialPreview: '••••cret',
  });
  assert.equal(JSON.stringify(result).includes('sk-test-secret'), false);

  const setCredential = handlers.get(CH_PROVIDER_CREDENTIAL_SET);
  const clearCredential = handlers.get(CH_PROVIDER_CREDENTIAL_CLEAR);
  assert.ok(setCredential);
  assert.ok(clearCredential);
  const stored = await setCredential!({ sender, senderFrame: frame }, 'anthropic', 'sk-replacement-secret') as Record<string, unknown>;
  assert.equal(JSON.stringify(stored).includes('sk-replacement-secret'), false);
  assert.deepEqual(operations, ['broker:set:anthropic', 'runtime:set:anthropic']);

  await clearCredential!({ sender, senderFrame: frame }, 'anthropic');
  assert.deepEqual(operations, [
    'broker:set:anthropic',
    'runtime:set:anthropic',
    'runtime:delete:anthropic',
    'broker:delete:anthropic',
  ]);
});

test('broker-backed bootstrap ignores legacy runtime credential status', async () => {
  const settings = {
    getWorkspace: () => '/workspace',
    getTrust: () => ({ trusted: true, fingerprint: 'fingerprint' }),
    getPublic: () => ({ version: 1, activeProject: '/workspace', projects: ['/workspace'], pinnedSessions: [] }),
    credentialMetadata: () => ({ configured: false, encryptionAvailable: false }),
    providerCredentialMetadataFor: () => [],
  };
  const bridge = {
    connectionState: { status: 'connected' as const },
    persistedCredentialProviderIds: ['deepseek'],
    providerCredentialStorageEncrypted: true,
    providerCredentialPreviews: { deepseek: '••••legacy' },
    turnActive: false,
  };
  const broker = {
    health: async () => ({ protocolVersion: 1, buildVersion: 'test' }),
    listStatus: async () => [],
    preview: async (providerId: string) => ({ providerId, configured: false }),
    resolve: async () => undefined,
    set: async (providerId: string) => ({ providerId, configured: true, maskedValue: '••••test' }),
    delete: async () => undefined,
  };
  const host = new HostController(
    settings as any,
    bridge as any,
    new DiagnosticBuffer(),
    undefined,
    undefined,
    undefined,
    broker,
  );

  const bootstrap = await (host as any).bootstrap();
  const deepseek = bootstrap.providerCredentials.find((entry: { providerId: string }) => entry.providerId === 'deepseek');
  assert.deepEqual(deepseek, {
    providerId: 'deepseek',
    configured: false,
    encryptionAvailable: true,
  });
});

test('settings model and voice patches persist without restarting a live session', async () => {
  const userData = mkdtempSync(join(tmpdir(), 'lingxi-settings-update-voice-'));
  const settings = new SettingsStore(userData);
  const handlers = new Map<string, (...args: unknown[]) => unknown>();
  const ipc = {
    handle: (channel: string, handler: (...args: unknown[]) => unknown) => { handlers.set(channel, handler); },
    removeHandler: (channel: string) => { handlers.delete(channel); },
  };
  let restartCalls = 0;
  const bridge = {
    registerIpc: () => undefined,
    registerWindow: () => undefined,
    // If a `voice`-only patch ever triggered a restart, this would be
    // called — `main/host.ts`'s own `restartsBridge` check only looks at
    // `'model' in patch || 'apiBaseUrl' in patch`, deliberately excluding
    // `voice` (see its comment: recognition/synthesis read
    // `bootstrap.settings.voice` fresh on every audio request, so a write
    // takes effect on the next request with no restart needed).
    restart: async () => { restartCalls += 1; },
  };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), undefined, ipc as any);
  const frame = { url: 'http://127.0.0.1:4242' };
  const sender = {
    mainFrame: frame,
    isDestroyed: () => false,
    once: () => undefined,
    removeListener: () => undefined,
  };
  host.registerWindow(sender as any, frame.url);
  host.registerIpc();
  const update = handlers.get(CH_SETTINGS_UPDATE);
  assert.ok(update);
  const event = { sender, senderFrame: frame };

  try {
    const modelResult = await Promise.resolve(update!(event, {
      model: 'openrouter/minimax/minimax-m3:free',
    })) as { model?: string | null };
    assert.equal(modelResult.model, 'openrouter/minimax/minimax-m3:free');
    assert.equal(restartCalls, 0, 'persisting a selected model must not restart its session engine');

    const result = await Promise.resolve(update!(event, {
      voice: { schemaVersion: 2, recognitionMode: 'localOnly', language: '  ZH-cn  ', voiceSelection: 'Alex', rate: 99, autoPlayReplies: true },
    })) as { voice?: Record<string, unknown> };

    // Normalized through the REAL `parseVoicePreferences` (Task 4), not
    // echoed back raw: language is trimmed (case preserved — only an
    // "auto"-insensitive match is special-cased), `rate` is clamped into
    // [0.5, 2.0], and the bare voice name gets its `system:` prefix.
    assert.deepEqual(result.voice, {
      schemaVersion: 2,
      recognitionMode: 'localOnly',
      language: 'ZH-cn',
      voiceSelection: 'system:Alex',
      rate: 2.0,
      autoPlayReplies: true,
    });
    assert.deepEqual(settings.getPublic().voice, result.voice, 'the IPC response must reflect what was actually persisted, not an optimistic echo');
    assert.equal(restartCalls, 0, 'a voice-only patch must never restart the bridge');

    // The allowlist genuinely rejects anything else — `voice` joining it
    // must not have accidentally opened the gate to arbitrary keys.
    await assert.rejects(
      () => Promise.resolve(update!(event, { notARealSetting: true })),
      /unsupported setting/,
    );
  } finally {
    host.dispose();
    rmSync(userData, { recursive: true, force: true });
  }
});
