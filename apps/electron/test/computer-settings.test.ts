import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync, statSync, mkdirSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { ComputerSettingsStore, siteOrigin } from '../src/main/computerSettings.js';
import { assertComputerAccessSubset } from '../src/main/computerAccessApproval.js';
import type { ComputerAccessRequestDto, ComputerAccessResponseDto } from '@lingxi/bridge-client';

function fixture() {
  const directory = mkdtempSync(join(tmpdir(), 'lingxi-computer-settings-'));
  let changes = 0;
  const store = new ComputerSettingsStore(directory, {
    onChanged: () => { changes++; },
    resolveApplication: async bundleId => ({ bundleId, displayName: 'Test Application' }),
  });
  return { store, changed: () => changes, cleanup: () => rmSync(directory, { recursive: true, force: true }) };
}
test('device policy persists atomically and permanently removing access revokes it', async () => {
  const f = fixture();
  try {
    let state = f.store.snapshot();
    assert.equal(statSync(f.store.policyPath).mode & 0o777, 0o600);
    state = await f.store.execute({ action: 'setApplication', revision: state.revision, bundleId: 'org.example.test', permission: 'allow' });
    assert.equal(state.apps[0]?.permission, 'allow');
    state = await f.store.execute({ action: 'removeApplication', revision: state.revision, bundleId: 'org.example.test' });
    assert.equal(state.apps[0]?.permission, 'deny');
    assert.equal(new ComputerSettingsStore(join(f.store.policyPath, '../..')).policy().apps[0]?.permission, 'deny');
    assert.equal(f.changed(), 2);
  } finally { f.cleanup(); }
});
test('stale settings and invalid commands cannot replace current grants', async () => {
  const f = fixture();
  try {
    const initial = f.store.snapshot();
    await f.store.execute({ action: 'setEnabled', revision: initial.revision, enabled: false });
    const before = readFileSync(f.store.policyPath, 'utf8');
    await assert.rejects(f.store.execute({ action: 'setEnabled', revision: initial.revision, enabled: true }), /changed/);
    await assert.rejects(f.store.execute({ action: 'setBrowser', revision: f.store.policy().revision, key: '__proto__', value: true }), /Invalid browser/);
    await assert.rejects(f.store.execute({ action: 'setApplication', revision: f.store.policy().revision, bundleId: 'com.lingxi.code.development', permission: 'allow' }), /other than LingXi/);
    assert.equal(readFileSync(f.store.policyPath, 'utf8'), before);
  } finally { f.cleanup(); }
});
test('concurrent mutations and sender navigation cannot commit a delayed app resolution', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'lingxi-computer-race-'));
  let resolve!: (app: { bundleId: string; displayName: string }) => void;
  const store = new ComputerSettingsStore(directory, { resolveApplication: () => new Promise(done => { resolve = done; }) });
  try {
    const before = readFileSync(store.policyPath, 'utf8');
    const pending = store.execute({ action: 'setApplication', revision: store.policy().revision, bundleId: 'org.example.test', permission: 'allow' }, () => { throw new Error('sender navigated'); });
    await assert.rejects(store.execute({ action: 'setEnabled', revision: store.policy().revision, enabled: false }), /being updated/);
    resolve({ bundleId: 'org.example.test', displayName: 'Test' });
    await assert.rejects(pending, /sender navigated/);
    assert.equal(readFileSync(store.policyPath, 'utf8'), before);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
test('browser permissions use normalized exact origins and preserve independent scopes', async () => {
  const f = fixture();
  try {
    let state = f.store.snapshot();
    state = await f.store.execute({ action: 'setSite', revision: state.revision, origin: 'https://EXAMPLE.com:443/', browsing: 'allow', downloads: 'deny' });
    assert.deepEqual(state.browser.sites, [{ origin: 'https://example.com', browsing: 'allow', downloads: 'deny' }]);
    state = await f.store.execute({ action: 'setBrowser', revision: state.revision, key: 'history', value: 'deny' });
    state = await f.store.execute({ action: 'setBrowser', revision: state.revision, key: 'fullCdp', value: true });
    assert.equal(state.browser.history, 'deny');
    assert.equal(state.browser.fullCdp, true);
    assert.equal(state.browser.siteTools, false);
    for (const invalid of ['https://*.example.com', 'https://example.com/path', 'file:///etc/passwd', 'https://user:pass@example.com', 'https://example.com#fragment']) assert.throws(() => siteOrigin(invalid));
    assert.notEqual(siteOrigin('http://example.com'), siteOrigin('https://example.com'));
    assert.notEqual(siteOrigin('https://sub.example.com'), siteOrigin('https://example.com'));
  } finally { f.cleanup(); }
});
test('unreadable settings fail closed without overwriting the original file', async () => {
  const f = fixture();
  try {
    writeFileSync(f.store.policyPath, '{broken');
    const broken = new ComputerSettingsStore(join(f.store.policyPath, '../..'));
    assert.equal(broken.policy().enabled, false);
    await assert.rejects(broken.execute({ action: 'setEnabled', revision: broken.policy().revision, enabled: true }), /could not be read/);
    assert.equal(readFileSync(f.store.policyPath, 'utf8'), '{broken');
  } finally { f.cleanup(); }
});
test('trusted approval boundary refuses capabilities and applications outside the pending request', () => {
  const request: ComputerAccessRequestDto = { request_id: 7, reason: 'Test', apps: [{ label: 'org.example.test' }], tier: 'full', clipboard_read: false, clipboard_write: false, system_key_combos: false };
  const response: ComputerAccessResponseDto = { granted_apps: ['org.example.test'], clipboard_read: false, clipboard_write: false, system_key_combos: false };
  assert.doesNotThrow(() => assertComputerAccessSubset(request, response));
  assert.throws(() => assertComputerAccessSubset(request, { ...response, granted_apps: ['com.other.app'] }), /exceeds/);
  assert.throws(() => assertComputerAccessSubset(request, { ...response, granted_apps: [...response.granted_apps, ...response.granted_apps] }), /exceeds/);
  for (const key of ['clipboard_read', 'clipboard_write', 'system_key_combos'] as const) assert.throws(() => assertComputerAccessSubset(request, { ...response, [key]: true }), /exceeds/);
  assert.throws(() => assertComputerAccessSubset({ ...request, tcc_state: { accessibility: false, screen_recording: true } }, response), /exceeds/);
});

test('disconnecting Browser invalidates the device revision and pending approval owners', async () => {
  const f = fixture();
  try {
    const calls: string[] = [];
    const status = { connected: true, installed: true, extensionDirectory: '/controlled/extension' };
    f.store.attachBrowser({ status: () => status, retry: async () => status, install: async () => status,
      remove: async () => { calls.push('remove'); return status; }, policyChanged: () => { calls.push('revoke'); } });
    const before = f.store.policy().revision;
    await f.store.execute({ action: 'removeBrowser' });
    assert.notEqual(f.store.policy().revision, before);
    assert.deepEqual(calls, ['revoke', 'remove']);
    assert.ok(f.changed() >= 1);
  } finally { f.cleanup(); }
});

test('live source changes replace effective revisions and revoke Browser grants even with a reused label', async () => {
  const f = fixture();
  try {
    let revocations = 0;
    const status = { connected: true, installed: true, extensionDirectory: '/test' };
    f.store.attachBrowser({ status: () => status, retry: async () => status, install: async () => status,
      remove: async () => status, policyChanged: () => { revocations++; f.store.policy(); } });
    const before = f.store.policy();
    const external = { ...JSON.parse(readFileSync(f.store.policyPath, 'utf8')), enabled: false };
    writeFileSync(f.store.policyPath, JSON.stringify(external), { mode: 0o600 });
    const next = f.store.policy();
    assert.equal(next.enabled, false);
    assert.notEqual(next.revision, before.revision);
    assert.equal(revocations, 1);
    assert.equal(f.store.policy().revision, next.revision);
    const authority = JSON.parse(readFileSync(f.store.executionPolicyPath, 'utf8'));
    assert.equal(authority.ownerPid, process.pid);
    assert.equal(authority.revision, next.revision);
    assert.deepEqual(authority.source, { path: f.store.policyPath, sha256: createHash('sha256').update(readFileSync(f.store.policyPath)).digest('hex') });
    await assert.rejects(f.store.execute({ action: 'setEnabled', revision: before.revision, enabled: true }), /changed/);
  } finally { f.cleanup(); }
});

for (const damage of ['corrupt', 'delete', 'invalid-utf8'] as const) test(`live ${damage} source latches disabled and restoring it cannot revive grants`, () => {
  const f = fixture();
  try {
    const source = readFileSync(f.store.policyPath);
    const authorityPath = f.store.executionPolicyPath;
    if (damage === 'delete') rmSync(f.store.policyPath);
    else writeFileSync(f.store.policyPath, damage === 'corrupt' ? '{broken' : Buffer.from([0xff]));
    assert.equal(f.store.policy().enabled, false);
    assert.ok(f.store.snapshot().error);
    assert.equal(JSON.parse(readFileSync(authorityPath, 'utf8')).enabled, false);
    writeFileSync(f.store.policyPath, source, { mode: 0o600 });
    assert.equal(f.store.policy().enabled, false);
    assert.ok(f.store.snapshot().error);
    assert.notEqual(f.store.executionPolicyPath, authorityPath);
  } finally { f.cleanup(); }
});

test('initial settings write failure disables the feature without throwing', () => {
  const directory = mkdtempSync(join(tmpdir(), 'lingxi-computer-init-'));
  try {
    const blocked = join(directory, 'not-a-directory');
    writeFileSync(blocked, 'preserve');
    const store = new ComputerSettingsStore(blocked);
    assert.equal(store.policy().enabled, false);
    assert.ok(store.snapshot().error);
    assert.equal(readFileSync(blocked, 'utf8'), 'preserve');
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test('execution authority write failure after configuration commit fails closed', async () => {
  const f = fixture();
  try {
    const authorityPath = f.store.executionPolicyPath;
    rmSync(authorityPath);
    mkdirSync(authorityPath);
    await assert.rejects(f.store.execute({ action: 'setBrowser', revision: f.store.policy().revision, key: 'fullCdp', value: true }), /could not be read or saved/);
    assert.equal(f.store.policy().enabled, false);
    assert.ok(f.store.snapshot().error);
    assert.notEqual(f.store.executionPolicyPath, authorityPath);
    assert.equal(JSON.parse(readFileSync(f.store.policyPath, 'utf8')).browser.fullCdp, true);
  } finally { f.cleanup(); }
});

test('existing uninstalled app rules can be reset or removed without resolving an application', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'lingxi-computer-uninstalled-'));
  try {
    const initial = new ComputerSettingsStore(directory);
    const policy = JSON.parse(readFileSync(initial.policyPath, 'utf8'));
    policy.apps = [{ bundleId: 'org.example.removed', displayName: 'Removed', permission: 'deny' }];
    writeFileSync(initial.policyPath, JSON.stringify(policy), { mode: 0o600 });
    const store = new ComputerSettingsStore(directory, { resolveApplication: async () => { throw new Error('Application is not installed'); } });
    let state = await store.execute({ action: 'setApplication', revision: store.policy().revision, bundleId: 'org.example.removed', permission: 'ask' });
    assert.equal(state.apps[0]?.permission, 'ask');
    await assert.rejects(store.execute({ action: 'setApplication', revision: state.revision, bundleId: 'org.example.removed', permission: 'allow' }), /not installed/);
    state = await store.execute({ action: 'removeApplication', revision: state.revision, bundleId: 'org.example.removed' });
    assert.deepEqual(state.apps, []);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test('Browser installation receives a guard for asynchronous commit and unavailable Browser does not disable native settings', async () => {
  const f = fixture();
  try {
    const status = { connected: false, installed: false, extensionDirectory: '/test' };
    let trusted = true;
    let installed = false;
    f.store.attachBrowser({ status: () => status, retry: async () => status,
      install: async guard => { await Promise.resolve(); trusted = false; guard?.(); installed = true; return status; },
      remove: async () => status, policyChanged: () => {} });
    await assert.rejects(f.store.execute({ action: 'installBrowser' }, () => { if (!trusted) throw new Error('sender navigated'); }), /sender navigated/);
    assert.equal(installed, false);
    f.store.browserUnavailable('Could not start');
    assert.equal(f.store.snapshot().capabilities.browser, false);
    assert.equal(f.store.snapshot().browserStatus.error, 'Could not start');
    assert.equal(f.store.policy().enabled, true);
  } finally { f.cleanup(); }
});

test('a delayed application lookup cannot overwrite externally changed policy', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'lingxi-computer-external-race-'));
  let resolve!: (app: { bundleId: string; displayName: string }) => void;
  const store = new ComputerSettingsStore(directory, { resolveApplication: () => new Promise(done => { resolve = done; }) });
  try {
    const pending = store.execute({ action: 'setApplication', revision: store.policy().revision, bundleId: 'org.example.test', permission: 'allow' });
    const external = { ...JSON.parse(readFileSync(store.policyPath, 'utf8')), enabled: false };
    const contents = JSON.stringify(external);
    writeFileSync(store.policyPath, contents, { mode: 0o600 });
    resolve({ bundleId: 'org.example.test', displayName: 'Test' });
    await assert.rejects(pending, /changed/);
    assert.equal(readFileSync(store.policyPath, 'utf8'), contents);
    assert.equal(store.policy().enabled, false);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
