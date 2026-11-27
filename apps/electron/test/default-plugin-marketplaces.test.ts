import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import type { ClientEvent, PluginAdminCommandDto } from '@lingxi/bridge-client';
import { seedDefaultPluginMarketplaces } from '../src/main/defaultPluginMarketplaces';

const sources = [
  ['claude-plugins-official', 'anthropics/claude-plugins-official'],
  ['anthropic-agent-skills', 'anthropics/skills'],
  ['knowledge-work-plugins', 'anthropics/knowledge-work-plugins'],
] as const;

function registryRow(name: string, repo: string) {
  return { name, source_json: JSON.stringify({ source: 'github', repo }) };
}

function testEngine(registered: ReturnType<typeof registryRow>[]) {
  const calls: string[] = [];
  const failures = new Set<string>();
  let revision = 0;
  const catalog = (): ClientEvent => ({
    type: 'plugin_catalog', catalog_json: JSON.stringify({ marketplaces: registered, revisions: { user: `r${revision}` } }),
  });
  const pluginAdmin = async (command: PluginAdminCommandDto): Promise<ClientEvent[]> => {
    if (command.action === 'get_catalog') return [catalog()];
    assert.equal(command.action, 'apply_operation');
    assert.equal(command.scope, 'user');
    assert.equal(command.revision, `r${revision}`);
    const payload = JSON.parse(command.payload_json ?? '');
    assert.deepEqual(payload, { action: 'marketplace_add', source: payload.source, scope: 'user', confirmed: true });
    calls.push(payload.source);
    if (failures.has(payload.source)) throw new Error('offline');
    registered.push(registryRow(sources.find((entry) => entry[1] === payload.source)![0], payload.source));
    revision += 1;
    return [{ type: 'configuration_operation', domain: 'plugin', operation_id: 1, status: 'succeeded', effect: 'restart_required' }, catalog()];
  };
  return { pluginAdmin, calls, failures };
}

test('registers missing defaults with fresh revisions and remembers a user removal', async (t) => {
  const brandHome = await mkdtemp(join(tmpdir(), 'lingxi-marketplace-seed-'));
  t.after(() => rm(brandHome, { recursive: true, force: true }));
  const registered = [registryRow('existing-official-alias', sources[0][1])];
  const { pluginAdmin, calls } = testEngine(registered);
  let refreshes = 0;
  const launch = () => seedDefaultPluginMarketplaces({
    brandHome, pluginAdmin, onAdded: () => { refreshes += 1; }, onError: (_source, error) => { throw error; },
  });
  await launch();
  assert.deepEqual(calls, [sources[1][1], sources[2][1]]);
  assert.equal(refreshes, 2);

  registered.splice(1, 1);
  await launch();
  assert.equal(registered.some((marketplace) => marketplace.name === sources[1][0]), false);
  assert.deepEqual(calls, [sources[1][1], sources[2][1]]);
});

test('leaves conflicts in place and retries failed registrations on a later launch', async (t) => {
  const brandHome = await mkdtemp(join(tmpdir(), 'lingxi-marketplace-seed-'));
  t.after(() => rm(brandHome, { recursive: true, force: true }));
  const registered = [registryRow(sources[0][0], 'other/marketplace')];
  const { pluginAdmin, calls, failures } = testEngine(registered);
  failures.add(sources[1][1]);
  const errors: string[] = [];
  const launch = () => seedDefaultPluginMarketplaces({ brandHome, pluginAdmin, onAdded() {}, onError: (source) => { errors.push(source); } });
  await launch();
  assert.deepEqual(calls, [sources[1][1], sources[2][1]]);
  assert.deepEqual(errors, [sources[1][1]]);
  assert.deepEqual(registered[0], registryRow(sources[0][0], 'other/marketplace'));

  failures.clear();
  await launch();
  assert.deepEqual(calls, [sources[1][1], sources[2][1], sources[1][1]]);
});
