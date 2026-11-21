import assert from 'node:assert/strict';
import { test, before, after, beforeEach } from 'node:test';
import { readFileSync } from 'node:fs';
import { randomBytes } from 'node:crypto';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getFirestore } from 'firebase-admin/firestore';
import { initializeTestEnvironment, assertFails } from '@firebase/rules-unit-testing';
import { doc, getDoc, setDoc, collection, getDocs } from 'firebase/firestore';
import { PROVIDER_LIMIT, VaultStore, parseKeyring } from '../src/llm-vault.mjs';

const options = { skip: !process.env.FIRESTORE_EMULATOR_HOST, timeout: 60000 };
const projectId = 'demo-lingxi-teams-t0';
const ring = parseKeyring(JSON.stringify({ current: 'k1', keys: { k1: randomBytes(32).toString('base64') } }));
let env, app, db, vault, now;
before(async () => {
  if (!process.env.FIRESTORE_EMULATOR_HOST) return;
  const [host, port] = process.env.FIRESTORE_EMULATOR_HOST.split(':');
  assert.ok(['127.0.0.1', 'localhost'].includes(host));
  env = await initializeTestEnvironment({ projectId, firestore: { host, port: Number(port),
    rules: readFileSync(new URL('../../../infra/firebase/firestore.default.rules', import.meta.url), 'utf8') } });
  app = initializeApp({ projectId }, 'llm-vault-test');
  db = getFirestore(app);
});
beforeEach(async () => {
  if (!env) return;
  await env.clearFirestore();
  now = Date.UTC(2026, 10, 18, 12, 0, 0);
  vault = new VaultStore(db, ring, () => now);
});
after(async () => { await env?.cleanup(); await db?.terminate(); if (app) await deleteApp(app); });
const deepseek = { action: 'upsert', id: 'deepseek', presetId: 'deepseek', apiKey: 'sk-test-deepseek-0001' };

test('vault store: a preset provider fills in from its preset and lists without its key', options, async () => {
  const saved = await vault.upsert('alice', deepseek, 'web');
  assert.equal(saved.protocol, 'open_ai_chat');
  assert.equal(saved.baseUrl, 'https://api.deepseek.com');
  assert.equal(saved.defaultModel, 'deepseek-flash');
  assert.equal(saved.webUsable, true);
  assert.equal(saved.source, 'web');
  assert.equal(saved.revision, 1);
  const list = await vault.list('alice');
  assert.deepEqual(list.map(entry => entry.id), ['deepseek']);
  assert.doesNotMatch(JSON.stringify(list), /sk-test-deepseek/u);
  const stored = JSON.stringify((await db.doc('llmVault/alice/providers/deepseek').get()).data());
  assert.doesNotMatch(stored, /sk-test-deepseek/u, 'the record holds only the sealed key');
  assert.deepEqual(await vault.list('bob'), []);
});

test('vault store: a change without a key keeps the key, and a stale revision is refused', options, async () => {
  await vault.upsert('alice', deepseek, 'web');
  const renamed = await vault.upsert('alice', { action: 'upsert', id: 'deepseek', label: 'Work DeepSeek', defaultModel: 'deepseek-v4-pro', expectedRevision: 1 }, 'desktop');
  assert.equal(renamed.revision, 2);
  assert.equal(renamed.label, 'Work DeepSeek');
  assert.equal(renamed.source, 'web', 'the key came from the Web');
  assert.equal((await vault.openKey('alice', 'deepseek')).apiKey, 'sk-test-deepseek-0001');
  await assert.rejects(vault.upsert('alice', { action: 'upsert', id: 'deepseek', label: 'x', expectedRevision: 1 }, 'web'), { code: 'REVISION_CONFLICT' });
  const rekeyed = await vault.upsert('alice', { ...deepseek, apiKey: 'sk-test-deepseek-0002' }, 'desktop');
  assert.equal(rekeyed.source, 'desktop');
  assert.equal(rekeyed.keyHint, 'sk-…0002');
  assert.equal((await vault.openKey('alice', 'deepseek')).apiKey, 'sk-test-deepseek-0002');
});

test('vault store: a stored key never follows a provider to a new endpoint', options, async () => {
  await vault.upsert('alice', deepseek, 'web');
  for (const change of [{ baseUrl: 'https://collector.example.com/v1' }, { protocol: 'anthropic_messages' }, { presetId: 'openai' }])
    await assert.rejects(vault.upsert('alice', { action: 'upsert', id: 'deepseek', ...change }, 'web'), { code: 'KEY_REQUIRED' }, JSON.stringify(change));
  assert.equal((await vault.openKey('alice', 'deepseek')).provider.baseUrl, 'https://api.deepseek.com');
  const moved = await vault.upsert('alice', { action: 'upsert', id: 'deepseek', baseUrl: 'https://api2.example.com/v1', apiKey: 'sk-test-deepseek-0003' }, 'web');
  assert.equal(moved.baseUrl, 'https://api2.example.com/v1');
  assert.equal((await vault.openKey('alice', 'deepseek')).apiKey, 'sk-test-deepseek-0003');
});

