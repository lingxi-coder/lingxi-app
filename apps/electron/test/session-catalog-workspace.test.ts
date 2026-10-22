import assert from 'node:assert/strict';
import { test } from 'node:test';
import { EventEmitter } from 'node:events';
import { mkdtempSync, realpathSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { ProjectSessionCatalog } from '../src/main/session-catalog.js';
import { HostController } from '../src/main/host.js';
import { SettingsStore } from '../src/main/settings.js';
import { DiagnosticBuffer } from '../src/main/host-utils.js';

const id = '11111111-2222-4333-8444-555555555555';
const row = { uuid: id, title: 'Session', modified_rfc3339: '', message_count: 1,
  path: '/store/session.jsonl', workspace_path: '/source', empty_session: false };
function child() {
  const process = new EventEmitter() as EventEmitter & { stdout: EventEmitter; stderr: EventEmitter; kill(): boolean };
  process.stdout = new EventEmitter();
  process.stderr = new EventEmitter();
  process.kill = () => true;
  return { process, finish(value: unknown) {
    process.stdout.emit('data', Buffer.from(JSON.stringify(value)));
    process.emit('close', 0, null);
  } };
}

test('catalog scopes have separate caches and an invalidated pending read cannot repopulate them', async (t) => {
  const project = mkdtempSync(join(tmpdir(), 'catalog-scopes-'));
  t.after(() => rmSync(project, { recursive: true, force: true }));
  const requests: { args: string[]; finish(value: unknown): void }[] = [];
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: ((_bin, args) => {
    const spawned = child();
    requests.push({ args: [...args as string[]], finish: spawned.finish });
    return spawned.process;
  }) as any });
  const current = catalog.list(project);
  const all = catalog.list(project, 'all');
  assert.equal(requests.length, 2);
  assert.equal(requests[0].args.at(-1), 'current');
  assert.equal(requests[1].args.at(-1), 'all');
  requests[1].finish({ version: 1, sessions: [row] });
  assert.equal((await all).sessions[0].workspace_path, '/source');
  catalog.invalidate(project);
  requests[0].finish({ version: 1, sessions: [] });
  await current;
  const refreshed = catalog.list(project);
  assert.equal(requests.length, 3);
  requests[2].finish({ version: 1, sessions: [row] });
  await refreshed;
  assert.equal((await catalog.list(project)).sessions.length, 1);
  assert.equal(requests.length, 3);
});

test('cached current absence still performs exact global UUID resolution', async (t) => {
  const project = mkdtempSync(join(tmpdir(), 'catalog-exact-global-'));
  t.after(() => rmSync(project, { recursive: true, force: true }));
  const calls: string[][] = [];
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: ((_bin, args) => {
    calls.push(args as string[]);
    const spawned = child();
    queueMicrotask(() => spawned.finish({ version: 1, sessions: calls.length === 1 ? [] : [row] }));
    return spawned.process;
  }) as any });
  assert.deepEqual(await catalog.list(project), { sessions: [] });
  assert.equal((await catalog.find(project, id))?.workspace_path, '/source');
  assert.equal(calls[1].at(-2), '--session-id');
  assert.equal(calls[1].at(-1), id);
});

test('workspace environment reads engine settings without an engine or credentials', async (t) => {
  const project = mkdtempSync(join(tmpdir(), 'catalog-environment-'));
  t.after(() => rmSync(project, { recursive: true, force: true }));
  const calls: string[][] = [];
  let response: unknown = { version: 1, environment: { KEEP: 'old', TARGET: '目标' }, config_home: '/original/.lingxi', global_config_path: '/original/.lingxi.json' };
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: ((_bin, args, options) => {
    calls.push(args as string[]);
    assert.deepEqual((options as any).stdio, ['ignore', 'pipe', 'pipe']);
    assert.equal((options as any).env.ANTHROPIC_API_KEY, undefined);
    const spawned = child();
    queueMicrotask(() => spawned.finish(response));
    return spawned.process;
  }) as any });
  assert.deepEqual(await catalog.workspaceSettings(project), { environment: { KEEP: 'old', TARGET: '目标' }, configHome: '/original/.lingxi', globalConfigPath: '/original/.lingxi.json' });
  assert.equal(calls[0].at(-1), '--workspace-environment-json');
  response = { version: 1, environment: { BAD: 1 } };
  await assert.rejects(catalog.workspaceSettings(project), /invalid workspace environment/);
  response = { version: 1, environment: { 'BAD=KEY': 'value' } };
  await assert.rejects(catalog.workspaceSettings(project), /invalid workspace environment/);
});

test('catalog rejects nonabsolute or NUL source hints before host navigation', async (t) => {
  const project = mkdtempSync(join(tmpdir(), 'catalog-source-hint-'));
  t.after(() => rmSync(project, { recursive: true, force: true }));
  for (const workspace_path of ['relative', '/bad\0path']) {
    const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: (() => {
      const spawned = child();
      queueMicrotask(() => spawned.finish({ version: 1, sessions: [{ ...row, workspace_path }] }));
      return spawned.process;
    }) as any });
    await assert.rejects(catalog.list(project), /invalid session row/);
  }
});

test('removing a project fences an already pending scoped host catalog read', async (t) => {
  const directory = mkdtempSync(join(tmpdir(), 'catalog-remove-project-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const project = realpathSync(directory);
  const settings = new SettingsStore(join(project, 'settings'));
  settings.addProject(project);
  const bridge = { isProjectClosing: () => false, hasActiveWork: () => false,
    closeProject: async () => undefined };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer());
  t.after(() => host.dispose());
  let finish!: (result: { sessions: typeof row[] }) => void;
  (host as any).sessionCatalog = {
    list: () => new Promise(resolve => { finish = resolve; }), invalidate() {},
  };
  (host as any).bootstrap = async () => ({ settings: settings.getPublic() });
  const pending = (host as any).loadProjectSessions(project, 'current');
  await (host as any).removeProjectInternal(project);
  finish({ sessions: [row] });
  await pending;
  assert.equal((host as any).catalogs.has(project), false,
    'a late current-scope result must not recreate the removed sidebar catalog');
});
