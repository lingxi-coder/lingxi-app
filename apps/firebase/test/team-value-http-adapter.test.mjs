// All API/ledger responses here are injected fixtures, never real-provider evidence.
import test from 'node:test';
import assert from 'node:assert/strict';
import { createHttpTransport, createAdapter, tokenFromFile } from '../scripts/team-value-http-adapter.mjs';
import { mkdtemp, writeFile, chmod, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { digest } from '../scripts/team-value-fixtures.mjs';
import { SharedBotsStore } from '../src/shared-bots.mjs';

const config = () => ({ runId: 'qa_run_0001', binding: { generation: 1, conversationId: 'qa_chat_01', sharedBotId: 'qa_bot_001' },
  http: { baseUrl: 'https://qa.example.test/teamsApi', requesterUid: 'qa_member_01', scope: { kind: 'team', id: 'qa_team_01' } } });
const requester = { uid: 'qa_member_01', generation: 1 }, packageInfo = { revision: 'qa-pkg-r1', sha256: 'a'.repeat(64), sharedBotRevision: 1, evidenceRef: 'pkg-read-receipt' };
function setup() {
  const c = config(), calls = [];
  const localPackage = structuredClone(packageInfo);
  const task = { taskId: 'qa_task_01', conversationId: c.binding.conversationId, sharedBotId: c.binding.sharedBotId, requester, state: 'completed', replyId: 'qa_reply_01' };
  const provider = { source: 'real_process_verified_route', id: 'mocked-provider', model: 'mocked-model', requestId: 'request_01', taskId: task.taskId, evidenceRef: 'mock-provider-source' };
  const inspection = { evidenceKind: 'real_process_verified_route', provenance: 'fixture', inspectedAt: Date.now(), scope: c.http.scope, requester,
    ...c.binding, isolatedQa: true, package: localPackage, provider,
    limits: { context_window_tokens: 2000000, output_tokens_per_request: 4096, physical_dispatches: 32 } };
  const evidence = { evidenceKind: 'real_process_verified_route', provenance: 'fixture', scope: c.http.scope, taskId: task.taskId, requester,
    conversationId: c.binding.conversationId, sharedBotId: c.binding.sharedBotId, replyId: task.replyId,
    package: localPackage, provider, textDigest: digest('fixture answer'), dispatchedRequests: 1, usage: { status: 'complete', inputTokens: 10, outputTokens: 5 }, evidenceRef: 'mock-task-ledger' };
  const messages = [{ id: task.replyId, taskId: task.taskId, author: { kind: 'shared_bot', sharedBotId: task.sharedBotId }, text: 'fixture answer' }];
  const receiptReader = { async inspect() { return inspection; }, async readTask() { return evidence; } };
  const fetcher = async (url, options) => {
    const body = options.body ? JSON.parse(options.body) : undefined;
    calls.push({ url, body, options });
    let value;
    if (options.method === 'GET') value = { team: { id: c.http.scope.id }, members: [requester] };
    else if (body.action === 'listSharedBots') value = { bots: [{ sharedBotId: task.sharedBotId, permitted: true, ready: true, state: 'published', revision: 1, quotas: { team: 200, member: 200, bot: 200 }, dailyRemaining: 200 }] };
    else if (body.action === 'getTask') value = task;
    else if (body.action === 'submit') value = { tasks: [{ taskId: task.taskId }] };
    else if (body.action === 'sync') value = body.after === 0 ? { changes: [{ seq: 100 }], messages: [], hasMore: true } : { changes: [{ seq: 101 }], messages, hasMore: false };
    else assert.fail('unexpected mutation');
    return new Response(JSON.stringify(value), { status: 200 });
  };
  return { c, calls, task, inspection, evidence, messages, receiptReader, fetcher };
}
test('token source is explicit, permissions enforced, and rotated token is reread', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'team-value-token-test-'));
  try {
    const path = join(dir, 'token');
    await writeFile(path, 'synthetic-token-one', { mode: 0o600 });
    assert.equal(await tokenFromFile(path), 'synthetic-token-one');
    await writeFile(path, JSON.stringify({ idToken: 'synthetic-token-two' }));
    assert.equal(await tokenFromFile(path), 'synthetic-token-two');
    await chmod(path, 0o644);
    await assert.rejects(tokenFromFile(path), /UNSAFE_TOKEN_FILE/u);
  } finally { await rm(dir, { recursive: true }); }
});
test('actual transport uses existing path, bearer and envelope, and never retries failed submit', async () => {
  let calls = 0;
  const transport = createHttpTransport({ baseUrl: 'https://qa.example.test/teamsApi', getToken: async () => 'synthetic-token',
    fetcher: async (url, options) => {
      calls++; assert.equal(url, 'https://qa.example.test/teamsApi/v1/teams/messages');
      assert.equal(options.headers.Authorization, 'Bearer synthetic-token');
      assert.equal(options.redirect, 'error'); assert.equal(JSON.parse(options.body).commandId, 'original_command');
      return new Response('private error body with credential', { status: 503 });
    } });
  await assert.rejects(transport('/v1/teams/messages', { commandId: 'original_command' }), { message: 'QA_HTTP_503' });
  assert.equal(calls, 1);
});
test('auth/network errors do not surface secrets or response bodies', async () => {
  const auth = createHttpTransport({ baseUrl: 'https://qa.example.test', getToken: async () => { throw new Error('secret-token'); } });
  await assert.rejects(auth('/v1/teams/messages', {}), { message: 'QA_AUTH_UNAVAILABLE' });
  const network = createHttpTransport({ baseUrl: 'https://qa.example.test', getToken: async () => 'synthetic', fetcher: async () => { throw new Error('secret-url'); } });
  await assert.rejects(network('/v1/teams/messages', {}), { message: 'QA_TRANSPORT_FAILED' });
  assert.throws(() => createHttpTransport({ baseUrl: 'http://public.example.test', getToken: async () => 'x' }), /INVALID_API_BASE/u);
});
test('live string alone cannot enable a run; absence of actual host receipts fails before submit', async () => {
  const s = setup(); s.c.mode = 'live';
  const adapter = await createAdapter({ config: s.c, getToken: async () => 'synthetic', fetcher: s.fetcher });
  await assert.rejects(adapter.preflight(), /ACTUAL_RUN_TEAM_HOST_RECEIPTS_UNAVAILABLE/u);
  await assert.rejects(adapter.submit({}), /PREFLIGHT_REQUIRED/u);
  assert.ok(!s.calls.some(c => c.body?.action === 'submit'));
});
test('preflight requires actual scoped process receipts and per-task limits', async () => {
  for (const mutate of [s => { s.inspection.provider.source = 'configured-model'; }, s => { s.inspection.limits.output_tokens_per_request = 0; },
    s => { s.inspection.evidenceKind = 'operator-json'; }, s => { s.inspection.package.sharedBotRevision = 2; }]) {
    const s = setup(); mutate(s);
    const adapter = await createAdapter({ config: s.c, getToken: async () => 'synthetic', fetcher: s.fetcher, receiptReader: s.receiptReader });
    await assert.rejects(adapter.preflight());
  }
});
test('injected API and ledger fixtures exercise unique submit and complete paginated reply identity checks', async () => {
  const s = setup(), adapter = await createAdapter({ config: s.c, getToken: async () => 'synthetic', fetcher: s.fetcher, receiptReader: s.receiptReader });
  const p = await adapter.preflight(); assert.equal(p.quotas.remaining.bot, 200);
  const command = { schemaVersion: 1, commandId: 'original_command', teamId: s.c.http.scope.id, generation: 1, action: 'submit',
    payload: { conversationId: s.c.binding.conversationId, targets: [s.c.binding.sharedBotId], text: 'synthetic prompt', messageId: 'original_message', changeId: 'original_change' } };
  assert.deepEqual(await adapter.submit(command), { taskId: s.task.taskId });
  assert.deepEqual(s.calls.find(c => c.body?.action === 'submit').body, command);
  const result = await adapter.poll(s.task.taskId);
  assert.equal(result.text, 'fixture answer'); assert.equal(result.provider.requestId, 'request_01');
  assert.ok(s.calls.some(c => c.body?.action === 'sync' && c.body.after === 100));
  assert.ok(!JSON.stringify(p).includes('synthetic-token'));
});
test('a valid-looking answer from the wrong task/author/requester/provider receipt is refused', async () => {
  for (const mutate of [s => { s.messages[0].taskId = 'other_task_01'; }, s => { s.messages[0].author.sharedBotId = 'other_bot_01'; },
    s => { s.evidence.taskId = 'other_task_01'; }, s => { s.evidence.requester = { uid: 'other', generation: 1 }; },
    s => { s.evidence.textDigest = 'wrong-answer-digest'; }, s => { s.evidence.dispatchedRequests = 0; }]) {
    const s = setup(), adapter = await createAdapter({ config: s.c, getToken: async () => 'synthetic', fetcher: s.fetcher, receiptReader: s.receiptReader });
    await adapter.preflight(); mutate(s);
    await assert.rejects(adapter.poll(s.task.taskId));
  }
});
test('HTTP positive fixture uses current SharedBotsStore.botMessage taskId DTO; sourceTaskId alone is refused', async () => {
  const s = setup(), written = [];
  const ref = { collection: name => ({ doc: value => ({ name, id: value }) }) };
  new SharedBotsStore(undefined).botMessage({ create(target, value) { written.push({ target, value }); }, update() {} },
    { conversation: { ref }, c: { lastSeq: 0, lastChangeSeq: 0 } }, { ...s.task, messageId: 'request_message_01' },
    { name: 'Fixture bot' }, requester, Date.now(), { messageId: s.task.replyId, changeId: 'reply_change_01', text: 'fixture answer', fields: { deliveryId: 'delivery_01' } });
  const stored = written.find(row => row.target.name === 'messages');
  assert.equal(stored.value.taskId, s.task.taskId); assert.ok(!('sourceTaskId' in stored.value));
  s.messages[0] = { id: stored.target.id, ...stored.value };
  const adapter = await createAdapter({ config: s.c, getToken: async () => 'synthetic', fetcher: s.fetcher, receiptReader: s.receiptReader });
  await adapter.preflight(); assert.equal((await adapter.poll(s.task.taskId)).resultId, s.task.replyId);
  s.messages[0].sourceTaskId = s.task.taskId; delete s.messages[0].taskId;
  await assert.rejects(adapter.poll(s.task.taskId), /REPLY_IDENTITY_MISMATCH/u);
});

test('HTTP receipt adapter permits Missing/Partial usage and missing provider request ID with real-process evidence', async () => {
  for (const usage of [{ status: 'missing' }, { status: 'partial', inputTokens: 4, outputTokens: 2 }]) {
    const s = setup(), adapter = await createAdapter({ config: s.c, getToken: async () => 'synthetic', fetcher: s.fetcher, receiptReader: s.receiptReader });
    s.evidence.usage = usage; s.evidence.provider.requestId = null;
    const p = await adapter.preflight(); assert.equal(p.mode, 'fixture');
    const r = await adapter.poll(s.task.taskId);
    assert.deepEqual(r.usage, usage); assert.equal(r.provider.requestIdStatus, 'missing');
    assert.equal(r.dispatchedRequests, 1);
  }
});
test('API accumulated Partial usage remains Partial even when the last worker attempt reported complete', async () => {
  const s = setup();
  s.task.usage = { status: 'partial', inputTokens: 15, outputTokens: 7 };
  const adapter = await createAdapter({ config: s.c, getToken: async () => 'synthetic', fetcher: s.fetcher, receiptReader: s.receiptReader });
  await adapter.preflight();
  assert.deepEqual((await adapter.poll(s.task.taskId)).usage, s.task.usage);
});
