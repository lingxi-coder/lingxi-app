import { test, type TestContext } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, renameSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { CH_PROJECT_REMOVE, HostController, type BootstrapState } from '../src/main/host';
import { DiagnosticBuffer } from '../src/main/host-utils';
import { ScheduledTaskService } from '../src/main/scheduled';
import { ProjectSessionCatalog } from '../src/main/session-catalog';
import { SettingsStore } from '../src/main/settings';

function removalFixture(t: TestContext) {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), 'lingxi-project-removal-')));
  const userData = join(root, 'settings');
  const settings = new SettingsStore(userData);
  const handlers = new Map<string, (...args: unknown[]) => unknown>();
  const ipc = {
    handle: (channel: string, handler: (...args: unknown[]) => unknown) => { handlers.set(channel, handler); },
    removeHandler: (channel: string) => { handlers.delete(channel); },
  };
  const calls = { activeWork: [] as string[], closeProject: [] as string[], engineStarts: 0 };
  const activeProjects = new Set<string>();
  const bridge = {
    registerIpc: () => undefined,
    registerWindow: () => undefined,
    isProjectClosing: () => false,
    get: () => undefined,
    runtimeSummaries: [],
    hasActiveWork: (path: string) => {
      calls.activeWork.push(path);
      return activeProjects.has(path);
    },
    closeProject: async (path: string) => { calls.closeProject.push(path); },
    retainBackgroundSession: () => {
      calls.engineStarts += 1;
      throw new Error('removing an empty or shared project scope must not start an engine');
    },
  };
  const catalog = new ProjectSessionCatalog();
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), catalog, ipc as any);
  const scheduledErrors: unknown[] = [];
  host.attachScheduled(new ScheduledTaskService(settings, bridge as any, catalog, () => false, (error) => scheduledErrors.push(error)));
  const frame = { url: 'http://127.0.0.1:4242' };
  const sender = { mainFrame: frame, isDestroyed: () => false, once: () => undefined, removeListener: () => undefined };
  host.registerWindow(sender as any, frame.url);
  host.registerIpc();
  const remove = handlers.get(CH_PROJECT_REMOVE);
  assert.ok(remove);
  t.after(() => {
    host.dispose();
    rmSync(root, { recursive: true, force: true });
  });

  return {
    root,
    userData,
    settings,
    calls,
    activeProjects,
    scheduledErrors,
    remove: (path: unknown) => Promise.resolve(remove({ sender, senderFrame: frame }, path)) as Promise<BootstrapState>,
    project: (name: string) => {
      const path = join(root, name);
      mkdirSync(path);
      writeFileSync(join(path, 'keep.txt'), name);
      settings.addProject(path);
      return path;
    },
  };
}

for (const targetRegistered of [false, true]) {
  test(`project removal uses the stored path after it becomes a symlink (${targetRegistered ? 'registered' : 'unregistered'} target)`, { skip: process.platform === 'win32' }, async (t) => {
    const fixture = removalFixture(t);
    const project = fixture.project('project');
    const unrelated = fixture.project('unrelated');
    const target = join(fixture.root, 'moved-project');
    renameSync(project, target);
    symlinkSync(target, project, 'dir');
    if (targetRegistered) fixture.settings.addProject(target);
    const remaining = fixture.settings.getPublic().projects.filter((path) => path !== project);
    fixture.settings.activateProject(project);
    fixture.settings.setActiveSession({ projectPath: project, sessionId: '11111111-2222-4333-8444-555555555555' });
    mkdirSync(join(target, '.lingxi'));
    const scheduledCatalog = join(target, '.lingxi', 'scheduled_tasks.json');
    const scheduledContents = JSON.stringify({ tasks: targetRegistered ? [{
      id: 'keep-active', cron: '0 9 * * *', prompt: 'Summarize the project.',
      recurring: true, durable: true, permanent: false, created_at: 1,
      automation: { version: 2, status: 'active', model: 'openai/test-model', runMode: 'new_session', notificationPolicy: 'all' },
    }] : [] });
    writeFileSync(scheduledCatalog, scheduledContents);

    const snapshot = await fixture.remove(project);

    assert.deepEqual(snapshot.settings.projects, remaining);
    assert.equal(snapshot.activeSession, undefined);
    assert.notEqual(snapshot.settings.activeProject, project);
    assert.deepEqual(new SettingsStore(fixture.userData).getPublic().projects, remaining);
    assert.deepEqual(fixture.calls.closeProject, [project]);
    assert.ok(fixture.calls.activeWork.every((path) => path === project));
    assert.equal(fixture.calls.engineStarts, 0);
    assert.deepEqual(fixture.scheduledErrors, []);
    assert.equal(lstatSync(project).isSymbolicLink(), true);
    assert.equal(readFileSync(join(target, 'keep.txt'), 'utf8'), 'project');
    assert.equal(readFileSync(join(unrelated, 'keep.txt'), 'utf8'), 'unrelated');
    assert.equal(readFileSync(scheduledCatalog, 'utf8'), scheduledContents);
  });
}

