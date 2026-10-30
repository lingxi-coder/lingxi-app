import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

import { CH_PLUGIN_SECRET_CLEANUP, HostController } from '../src/main/host.js';
import { DiagnosticBuffer } from '../src/main/host-utils.js';
import { SettingsStore } from '../src/main/settings.js';

const PLUGIN_ID = 'secure@acme';

function installedRecord(scope = 'user') {
  return {
    scope,
    installPath: '/test/plugin-cache/secure/1.0.0',
    version: '1.0.0',
    installedAt: '2026-10-26T00:00:00.000Z',
    lastUpdated: '2026-10-26T00:00:00.000Z',
  };
}

function writeRegistry(configHome: string, plugins: Record<string, unknown>) {
  const directory = join(configHome, 'plugins');
  mkdirSync(directory, { recursive: true });
  writeFileSync(join(directory, 'installed_plugins.json'), JSON.stringify({ version: 2, plugins }));
}

async function cleanupHarness(
  t: { after(callback: () => void): void },
  options: {
    plugins?: Record<string, unknown>;
    refs?: Array<{ pluginId: string; key: string }>;
    broker?: Record<string, (...args: any[]) => Promise<any>>;
    productPaths?: () => Promise<{ configHome: string }>;
  } = {},
) {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-plugin-secret-cleanup-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const configHome = join(root, 'config');
  if (options.plugins) writeRegistry(configHome, options.plugins);
  const settings = new SettingsStore(join(root, 'user-data'));
  const handlers = new Map<string, (...args: any[]) => any>();
  const ipc = {
    handle: (channel: string, handler: (...args: any[]) => any) => handlers.set(channel, handler),
    removeHandler: (channel: string) => handlers.delete(channel),
  };
  let invalidations = 0;
  const bridge = {
    registerIpc() {},
    registerWindow() {},
    runtimeSummaries: [],
    invalidateLaunchConfigCache() { invalidations += 1; },
    hasActiveWork: () => false,
    get: () => undefined,
  };
  const refs = options.refs ?? [];
  const deleted: string[] = [];
  const broker = options.broker ?? {
    listPluginSecrets: async () => refs.slice(),
    deletePluginSecret: async (pluginId: string, key: string) => {
      deleted.push(`${pluginId}/${key}`);
      const index = refs.findIndex(ref => ref.pluginId === pluginId && ref.key === key);
      if (index >= 0) refs.splice(index, 1);
    },
  };
  const catalog = {
    productPaths: options.productPaths ?? (async () => ({ brandHome: configHome })),
  };
  const host = new HostController(
    settings,
    bridge as any,
    new DiagnosticBuffer(),
    catalog as any,
    ipc as any,
    undefined,
    broker as any,
  );
  host.registerIpc();
  const window = new EventEmitter() as any;
  window.mainFrame = { url: 'file:///LingXi/index.html' };
  window.isDestroyed = () => false;
  window.send = () => undefined;
  host.registerWindow(window, window.mainFrame.url);
  t.after(() => host.dispose());

  const invoke = (pluginId = PLUGIN_ID, lastScopeRemoved: unknown = true) => {
    const handler = handlers.get(CH_PLUGIN_SECRET_CLEANUP);
    assert.ok(handler, 'cleanup IPC handler must be registered');
    return handler({ sender: window, senderFrame: window.mainFrame }, pluginId, lastScopeRemoved);
  };
  return { configHome, deleted, invoke, invalidations: () => invalidations, refs, root };
}

test('last-scope uninstall deletes broker keys absent from the current schema and preserves other plugins', async t => {
  const h = await cleanupHarness(t, {
    plugins: { 'other@acme': [installedRecord()] },
    refs: [
      { pluginId: PLUGIN_ID, key: 'CURRENT_TOKEN' },
      { pluginId: PLUGIN_ID, key: 'RENAMED_ORPHAN_TOKEN' },
      { pluginId: 'other@acme', key: 'TOKEN' },
    ],
  });

  const result = await h.invoke();

  assert.deepEqual(result, { pluginId: PLUGIN_ID, status: 'cleared', deletedCount: 2, failedCount: 0 });
  assert.deepEqual(h.deleted.sort(), [`${PLUGIN_ID}/CURRENT_TOKEN`, `${PLUGIN_ID}/RENAMED_ORPHAN_TOKEN`]);
  assert.deepEqual(h.refs, [{ pluginId: 'other@acme', key: 'TOKEN' }]);
  assert.equal(h.invalidations(), 1);
});

