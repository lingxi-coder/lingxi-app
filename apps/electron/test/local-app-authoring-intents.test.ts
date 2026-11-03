import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { AuthoringSourceIntents } from '../src/main/local-apps/authoringSourceIntents.ts';
import { AuthoringSessionIntents } from '../src/main/local-apps/authoringSessionIntents.ts';

test('source intent retries survive restart and distinct user sessions get distinct request ids', async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-source-intent-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const projectPath = join(await fs.realpath(root), 'project');
  const input = { purpose: 'create' as const, sourceDirectory: projectPath, projectPath, sessionId: randomUUID() };
  const first = new AuthoringSourceIntents(root);
  assert.equal(await first.findSourceRequestId(input), undefined);
  const [requestId, concurrent] = await Promise.all([first.sourceRequestId(input), first.sourceRequestId(input)]);
  assert.equal(concurrent, requestId);
  const reopened = new AuthoringSourceIntents(root);
  assert.equal(await reopened.findSourceRequestId(input), requestId);
  assert.equal(await reopened.sourceRequestId(input), requestId, 'cold retry uses its original persisted operation id');
  assert.notEqual(await reopened.sourceRequestId({ ...input, sessionId: randomUUID() }), requestId,
    'a separate user-created session is not folded into the earlier operation');
  assert.notEqual(await reopened.sourceRequestId({ ...input, purpose: 'rebind', appId: `a${'1'.repeat(32)}` }), requestId,
    'a rebind is a distinct operation even for the same source and session');
});

test('a corrupt source intent registry fails closed without overwriting recovery information', async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-bad-intent-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const path = join(root, 'authoring-source-intents.v1.json');
  await fs.writeFile(path, '{"version":1,"entries":[{"requestId":"not-a-uuid"}]}');
  const before = await fs.readFile(path, 'utf8');
  const source = join(await fs.realpath(root), 'source');
  await assert.rejects(new AuthoringSourceIntents(root).sourceRequestId({ purpose: 'create', sourceDirectory: source,
    projectPath: source, sessionId: randomUUID() }), /intent.*invalid/i);
  assert.equal(await fs.readFile(path, 'utf8'), before);
});

test('source copy intents persist independently and recover their own exact result', async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-copy-intent-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const sourcePath = join(await fs.realpath(root), 'copy');
  const input = { purpose: 'restoreSource' as const, appId: `a${'1'.repeat(32)}`, revision: 3, sourcePath };
  const first = new AuthoringSourceIntents(root);
  const a = await first.createCopy(input), b = await first.createCopy(input);
  assert.notEqual(a.requestId, b.requestId, 'separate user operations are never folded together');
  const reopened = new AuthoringSourceIntents(root);
  assert.deepEqual(await reopened.getCopy(a.requestId), a, 'an interrupted copy retains its original payload and request id');
  const sourceId = randomUUID();
  const ready = await reopened.recordCopy(a.requestId, sourceId, input.appId);
  assert.equal(ready.state, 'ready');
  await assert.rejects(reopened.recordCopy(a.requestId, randomUUID(), input.appId), /does not match/);
  await reopened.completeCopy(a.requestId);
  assert.equal((await new AuthoringSourceIntents(root).getCopy(a.requestId))?.state, 'completed');
  assert.equal((await reopened.getCopy(b.requestId))?.state, 'pending');
});

test('session creation intent retains its exact confirmed source across restart and rejects replacement', async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-session-intent-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const projectPath = join(await fs.realpath(root), 'project'), sessionId = randomUUID();
  const ledger = new AuthoringSessionIntents(root);
  const saved = await ledger.create({ projectPath, sessionId }, { name: 'App', appId: `a${'1'.repeat(32)}`, setCurrent: true });
  await assert.rejects(ledger.ready(saved.requestId), /unavailable/);
  const source = { scope: 'repository' as const, transcriptPath: join(root, 'a', `${sessionId}.jsonl`) };
  await ledger.recordSource(saved.requestId, source); await ledger.ready(saved.requestId);
  const reopened = new AuthoringSessionIntents(root);
  assert.deepEqual((await reopened.get(saved.requestId))?.source, source);
  await assert.rejects(reopened.recordSource(saved.requestId, { ...source, transcriptPath: join(root, 'b', `${sessionId}.jsonl`) }), /source changed/i);
  await reopened.complete({ projectPath, sessionId });
  assert.equal((await reopened.pending()).length, 0);
});

test('a corrupt session creation intent fails closed without overwriting its file', async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-session-bad-intent-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const path = join(root, 'authoring-session-intents.v1.json');
  const content = '{"version":1,"entries":[{"requestId":"bad"}]}'; await fs.writeFile(path, content);
  await assert.rejects(new AuthoringSessionIntents(root).create({ projectPath: join(root, 'project'), sessionId: randomUUID() },
    { name: 'App', setCurrent: false }), /invalid.*intent/i);
  assert.equal(await fs.readFile(path, 'utf8'), content);
});
