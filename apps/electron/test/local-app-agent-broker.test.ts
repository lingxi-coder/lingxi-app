import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { BaseEvent } from '@ag-ui/core';
import { EventSchema, MessageSchema } from '@lingxi/local-app-agui/client';
import {
  LocalAppAgentBroker,
  type LocalAppAgentAuthorization,
  type LocalAppAgentBrokerOptions,
  type LocalAppAgentExecution,
  type LocalAppAgentOwner,
  type LocalAppAgentPrivateResult,
} from '../src/main/local-apps/agentBroker.ts';
import { agentDigest, parseRunAgentInput, sameAgentGrant } from '../src/main/local-apps/agentPolicy.ts';
import type { LocalAppAgentTask, LocalAppAgentToolGrant } from '../src/shared/localAppAgent.ts';

const AGUI_FIXTURE = JSON.parse(readFileSync(new URL('../vendor/local-app-agui/spec/fixtures/run-agent-input.ag-ui-1.0.3.json', import.meta.url), 'utf8')) as any;
const principal: LocalAppAgentOwner = { appId: 'a11111111111111111111111111111111', releaseDigest: '1'.repeat(64), invocationId: 'invocation-1', webContentsId: 123 };
const other: LocalAppAgentOwner = { appId: 'a22222222222222222222222222222222', releaseDigest: '2'.repeat(64), invocationId: 'invocation-2' };
const readGrant: LocalAppAgentToolGrant = { id: 'builtin:Read', kind: 'builtin', name: 'Read' };
const skill = { id: 'extract-text', path: 'skills/extract-text/SKILL.md' };