test('vault store: a new provider needs a key, a complete endpoint and room', options, async () => {
  await assert.rejects(vault.upsert('alice', { action: 'upsert', id: 'deepseek', presetId: 'deepseek' }, 'web'), { code: 'KEY_REQUIRED' });
  await assert.rejects(vault.upsert('alice', { action: 'upsert', id: 'mine', apiKey: 'k-123' }, 'web'), { code: 'PROVIDER_INCOMPLETE' });
  await assert.rejects(vault.upsert('alice', { action: 'upsert', id: 'mine', presetId: 'nope', apiKey: 'k-123' }, 'web'), { code: 'UNKNOWN_PRESET' });
  await assert.rejects(vault.upsert('alice', { action: 'upsert', id: 'azure', protocol: 'other', baseUrl: 'https://x.openai.azure.com', apiKey: 'k-123' }, 'desktop'), { code: 'PROVIDER_INCOMPLETE' });
  const azure = await vault.upsert('alice', { action: 'upsert', id: 'azure', protocol: 'other', desktopType: 'azure-openai', baseUrl: 'https://x.openai.azure.com', apiKey: 'k-123' }, 'desktop');
  assert.equal(azure.webUsable, false);
  for (let i = 1; i < PROVIDER_LIMIT; i++) await vault.upsert('alice', { action: 'upsert', id: `p${i}`, protocol: 'open_ai_chat', baseUrl: 'https://api.example.com/v1', apiKey: 'k-123' }, 'web');
  await assert.rejects(vault.upsert('alice', { action: 'upsert', id: 'one-more', protocol: 'open_ai_chat', baseUrl: 'https://api.example.com/v1', apiKey: 'k-123' }, 'web'), { code: 'PROVIDER_LIMIT' });
  await vault.upsert('alice', { action: 'upsert', id: 'p1', label: 'still editable' }, 'web');
});

test('vault store: a record copied to another owner does not open', options, async () => {
  await vault.upsert('alice', deepseek, 'web');
  await db.doc('llmVault/mallory/providers/deepseek').set((await db.doc('llmVault/alice/providers/deepseek').get()).data());
  await assert.rejects(vault.openKey('mallory', 'deepseek'), { code: 'VAULT_UNREADABLE' });
  await assert.rejects(vault.openKey('alice', 'missing'), { code: 'PROVIDER_NOT_FOUND' });
});

test('vault store: delete, export and every write are audited', options, async () => {
  await vault.upsert('alice', deepseek, 'web');
  await vault.upsert('alice', { action: 'upsert', id: 'openai', presetId: 'openai', apiKey: 'sk-test-openai-0001' }, 'web');
  const exported = await vault.exportKeys('alice', undefined, 'desktop');
  assert.deepEqual(exported.map(({ id, apiKey }) => [id, apiKey]).sort(), [['deepseek', 'sk-test-deepseek-0001'], ['openai', 'sk-test-openai-0001']]);
  assert.deepEqual((await vault.exportKeys('alice', ['openai', 'missing'], 'ios')).map(entry => entry.id), ['openai']);
  await vault.remove('alice', { action: 'delete', id: 'deepseek' }, 'web');
  await vault.remove('alice', { action: 'delete', id: 'deepseek' }, 'web');
  await assert.rejects(vault.remove('alice', { action: 'delete', id: 'openai', expectedRevision: 7 }, 'web'), { code: 'REVISION_CONFLICT' });
  assert.deepEqual((await vault.list('alice')).map(entry => entry.id), ['openai']);
  const audit = (await db.collection('llmVault/alice/vaultAudit').get()).docs.map(entry => entry.data());
  const rows = audit.map(({ action, providerId, client }) => `${action}:${providerId}:${client}`).sort();
  assert.deepEqual(rows, ['create:deepseek:web', 'create:openai:web', 'delete:deepseek:web', 'export:deepseek:desktop', 'export:openai:desktop', 'export:openai:ios']);
  assert.doesNotMatch(JSON.stringify(audit), /sk-test/u);
});

test('vault store: limits count per minute and per day', options, async () => {
  for (let i = 0; i < 3; i++) await vault.take('alice', 'chat', { perMinute: 3, perDay: 5 });
  await assert.rejects(vault.take('alice', 'chat', { perMinute: 3, perDay: 5 }), { code: 'RATE_LIMITED' });
  await vault.take('alice', 'export', { perDay: 1 });
  await assert.rejects(vault.take('alice', 'export', { perDay: 1 }), { code: 'RATE_LIMITED' });
  now += 60_000;
  await vault.take('alice', 'chat', { perMinute: 3, perDay: 5 });
  await vault.take('alice', 'chat', { perMinute: 3, perDay: 5 });
  await assert.rejects(vault.take('alice', 'chat', { perMinute: 3, perDay: 5 }), { code: 'RATE_LIMITED' }, 'the day is used up');
  now += 86_400_000;
  await vault.take('alice', 'chat', { perMinute: 3, perDay: 5 });
  await vault.take('bob', 'export', { perDay: 1 });
});

test('vault rules: no client reads or writes the vault, not even its owner', options, async () => {
  await vault.upsert('alice', deepseek, 'web');
  const alice = env.authenticatedContext('alice').firestore();
  await assertFails(getDoc(doc(alice, 'llmVault/alice/providers/deepseek')));
  await assertFails(getDocs(collection(alice, 'llmVault/alice/providers')));
  await assertFails(getDoc(doc(alice, 'llmVault/alice')));
  await assertFails(getDocs(collection(alice, 'llmVault/alice/audit')));
  await assertFails(setDoc(doc(alice, 'llmVault/alice/providers/mine'), { protocol: 'open_ai_chat' }));
  await assertFails(getDoc(doc(env.unauthenticatedContext().firestore(), 'llmVault/alice/providers/deepseek')));
});
