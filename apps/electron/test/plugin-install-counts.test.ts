import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createPluginInstallCountsReader, parsePluginInstallCounts } from '../src/main/pluginInstallCounts';

const stats = {
  plugins: [
    { plugin: 'frontend-design@claude-plugins-official', unique_installs: 1_234_567 },
    { plugin: 'new-plugin@claude-plugins-official', unique_installs: 0 },
    { plugin: 'frontend-design@private-market', unique_installs: 99 },
    { plugin: 'invalid@claude-plugins-official', unique_installs: -1 },
    { plugin: 'fractional@claude-plugins-official', unique_installs: 1.5 },
  ],
};

test('keeps only valid published counts from the official marketplace', () => {
  assert.deepEqual(parsePluginInstallCounts(stats), { 'frontend-design': 1_234_567, 'new-plugin': 0 });
  assert.throws(() => parsePluginInstallCounts({ plugins: [stats.plugins[2]] }), /no valid counts/);
});

test('shares fetches across clients and caches a failed lookup without inventing counts', async () => {
  let requests = 0;
  const reader = createPluginInstallCountsReader(async (_url, options) => {
    assert.equal(options?.credentials, 'omit');
    requests += 1;
    return Response.json(stats);
  });
  const [a, b] = await Promise.all([reader(), reader()]);
  assert.deepEqual(a, b);
  assert.deepEqual(await reader(), a);
  assert.equal(requests, 1);

  const offlineReader = createPluginInstallCountsReader(async () => {
    requests += 1;
    return new Response(null, { status: 503 });
  });
  assert.deepEqual(await offlineReader(), {});
  assert.deepEqual(await offlineReader(), {});
  assert.equal(requests, 2);
});