test('non-final scope retains all broker credentials', async t => {
  const h = await cleanupHarness(t, {
    plugins: { [PLUGIN_ID]: [installedRecord('user'), installedRecord('project')] },
    refs: [{ pluginId: PLUGIN_ID, key: 'TOKEN' }],
  });

  const result = await h.invoke();

  assert.deepEqual(result, { pluginId: PLUGIN_ID, status: 'retained', deletedCount: 0, failedCount: 0 });
  assert.deepEqual(h.deleted, []);
  assert.equal(h.invalidations(), 0);
});

test('malformed or unavailable registry fails closed before broker enumeration', async t => {
  for (const kind of ['malformed', 'missing'] as const) {
    let brokerLists = 0;
    const h = await cleanupHarness(t, {
      plugins: {},
      broker: {
        listPluginSecrets: async () => { brokerLists += 1; return [{ pluginId: PLUGIN_ID, key: 'TOKEN' }]; },
        deletePluginSecret: async () => { assert.fail('invalid registry must not delete credentials'); },
      },
    });
    const registry = join(h.configHome, 'plugins', 'installed_plugins.json');
    if (kind === 'malformed') writeFileSync(registry, '{broken');
    else rmSync(registry);

    await assert.rejects(h.invoke());
    assert.equal(brokerLists, 0);
  }
});

test('partial broker deletion is reported and a retry clears only the remaining keys', async t => {
  const refs = [
    { pluginId: PLUGIN_ID, key: 'A_TOKEN' },
    { pluginId: PLUGIN_ID, key: 'B_TOKEN' },
    { pluginId: 'other@acme', key: 'TOKEN' },
  ];
  let failBOnce = true;
  const h = await cleanupHarness(t, {
    plugins: {},
    refs,
    broker: {
      listPluginSecrets: async () => refs.slice(),
      deletePluginSecret: async (pluginId: string, key: string) => {
        if (pluginId === PLUGIN_ID && key === 'B_TOKEN' && failBOnce) {
          failBOnce = false;
          throw new Error('simulated broker deletion failure');
        }
        const index = refs.findIndex(ref => ref.pluginId === pluginId && ref.key === key);
        if (index >= 0) refs.splice(index, 1);
      },
    },
  });

  const first = await h.invoke();
  assert.equal(first.status, 'partial');
  assert.equal(first.deletedCount, 1);
  assert.equal(first.failedCount, 1);
  assert.deepEqual(refs, [
    { pluginId: PLUGIN_ID, key: 'B_TOKEN' },
    { pluginId: 'other@acme', key: 'TOKEN' },
  ]);

  const retry = await h.invoke();
  assert.deepEqual(retry, { pluginId: PLUGIN_ID, status: 'cleared', deletedCount: 1, failedCount: 0 });
  assert.deepEqual(refs, [{ pluginId: 'other@acme', key: 'TOKEN' }]);
  assert.equal(h.invalidations(), 2);
});

test('registry change after broker listing aborts deletion and keeps credentials', async t => {
  const refs = [{ pluginId: PLUGIN_ID, key: 'TOKEN' }];
  let configHome = '';
  const deleted: string[] = [];
  const h = await cleanupHarness(t, {
    plugins: {},
    refs,
    broker: {
      listPluginSecrets: async () => {
        writeRegistry(configHome, { [PLUGIN_ID]: [installedRecord('project')] });
        return refs.slice();
      },
      deletePluginSecret: async (pluginId: string, key: string) => { deleted.push(`${pluginId}/${key}`); },
    },
  });
  configHome = h.configHome;

  const result = await h.invoke();

  assert.equal(result.status, 'retained');
  assert.deepEqual(deleted, []);
});
