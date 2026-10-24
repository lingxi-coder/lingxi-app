import { test } from 'node:test';
import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import {
  LocalAppAgentBroker,
  type LocalAppAgentAuthorization,
  type LocalAppAgentBrokerOptions,
  type LocalAppAgentExecution,
  type LocalAppAgentOwner,
} from '../src/main/local-apps/agentBroker.ts';
import { parseAgentRunInput, sameAgentGrant } from '../src/main/local-apps/agentPolicy.ts';
import type { LocalAppAgentEvent, LocalAppAgentTask, LocalAppAgentToolGrant } from '../src/shared/localAppAgent.ts';

const principal: LocalAppAgentOwner = { appId: 'a11111111111111111111111111111111', releaseDigest: '1'.repeat(64), invocationId: 'invocation-1', webContentsId: 123 };
const other: LocalAppAgentOwner = { appId: 'a22222222222222222222222222222222', releaseDigest: '2'.repeat(64), invocationId: 'invocation-2' };
const read: LocalAppAgentToolGrant = { id: 'builtin:Read', kind: 'builtin', name: 'Read' };
const skill = { id: 'nutrition', path: 'skills/nutrition/SKILL.md' };
const input = { requestId: 'request-1', prompt: 'Describe the supplied test object.' };

function auth(): LocalAppAgentAuthorization {
  return {
    settings: { enabled: true, model: { providerId: 'test-provider', modelId: 'test-model' }, tools: [read], skills: ['nutrition'], background: false, maxTurns: 20, timeoutMs: 600_000 },
    requestedTools: [read.id], requestedSkills: [skill.id], releaseRevision: 1, releaseSourceDirectory: '/release/source',
    skillSpecs: [skill], releaseFiles: [{ path: skill.path, sha256: 'a'.repeat(64), bytes: 18, executable: false }],
    model: { providerId: 'test-provider', modelId: 'test-model', label: 'Test', canSelect: true, supportsAttachments: true, supportsVision: true, supportsStructuredOutput: true },
  };
}
function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}
async function fixture(t: { after(callback: () => Promise<void>): void }, overrides: Partial<LocalAppAgentBrokerOptions> = {}) {
  const root = await fs.mkdtemp(join(tmpdir(), 'lingxi-agent-broker-'));
  const options: LocalAppAgentBrokerOptions = { root, authorize: async () => auth(), revalidate: async () => true,
    execute: async () => ({ result: { text: 'done', attachments: [] }, usage: { turns: 1 } }), ...overrides };
  const broker = new LocalAppAgentBroker(options);
  t.after(async () => {
    await broker.dispose().catch(() => undefined);
    await fs.rm(root, { recursive: true, force: true });
  });
  return { root, broker, options };
}
async function terminal(broker: LocalAppAgentBroker, caller: LocalAppAgentOwner, runId: string): Promise<LocalAppAgentTask> {
  const done = deferred<LocalAppAgentTask>();
  const timeout = setTimeout(() => done.reject(new Error('Task did not finish')), 3000);
  const unsubscribe = await broker.subscribe(caller, runId, event => {
    if (event.type === 'state' && ['succeeded', 'failed', 'cancelled', 'interrupted'].includes(event.task.state)) done.resolve(event.task);
  });
  try { return await done.promise; } finally { clearTimeout(timeout); unsubscribe(); }
}

test('idempotent task admission keeps one execution and task events use the public state envelope', async t => {
  const started = deferred<LocalAppAgentExecution>(), completion = deferred<{ result: { text: string; attachments: [] }; usage: { inputTokens: number } }>();
  let calls = 0;
  const { broker } = await fixture(t, { execute: execution => { calls++; started.resolve(execution); return completion.promise; } });
  const [one, two] = await Promise.all([broker.run(principal, input), broker.run(principal, input)]);
  assert.equal(one.runId, two.runId);
  const execution = await started.promise;
  assert.equal(calls, 1);
  await assert.rejects(broker.run(principal, { ...input, prompt: 'Different side effect' }), { code: 'request_conflict' });
  const events: LocalAppAgentEvent[] = [];
  const unsubscribe = await broker.subscribe(principal, one.runId, event => events.push(event));
  completion.resolve({ result: { text: 'done', attachments: [] }, usage: { inputTokens: 0 } });
  const done = await terminal(broker, principal, one.runId);
  unsubscribe();
  assert.equal(done.state, 'succeeded');
  assert.deepEqual(done.usage, { inputTokens: 0 });
  assert.ok(events.some(event => event.type === 'state' && event.task.state === 'succeeded'));
  assert.ok(events.every(event => event.type !== 'state' || (event.runId === one.runId && event.sequence === event.task.sequence && event.time === event.task.updatedAt)));
  assert.deepEqual(await broker.run(principal, input), one);
  assert.equal(calls, 1);
  assert.equal(execution.owner.releaseDigest, principal.releaseDigest);
});

