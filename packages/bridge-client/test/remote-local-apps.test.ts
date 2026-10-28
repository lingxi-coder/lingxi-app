import assert from 'node:assert/strict';
import { test } from 'node:test';
import { isAppsListResult, isAppsOpenResult } from '../src/remote/localApps.js';
const app = { app_id: 'a' + '0'.repeat(32), name: 'A', summary: '', revision: 1, phase: 'stopped', background: false, execution_host: 'desktop', remote_enabled: true };
test('catalog accepts only bounded shared apps and no extra credentials', () => {
  assert.equal(isAppsListResult({ apps: [app] }), true);
  assert.equal(isAppsListResult({ apps: [app, app] }), false);
  assert.equal(isAppsListResult({ apps: [{ ...app, token: 'secret' }] }), false);
  assert.equal(isAppsListResult({ apps: [{ ...app, revision: Number.MAX_SAFE_INTEGER + 1 }] }), false);
  assert.equal(isAppsListResult({ apps: [{ ...app, revision: 0 }] }), false);
  assert.equal(isAppsListResult({ apps: [{ ...app, app_id: 'invalid' }] }), false);
  assert.equal(isAppsListResult({ apps: Array(257).fill(app) }), false);
});
test('bootstrap requires credential-free HTTPS and a fragment', () => {
  const open = { app_id: 'a' + '0'.repeat(32), revision: 1, expires_at: 1, url: 'https://a' + '0'.repeat(32) + '-0123456789abcdef0123.example.com/__lingxi_remote_bootstrap#' + 'x'.repeat(43) };
  assert.equal(isAppsOpenResult(open), true);
  assert.equal(isAppsOpenResult({ ...open, url: open.url.replace('-0123456789abcdef0123.', '.') }), false);
  assert.equal(isAppsOpenResult({ ...open, url: open.url.replace('-0123456789abcdef0123.', '-invalid.') }), false);
  assert.equal(isAppsOpenResult({ ...open, revision: 0 }), false);
  assert.equal(isAppsOpenResult({ ...open, url: open.url + '?extra' }), false);
  for (const url of ['http://a' + '0'.repeat(32) + '-0123456789abcdef0123.example.com/__lingxi_remote_bootstrap#' + 'x'.repeat(43), 'https://u:p@example.com/#ticket', 'https://example.com/']) assert.equal(isAppsOpenResult({ ...open, url }), false);
  assert.equal(isAppsOpenResult({ ...open, url: 'http://a' + '0'.repeat(32) + '-0123456789abcdef0123.localhost/__lingxi_remote_bootstrap#' + 'x'.repeat(43) }, true), true);
});