function runInput(runId: string, threadId = `thread-${runId}`): any {
  const input = structuredClone(AGUI_FIXTURE);
  input.runId = runId;
  input.threadId = threadId;
  return input;
}
function auth(): LocalAppAgentAuthorization {
  return {
    settings: { enabled: true, model: { providerId: 'test-provider', modelId: 'test-model' }, tools: [readGrant], skills: [skill.id], background: false, maxTurns: 20, timeoutMs: 600_000 },
    requestedTools: [readGrant.id], requestedSkills: [skill.id], releaseRevision: 1, releaseSourceDirectory: '/release/source',
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
  const root = await fs.mkdtemp(join(tmpdir(), 'lingxi-agui-broker-'));
  const options: LocalAppAgentBrokerOptions = {
    root, authorize: async () => auth(), revalidate: async () => true,
    execute: async execution => ({ result: { text: 'done', output: { ok: true }, attachments: [], messageId: `assistant-${execution.runId}`,
      ...(execution.chat ? { chatId: execution.chat.chatId } : {}) }, usage: { inputTokens: 8, outputTokens: 2, turns: 1 } }),
    ...overrides,
  };
  const broker = new LocalAppAgentBroker(options);
  t.after(async () => { await broker.dispose().catch(() => undefined); await fs.rm(root, { recursive: true, force: true }); });
  return { root, broker, options };
}
async function terminal(broker: LocalAppAgentBroker, caller: LocalAppAgentOwner, runId: string): Promise<LocalAppAgentTask> {
  const end = Date.now() + 3000;
  while (Date.now() < end) {
    const task = await broker.get(caller, runId);
    if (['succeeded', 'failed', 'cancelled', 'interrupted'].includes(task.state)) return task;
    await new Promise(resolve => setTimeout(resolve, 5));
  }
  throw new Error('AG-UI task did not finish');
}

test('official SDK RunAgentInput fixture normalizes in order and retains digest bindings', () => {
  const parsed = parseRunAgentInput(AGUI_FIXTURE);
  assert.equal(parsed.threadId, 'thread-fixture-1');
  assert.equal(parsed.runId, 'run-fixture-1');
  assert.equal(parsed.executionInput.prompt, 'Inspect this image.');
  assert.deepEqual(parsed.executionInput.contentParts, [
    { kind: 'text', text: 'Inspect this image.' },
    { kind: 'attachment', name: 'photo.png', mediaType: 'image/png', base64: 'iVBORw==' },
  ]);
  assert.equal(parsed.executionInput.protocolThreadId, parsed.threadId);
  assert.equal(parsed.executionInput.protocolInputDigest, agentDigest(AGUI_FIXTURE));
  assert.match(parsed.executionInput.protocolMessageSha256, /^[a-f0-9]{64}$/);
});

test('stateless runs reject browser supplied history and unsupported protocol controls', () => {
  const base = runInput('unsupported-controls');
  assert.throws(() => parseRunAgentInput({ ...base, messages: [...base.messages, { id: 'second', role: 'user', content: 'two' }] }), { code: 'stateless_history_unsupported' });
  assert.throws(() => parseRunAgentInput({ ...base, parentRunId: 'parent' }), { code: 'unsupported_agent_input' });
  assert.throws(() => parseRunAgentInput({ ...base, context: [{ description: 'ambient', value: 'untrusted' }] }), { code: 'context_unsupported' });
  assert.throws(() => parseRunAgentInput({ ...base, resume: [{ interruptId: 'interrupt-1', status: 'resolved', payload: { answer: 'yes' } }] }), { code: 'resume_unsupported' });
  assert.throws(() => parseRunAgentInput({ ...base, forwardedProps: { extra: true } }), { code: 'invalid_input' });
  assert.throws(() => parseRunAgentInput({ ...base, forwardedProps: { 'com.lingxi.localApp': { version: 2 } } }), { code: 'invalid_input' });
});

test('idempotent task admission deduplicates by public runId and exposes only standard events', async t => {
  const started = deferred<LocalAppAgentExecution>();
  const completion = deferred<{ result: LocalAppAgentPrivateResult; usage: { inputTokens: number } }>();
  let calls = 0;
  const { broker } = await fixture(t, { execute: execution => { calls++; started.resolve(execution); return completion.promise; } });
  const input = runInput('public-run-1');
  const [first, retry] = await Promise.all([broker.run(principal, input), broker.run(principal, input)]);
  assert.deepEqual(first, retry);
  const execution = await started.promise;
  assert.equal(execution.runId, input.runId);
  assert.match(execution.taskStorageId, /^[0-9a-f-]{36}$/);
  assert.equal(calls, 1);
  await assert.rejects(broker.run(principal, { ...input, forwardedProps: { ...input.forwardedProps, 'com.lingxi.localApp': { ...input.forwardedProps['com.lingxi.localApp'], maxTurns: 1 } } }), { code: 'request_conflict' });

  completion.resolve({ result: { text: 'done', output: { ok: true }, attachments: [], messageId: 'assistant-public-run-1' }, usage: { inputTokens: 0 } });
  const done = await terminal(broker, principal, input.runId);
  assert.equal(done.state, 'succeeded');
  assert.deepEqual(done.usage, { inputTokens: 0 });
  assert.equal(done.threadId, input.threadId);
  assert.equal('result' in done, false);
  assert.equal('requestId' in done, false);
  assert.equal('model' in done, false);
  assert.equal((await broker.run(principal, input)).runId, input.runId, 'an exact terminal retry returns the prior public run');
  assert.equal(calls, 1);

  const page = await broker.events(principal, input.runId);
  assert.ok(page.events.length >= 5);
  assert.equal(page.events[0]?.type, 'RUN_STARTED');
  assert.deepEqual(page.events.filter(event => ['TEXT_MESSAGE_START', 'TEXT_MESSAGE_CONTENT', 'TEXT_MESSAGE_END'].includes(event.type)).map(event => event.type),
    ['TEXT_MESSAGE_START', 'TEXT_MESSAGE_CONTENT', 'TEXT_MESSAGE_END']);
  for (const event of page.events) EventSchema.parse(event);
  const start = page.events[0] as any;
  const finish = page.events.at(-1) as any;
  assert.equal(start.runId, input.runId);
  assert.equal(start.threadId, input.threadId);
  assert.equal(finish.type, 'RUN_FINISHED');
  assert.deepEqual(finish.result, { ok: true });
  const taskPage = await broker.list(principal, { limit: 10 });
  assert.deepEqual(taskPage.tasks.map(task => task.runId), [input.runId]);
  assert.ok(!('result' in taskPage.tasks[0]!));
  assert.equal((await broker.get(other, input.runId).catch(() => undefined)), undefined);
});

test('public run IDs are deduplicated and stored within application ownership', async t => {
  const executions: LocalAppAgentExecution[] = [];
  const { broker } = await fixture(t, { execute: async execution => {
    executions.push(execution);
    return { result: { text: execution.owner.appId, output: { ok: true }, attachments: [], messageId: `assistant-${execution.owner.appId}` }, usage: {} };
  } });
  const input = runInput('same-public-run');
  const [first, second] = await Promise.all([broker.run(principal, input), broker.run(other, input)]);
  assert.equal(first.runId, second.runId);
  assert.equal((await terminal(broker, principal, first.runId)).state, 'succeeded');
  assert.equal((await terminal(broker, other, second.runId)).state, 'succeeded');
  assert.equal(executions.length, 2);
  assert.notEqual(executions[0]?.taskStorageId, executions[1]?.taskStorageId);
  assert.equal((await broker.list(principal)).tasks.length, 1);
  assert.equal((await broker.list(other)).tasks.length, 1);
  assert.equal((await broker.events(principal, first.runId)).events.at(-1)?.type, 'RUN_FINISHED');
  assert.equal((await broker.events(other, second.runId)).events.at(-1)?.type, 'RUN_FINISHED');
});

test('event cursors replay bounded official pages and reconnect after host restart', async t => {
  const { root, broker, options } = await fixture(t);
  const input = runInput('cold-run');
  await broker.run(principal, input);
  await terminal(broker, principal, input.runId);
  const first = await broker.events(principal, input.runId, 0);
  assert.equal(first.events[0]?.type, 'RUN_STARTED');
  const next = await broker.events(principal, input.runId, 1);
  assert.ok(next.sequence > 1);
  assert.equal(first.events.length, next.events.length + 1);
  await broker.dispose();
  const resumed = new LocalAppAgentBroker(options);
  t.after(async () => { await resumed.dispose().catch(() => undefined); await fs.rm(root, { recursive: true, force: true }); });
  const task = await resumed.get(principal, input.runId);
  assert.equal(task.state, 'succeeded');
  const replay = await resumed.events(principal, input.runId, 0);
  assert.equal(replay.sequence, first.sequence);
  assert.ok(replay.events.some(event => event.type === 'RUN_FINISHED'));
  await assert.rejects(resumed.events(principal, input.runId, 10_000), { code: 'invalid_input' });
});

test('durable broker replays a long AG-UI journal in bounded pages after restart', async t => {
  const structuredResult = Array.from({ length: 200 }, (_, index) => ({ index }));
  const { root, broker, options } = await fixture(t, {
    execute: async execution => ({
      result: { text: 'done', output: structuredResult, attachments: [], messageId: 'assistant-' + execution.runId },
      usage: { turns: 1 },
    }),
  });
  const input = runInput('long-event-journal');
  await broker.run(principal, input);
  await terminal(broker, principal, input.runId);

  const taskDirectory = join(root, 'apps', principal.appId, 'agent', 'tasks');
  const taskFiles = await fs.readdir(taskDirectory);
  assert.equal(taskFiles.length, 1);
  const taskPath = join(taskDirectory, taskFiles[0]!);
  const ledger = JSON.parse(await fs.readFile(taskPath, 'utf8'));
  const startedEvent = ledger.events[0] as BaseEvent;
  const terminalEvent = ledger.events.at(-1) as BaseEvent;
  const messageEvents = Array.from({ length: 100 }, (_, index) => {
    const messageId = 'assistant-stream-' + index;
    return [
      { type: 'TEXT_MESSAGE_START', messageId, role: 'assistant' },
      { type: 'TEXT_MESSAGE_CONTENT', messageId, delta: 'chunk-' + index },
      { type: 'TEXT_MESSAGE_END', messageId },
    ];
  }).flat() as BaseEvent[];
  const expectedEvents = [
    startedEvent,
    ...messageEvents,
    { ...terminalEvent, result: structuredResult },
  ] as BaseEvent[];
  for (const event of expectedEvents) EventSchema.parse(event);
  ledger.events = expectedEvents;
  ledger.eventSequence = expectedEvents.length;

  await broker.dispose();
  await fs.writeFile(taskPath, JSON.stringify(ledger), 'utf8');
  const reopened = new LocalAppAgentBroker(options);
  try {
    assert.equal((await reopened.get(principal, input.runId)).state, 'succeeded');
    const pages: Array<{ events: BaseEvent[]; sequence: number }> = [];
    let afterSequence = 0;
    while (afterSequence < expectedEvents.length) {
      const page = await reopened.events(principal, input.runId, afterSequence);
      assert.ok(page.events.length <= 128);
      assert.equal(page.sequence, afterSequence + page.events.length);
      for (const event of page.events) EventSchema.parse(event);
      pages.push(page);
      afterSequence = page.sequence;
    }

    assert.deepEqual(pages.map(page => page.events.length), [128, 128, 46]);
    assert.deepEqual(pages.flatMap(page => page.events), expectedEvents);
    assert.deepEqual((pages.at(-1)?.events.at(-1) as any).result, structuredResult);
    assert.equal(pages.flatMap(page => page.events).filter(event => event.type === 'RUN_FINISHED').length, 1);
  } finally {
    await reopened.dispose();
  }
});

test('durable chats persist standard Message content and accept exact terminal retries after history advances', async t => {
  let calls = 0;
  const { root, broker, options } = await fixture(t, { execute: async execution => {
    calls++;
    return { result: { text: `answer-${calls}`, output: { ok: true }, attachments: [], messageId: `assistant-${execution.runId}`,
      ...(execution.chat ? { chatId: execution.chat.chatId } : {}) }, usage: { turns: 1 } };
  } });
  const chat = await broker.createChat(principal, { title: 'Planning' });
  assert.ok(!('sessionId' in chat));
  const firstInput = runInput('chat-run-1', chat.chatId);
  const firstRun = await broker.sendChat(principal, chat.chatId, firstInput);
  await terminal(broker, principal, firstRun.runId);
  assert.equal(calls, 1);
  const firstHistory = await broker.chatHistory(principal, chat.chatId, { limit: 1 });
  assert.equal(firstHistory.messages[0]?.id, 'message-fixture-1');
  assert.equal(firstHistory.messages[0]?.role, 'user');
  assert.ok(Array.isArray(firstHistory.messages[0]?.content));
  assert.ok(firstHistory.nextCursor);
  const assistantPage = await broker.chatHistory(principal, chat.chatId, { cursor: firstHistory.nextCursor, limit: 1 });
  assert.equal(assistantPage.messages[0]?.id, 'assistant-chat-run-1');
  assert.equal(assistantPage.messages[0]?.role, 'assistant');
  MessageSchema.parse(assistantPage.messages[0]);

  const exactRetry = await broker.run(principal, firstInput);
  assert.deepEqual(exactRetry, firstRun, 'dedupe runs before the now-advanced canonical history check');
  assert.equal(calls, 1);

  const newUser = { id: 'chat-user-2', role: 'user', content: 'Summarize the image in one sentence.' };
  const secondInput = { ...runInput('chat-run-2', chat.chatId), messages: [...(await broker.chatHistory(principal, chat.chatId)).messages, newUser] };
  const secondRun = await broker.sendChat(principal, chat.chatId, secondInput);
  await terminal(broker, principal, secondRun.runId);
  assert.equal(calls, 2);
  assert.equal((await broker.chatHistory(principal, chat.chatId)).messages.length, 4);
  const finalRunEvents = await broker.events(principal, secondRun.runId);
  assert.equal((finalRunEvents.events.at(-1) as any).type, 'RUN_FINISHED');
  const chats = await broker.listChats(principal);
  assert.deepEqual(chats.chats.map(row => row.chatId), [chat.chatId]);
  assert.ok(!('sessionId' in chats.chats[0]!));

  await broker.dispose();
  const resumed = new LocalAppAgentBroker(options);
  t.after(async () => { await resumed.dispose().catch(() => undefined); await fs.rm(root, { recursive: true, force: true }); });
  const recovered = await resumed.chatHistory(principal, chat.chatId);
  assert.deepEqual(recovered.messages.map(message => message.id), ['message-fixture-1', 'assistant-chat-run-1', 'chat-user-2', 'assistant-chat-run-2']);
  const disk = await fs.readFile(join(root, 'apps', principal.appId, 'agent', 'chats', chat.chatId, 'chat.json'), 'utf8');
  assert.ok(disk.includes('sessionId'));
  assert.ok(!disk.includes('iVBORw=='), 'inline attachment bytes are stored in the private per-chat asset directory');
  assert.ok(await fs.readdir(join(root, 'apps', principal.appId, 'agent', 'chats', chat.chatId, 'assets')).then(rows => rows.length === 1));
});

test('chat rejects a forged supplied history prefix, duplicate user ID and cross-thread input', async t => {
  const { broker } = await fixture(t);
  const chat = await broker.createChat(principal);
  const first = runInput('history-first', chat.chatId);
  const accepted = await broker.sendChat(principal, chat.chatId, first);
  await terminal(broker, principal, accepted.runId);
  const movedRetry = structuredClone(first); movedRetry.threadId = 'another-thread';
  await assert.rejects(broker.run(principal, movedRetry), { code: 'request_conflict' });
  const history = (await broker.chatHistory(principal, chat.chatId)).messages;
  const forged = structuredClone(history);
  (forged[1] as any).content = 'renderer forged assistant text';
  await assert.rejects(broker.sendChat(principal, chat.chatId, { ...runInput('forged-history', chat.chatId), messages: [...forged, { id: 'next', role: 'user', content: 'next' }] }), { code: 'history_mismatch' });
  await assert.rejects(broker.sendChat(principal, chat.chatId, { ...runInput('duplicate-message', chat.chatId), messages: [...history, history[0]] }), { code: 'message_conflict' });
  await assert.rejects(broker.sendChat(principal, chat.chatId, runInput('wrong-thread', 'another-thread')), { code: 'thread_mismatch' });
});

test('a structured null result fails with standard RUN_ERROR and never emits RUN_FINISHED result:null', async t => {
  const { broker } = await fixture(t, { execute: async () => ({ result: { text: '', output: null, attachments: [], messageId: 'assistant-null' }, usage: {} }) });
  const input = runInput('null-structured');
  const { runId } = await broker.run(principal, input);
  const task = await terminal(broker, principal, runId);
  assert.equal(task.state, 'failed');
  const events = (await broker.events(principal, runId)).events;
  const terminalEvent = events.at(-1) as any;
  assert.equal(terminalEvent.type, 'RUN_ERROR');
  assert.equal(terminalEvent.code, 'structured_output_not_representable');
  assert.ok(!events.some(event => event.type === 'RUN_FINISHED' && 'result' in event && event.result === null));
  for (const event of events) EventSchema.parse(event);
});

test('tool and Skill IDs must be declared and host-granted, and applications may only lower host limits', async t => {
  const state = auth(), seen: LocalAppAgentExecution[] = [];
  const { broker } = await fixture(t, { authorize: async () => state, execute: async execution => { seen.push(execution); return {
    result: { text: 'done', output: { ok: true }, attachments: [], messageId: 'assistant-limits' }, usage: {},
  }; } });
  const toolInput = runInput('bad-tool'); toolInput.forwardedProps['com.lingxi.localApp'].tools = ['builtin:Bash'];
  await assert.rejects(broker.run(principal, toolInput), { code: 'tool_not_granted' });
  const skillInput = runInput('bad-skill'); skillInput.forwardedProps['com.lingxi.localApp'].skills = ['undeclared'];
  await assert.rejects(broker.run(principal, skillInput), { code: 'skill_not_granted' });
  state.requestedTools = [];
  const undeclaredTool = runInput('unrequested-tool');
  await assert.rejects(broker.run(principal, undeclaredTool), { code: 'tool_not_granted' });
  state.requestedTools = [readGrant.id];
  for (const [runId, key, value] of [['too-many-turns', 'maxTurns', 21], ['too-long-timeout', 'timeoutMs', 600_001]] as const) {
    const input = runInput(runId); input.forwardedProps['com.lingxi.localApp'][key] = value;
    await assert.rejects(broker.run(principal, input), { code: 'limit_exceeded' });
  }
  const allowed = runInput('lower-limits');
  allowed.forwardedProps['com.lingxi.localApp'].maxTurns = 3;
  allowed.forwardedProps['com.lingxi.localApp'].timeoutMs = 5000;
  const { runId } = await broker.run(principal, allowed);
  assert.equal((await terminal(broker, principal, runId)).state, 'succeeded');
  assert.deepEqual(seen[0]?.authorization.settings.tools, [readGrant]);
  assert.deepEqual(seen[0]?.authorization.settings.skills, [skill.id]);
  assert.equal(seen[0]?.authorization.settings.maxTurns, 3);
  assert.equal(seen[0]?.authorization.settings.timeoutMs, 5000);
});

test('authorization revocation cancels an active task before later tool dispatch', async t => {
  const started = deferred<LocalAppAgentExecution>(), proceed = deferred<void>();
  let dispatched = false;
  const state = auth();
  const { broker } = await fixture(t, { authorize: async () => state,
    revalidate: async (_owner, authorization) => authorization.settings.tools.every(tool => state.settings.tools.some(current => sameAgentGrant(tool, current))),
    execute: async execution => { started.resolve(execution); await execution.emit({ phase: 'waiting_for_permission' }); await proceed.promise;
      await execution.assertAuthorized(); dispatched = true; return { result: { text: '', output: { ok: true }, attachments: [], messageId: 'assistant-revoked' }, usage: {} }; } });
  const input = runInput('revoke-run'); input.forwardedProps['com.lingxi.localApp'].tools = [readGrant.id];
  const { runId } = await broker.run(principal, input);
  await started.promise;
  state.settings.tools = [];
  proceed.resolve();
  await broker.cancelApp(principal.appId, 'The grant changed');
  assert.equal(dispatched, false);
  assert.equal((await broker.get(principal, runId)).state, 'cancelled');
  const events = await broker.events(principal, runId);
  assert.equal((events.events.at(-1) as any).outcome.type, 'cancelled');
});

test('first AG-UI initialization clears only obsolete Agent ledgers under known app agent folders', async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'lingxi-agent-prototype-clear-'));
  const appPath = join(root, 'apps', principal.appId);
  await fs.mkdir(join(appPath, 'agent', 'tasks'), { recursive: true });
  await fs.mkdir(join(appPath, 'agent', 'chats'), { recursive: true });
  await fs.mkdir(join(appPath, 'agent', 'assets'), { recursive: true });
  await fs.mkdir(join(appPath, 'workspace'), { recursive: true });
  await fs.writeFile(join(appPath, 'agent', 'tasks', 'legacy.json'), '{}');
  await fs.writeFile(join(appPath, 'workspace', 'keep.txt'), 'business data');
  const broker = new LocalAppAgentBroker({ root, authorize: async () => auth(), revalidate: async () => true, execute: async () => { throw new Error('not run'); } });
  t.after(async () => { await broker.dispose().catch(() => undefined); await fs.rm(root, { recursive: true, force: true }); });
  await broker.initialize();
  await assert.rejects(fs.stat(join(appPath, 'agent', 'tasks')));
  await assert.rejects(fs.stat(join(appPath, 'agent', 'chats')));
  assert.equal(await fs.readFile(join(appPath, 'workspace', 'keep.txt'), 'utf8'), 'business data');
});