test('durable task metadata is app-scoped and does not persist prompt or attachment bytes', async t => {
  const source = Buffer.from('private attachment bytes');
  const transportInput = { ...input, prompt: 'PRIVATE_PROMPT_SENTINEL', attachments: [
    { name: 'note.txt', mediaType: 'text/plain', size: source.length, bytes: source.buffer.slice(source.byteOffset, source.byteOffset + source.byteLength) },
  ] };
  const parsed = parseAgentRunInput(transportInput);
  const seen = deferred<LocalAppAgentExecution>();
  const { root, broker } = await fixture(t, { execute: async execution => { seen.resolve(execution); return { result: { text: 'answer', attachments: [] }, usage: { turns: 1 } }; } });
  const { runId } = await broker.run(principal, transportInput);
  const execution = await seen.promise;
  assert.equal(execution.input.attachments?.[0]?.base64, source.toString('base64'));
  const taskPath = join(root, 'apps', principal.appId, 'agent', 'tasks', `${runId}.json`);
  const persisted = await fs.readFile(taskPath, 'utf8');
  assert.ok(!persisted.includes('PRIVATE_PROMPT_SENTINEL'));
  assert.ok(!persisted.includes(source.toString('base64')));
  assert.ok(!persisted.includes('webContentsId'));
  assert.equal((await terminal(broker, principal, runId)).state, 'succeeded');
  await assert.rejects(broker.get(other, runId), { code: 'not_found' });
});

test('persistent App Chats use stable host IDs and return durable chat and message provenance', async t => {
  const started = deferred<LocalAppAgentExecution>(), completion = deferred<{ result: { text: string; output?: unknown; attachments: [] }; usage?: { inputTokens: number; turns: number } }>();
  const { root, broker } = await fixture(t, { execute: execution => { started.resolve(execution); return completion.promise; } });
  const summary = await broker.createChat(principal, { title: 'Planning' });
  assert.equal(summary.state, 'idle');
  assert.ok(!('sessionId' in summary));
  const admitted = await broker.sendChat(principal, summary.chatId, { ...input, skills: ['nutrition'] });
  const execution = await started.promise;
  assert.equal(execution.chat?.chatId, summary.chatId);
  assert.equal(execution.chat?.sessionDirectory, join(root, 'apps', principal.appId, 'agent', 'chats', summary.chatId));
  assert.ok(execution.chat?.sessionId);
  const before = await broker.chatHistory(principal, summary.chatId);
  assert.equal(before.messages[0]?.role, 'user');
  assert.equal(before.messages[0]?.runId, admitted.runId);
  assert.equal(before.messages[0]?.text, input.prompt);
  completion.resolve({ result: { text: 'Persistent answer', output: { planned: true }, attachments: [] }, usage: { inputTokens: 13, turns: 1 } });
  const task = await terminal(broker, principal, admitted.runId);
  assert.equal(task.result?.chatId, summary.chatId);
  assert.ok(task.result?.messageId);
  assert.equal(task.usage?.inputTokens, 13);
  const history = await broker.chatHistory(principal, summary.chatId);
  assert.equal(history.messages.length, 2);
  assert.equal(history.messages[1]?.messageId, task.result?.messageId);
  assert.equal(history.messages[1]?.runId, admitted.runId);
  const publicChat = await broker.getChat(principal, summary.chatId);
  assert.ok(!('sessionId' in publicChat));
  const chatDisk = await fs.readFile(join(root, 'apps', principal.appId, 'agent', 'chats', summary.chatId, 'chat.json'), 'utf8');
  assert.ok(chatDisk.includes(execution.chat!.sessionId), 'the private read-model stores its Harness transcript identity');
  assert.deepEqual(await broker.archiveChat(principal, summary.chatId), undefined);
  await assert.rejects(broker.sendChat(principal, summary.chatId, { ...input, requestId: 'archived' }), { code: 'chat_archived' });
});