for (const state of ['moved', 'missing', 'broken symlink'] as const) {
  test(`project removal accepts a stored ${state} path without recreating or deleting files`, { skip: state === 'broken symlink' && process.platform === 'win32' }, async (t) => {
    const fixture = removalFixture(t);
    const project = fixture.project('project');
    const unrelated = fixture.project('unrelated');
    const moved = join(fixture.root, 'moved-project');
    if (state === 'moved') renameSync(project, moved);
    else {
      rmSync(project, { recursive: true });
      if (state === 'broken symlink') symlinkSync(moved, project, 'dir');
    }

    const snapshot = await fixture.remove(project);

    assert.deepEqual(snapshot.settings.projects, [unrelated]);
    assert.deepEqual(new SettingsStore(fixture.userData).getPublic().projects, [unrelated]);
    assert.deepEqual(fixture.calls.closeProject, [project]);
    assert.equal(fixture.calls.engineStarts, 0);
    assert.deepEqual(fixture.scheduledErrors, []);
    assert.equal(existsSync(project), false);
    if (state === 'broken symlink') assert.equal(lstatSync(project).isSymbolicLink(), true);
    if (state === 'moved') assert.equal(readFileSync(join(moved, 'keep.txt'), 'utf8'), 'project');
    assert.equal(readFileSync(join(unrelated, 'keep.txt'), 'utf8'), 'unrelated');
  });
}

test('project removal rejects unregistered paths and the managed scheduled workspace before touching the bridge', async (t) => {
  const fixture = removalFixture(t);
  const project = fixture.project('project');
  const unregistered = join(fixture.root, 'unregistered');
  mkdirSync(unregistered);
  writeFileSync(join(fixture.settings.scheduledWorkspace, 'keep.txt'), 'scheduled');
  const inputs: unknown[] = [unregistered, join(fixture.root, 'missing'), fixture.settings.scheduledWorkspace, '', null, {}];
  if (process.platform !== 'win32') {
    const alias = join(fixture.root, 'unregistered-alias');
    symlinkSync(project, alias, 'dir');
    inputs.push(alias);
  }

  for (const path of inputs) {
    await assert.rejects(() => fixture.remove(path), /invalid project path|project is not in the project list/);
    assert.deepEqual(fixture.calls.activeWork, []);
    assert.deepEqual(fixture.calls.closeProject, []);
    assert.deepEqual(fixture.settings.getPublic().projects, [project]);
  }
  assert.equal(fixture.calls.engineStarts, 0);
  assert.equal(readFileSync(join(project, 'keep.txt'), 'utf8'), 'project');
  assert.equal(readFileSync(join(fixture.settings.scheduledWorkspace, 'keep.txt'), 'utf8'), 'scheduled');
});

test('project removal still refuses active work and preserves the saved project', async (t) => {
  const fixture = removalFixture(t);
  const project = fixture.project('project');
  fixture.activeProjects.add(project);

  await assert.rejects(() => fixture.remove(project), /cancel active turns and pending interactions/);

  assert.deepEqual(fixture.calls.activeWork, [project]);
  assert.deepEqual(fixture.calls.closeProject, []);
  assert.deepEqual(fixture.settings.getPublic().projects, [project]);
  assert.deepEqual(new SettingsStore(fixture.userData).getPublic().projects, [project]);
  assert.equal(fixture.calls.engineStarts, 0);
  assert.equal(readFileSync(join(project, 'keep.txt'), 'utf8'), 'project');
});
