import assert from 'node:assert/strict';
import { test } from 'node:test';
import { LocalAppsGateway } from '../src/main/remoteControl/localAppsGateway.js';
import type { RemoteViewer, RemoteViewerChannel } from '../src/main/remoteControl/gateway.js';

const viewer: RemoteViewer = { account_uid: 'account', host_id: 'host', device_id: 'device', conn_id: 'conn', device_name: 'phone', platform: 'ios' };
const channel = { notify() {}, async request() { return {}; } } as RemoteViewerChannel;
const app = { app_id: 'a' + '0'.repeat(32), name: 'App', summary: '', revision: 1, phase: 'running' as const, background: false, execution_host: 'desktop' as const, remote_enabled: true as const };
const opened = { app_id: 'a' + '0'.repeat(32), revision: 1, url: 'https://a' + '0'.repeat(32) + '-0123456789abcdef0123.example.com/__lingxi_remote_bootstrap#' + 'x'.repeat(43), expires_at: 100 };

test('only Local App catalog and browser bootstrap are intercepted and owner-bound', async () => {
  const calls: string[] = [];
  const gateway = new LocalAppsGateway({ async handle(_v, method) { calls.push(method); return 'base'; }, viewerClosed() { calls.push('baseClosed'); } }, {
    async list(v) { assert.equal(v, viewer); calls.push('list'); return [app]; },
    async open(v, id) { assert.equal(v, viewer); assert.equal(id, 'a' + '0'.repeat(32)); calls.push('open'); return opened; },
    viewerClosed() { calls.push('appsClosed'); },
  });
  assert.deepEqual(await gateway.handle(viewer, 'apps.list', {}, channel), { apps: [app] });
  assert.deepEqual(await gateway.handle(viewer, 'apps.open', { app_id: 'a' + '0'.repeat(32) }, channel), opened);
  assert.equal(await gateway.handle(viewer, 'agent.invoke', {}, channel), 'base');
  await assert.rejects(gateway.handle({ ...viewer, account_uid: undefined }, 'apps.list', {}, channel), /missing owner scope/);
  await assert.rejects(gateway.handle(viewer, 'apps.open', { app_id: 'a' + '0'.repeat(32), tool: 'shell' }, channel), /invalid apps.open parameters/);
  gateway.viewerClosed(viewer);
  assert.deepEqual(calls, ['list', 'open', 'agent.invoke', 'appsClosed', 'baseClosed']);
});

test('invalid or unsafe delegate results are rejected', async () => {
  const base = { async handle() {}, viewerClosed() {} };
  let url = 'http://a' + '0'.repeat(32) + '-0123456789abcdef0123.example.com/__lingxi_remote_bootstrap#' + 'x'.repeat(43);
  const gateway = new LocalAppsGateway(base, { async list() { return [{ ...app, remote_enabled: false } as unknown as typeof app]; }, async open() { return { ...opened, url }; }, viewerClosed() {} });
  await assert.rejects(gateway.handle(viewer, 'apps.list', {}, channel), /invalid apps.list result/);
  await assert.rejects(gateway.handle(viewer, 'apps.open', { app_id: 'a' + '0'.repeat(32) }, channel), /invalid apps.open result/);
  url = 'https://user:pass@a' + '0'.repeat(32) + '-0123456789abcdef0123.example.com/__lingxi_remote_bootstrap#' + 'x'.repeat(43);
  await assert.rejects(gateway.handle(viewer, 'apps.open', { app_id: 'a' + '0'.repeat(32) }, channel), /invalid apps.open result/);
});