test('chat history excludes the private session identifier and deletion serializes against admissions', async t => {
  const { broker } = await fixture(t);
  const chat = await broker.createChat(principal);
  await broker.deleteChat(principal, chat.chatId);
  await assert.rejects(broker.chatHistory(principal, chat.chatId), { code: 'not_found' });
});

test('request tools and Skills must be declared and explicitly granted, with host limits only reducible', async t => {
  const state = auth(), seen: LocalAppAgentExecution[] = [];
  const { broker } = await fixture(t, { authorize: async () => state, execute: async execution => { seen.push(execution); return { result: { text: '', attachments: [] }, usage: { turns: 1 } }; } });
  await assert.rejects(broker.run(principal, { ...input, tools: ['builtin:Bash'] }), { code: 'tool_not_granted' });
  await assert.rejects(broker.run(principal, { ...input, skills: ['undeclared'] }), { code: 'skill_not_granted' });
  state.requestedTools = [];
  await assert.rejects(broker.run(principal, { ...input, tools: [read.id] }), { code: 'tool_not_granted' });
  state.requestedTools = [read.id];
  for (const overrides of [{ maxTurns: 21 }, { timeoutMs: 600_001 }]) await assert.rejects(broker.run(principal, { ...input, ...overrides }), { code: 'limit_exceeded' });
  const { runId } = await broker.run(principal, { ...input, maxTurns: 3, timeoutMs: 5000, tools: [read.id], skills: ['nutrition'] });
  const done = await terminal(broker, principal, runId);
  assert.equal(done.state, 'succeeded');
  assert.deepEqual(seen[0]?.authorization.settings.tools, [read]);
  assert.deepEqual(seen[0]?.authorization.settings.skills, ['nutrition']);
  assert.equal(seen[0]?.authorization.settings.maxTurns, 3);
  assert.equal(seen[0]?.authorization.settings.timeoutMs, 5000);
});

test('authorization revocation cancels a task before later tool dispatch', async t => {
  const started = deferred<LocalAppAgentExecution>(), proceed = deferred<void>();
  let dispatched = false;
  const state = auth();
  const { broker } = await fixture(t, { authorize: async () => state,
    revalidate: async (_owner, authorization) => authorization.settings.tools.every(tool => state.settings.tools.some(current => sameAgentGrant(tool, current))),
    execute: async execution => { started.resolve(execution); await execution.emit({ phase: 'waiting_for_permission' }); await proceed.promise;
      await execution.assertAuthorized(); dispatched = true; return { result: { text: '', attachments: [] } }; } });
  const { runId } = await broker.run(principal, { ...input, tools: [read.id] });
  await started.promise;
  state.settings.tools = [];
  proceed.resolve();
  await broker.cancelApp(principal.appId, 'The grant changed');
  assert.equal(dispatched, false);
  assert.equal((await broker.get(principal, runId)).state, 'cancelled');
});

test('malformed files, renderer paths and invalid schema references are rejected before admission', () => {
  const bytes = Buffer.from('x');
  assert.throws(() => parseAgentRunInput({ ...input, attachments: [{ name: '../secret.txt', mediaType: 'text/plain', size: 1, bytes: bytes.buffer }]}), { code: 'invalid_attachment' });
  assert.throws(() => parseAgentRunInput({ ...input, attachments: [{ path: '/private/photo.png' }]}), { code: 'invalid_attachment' });
  assert.throws(() => parseAgentRunInput({ ...input, outputSchema: { $ref: 'https://private.example/schema' } }), { code: 'invalid_schema' });
});

test('successful tasks require actual valid usage while allowing unknown usage fields to be omitted', async t => {
  const { broker } = await fixture(t, { execute: async () => ({ result: { text: 'answer', attachments: [] } }) });
  const missing = await broker.run(principal, { ...input, requestId: 'usage-missing' });
  const missingResult = await terminal(broker, principal, missing.runId);
  assert.equal(missingResult.state, 'failed');
  assert.equal(missingResult.error?.code, 'invalid_result');
  assert.equal(missingResult.usage, undefined);

  const { broker: validBroker } = await fixture(t, { execute: async () => ({ result: { text: 'answer', attachments: [] }, usage: {} }) });
  const accepted = await validBroker.run(principal, { ...input, requestId: 'usage-unknown' });
  const acceptedResult = await terminal(validBroker, principal, accepted.runId);
  assert.equal(acceptedResult.state, 'succeeded');
  assert.deepEqual(acceptedResult.usage, {});
});
