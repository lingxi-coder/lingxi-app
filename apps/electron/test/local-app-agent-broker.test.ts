import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, promises as fs } from 'node:fs';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
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
import { LocalAppChatAssetJournal } from '../src/main/local-apps/agentChatAssets.ts';
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

test('settlement subscribers observe success, failure and cancellation after active removal', async t => {
  for (const outcome of ['succeeded', 'failed', 'cancelled'] as const) {
    const started = deferred<void>(), notified = deferred<void>();
    const { broker } = await fixture(t, { execute: async execution => {
      started.resolve();
      if (outcome === 'failed') throw new Error('executor failure');
      if (outcome === 'cancelled') await new Promise<void>((_resolve, reject) => {
        const abort = () => reject(new Error('cancelled'));
        execution.signal.addEventListener('abort', abort, { once: true });
        if (execution.signal.aborted) abort();
      });
      return { result: { text: 'done', output: { ok: true }, attachments: [], messageId: 'assistant' }, usage: { turns: 1 } };
    } });
    let count = 0, settledOwner: LocalAppAgentOwner | undefined, activeAtSettlement: boolean | undefined;
    broker.onSettled(() => { throw new Error('listener failure'); });
    const unsubscribe = broker.onSettled(owner => {
      count++;
      settledOwner = owner;
      activeAtSettlement = broker.hasActiveApp(principal.appId);
      // Reentrant cleanup must not be awaited by the settled execution.
      void broker.cancelApp(principal.appId).then(() => notified.resolve());
    });
    const input = runInput(`settlement-${outcome}`);
    await broker.run(principal, input); await started.promise;
    if (outcome === 'cancelled') await broker.cancel(principal, input.runId);
    await notified.promise;
    assert.equal((await broker.get(principal, input.runId)).state, outcome);
    assert.equal(count, 1);
    assert.deepEqual(settledOwner, { appId: principal.appId, releaseDigest: principal.releaseDigest, invocationId: principal.invocationId });
    assert.equal(activeAtSettlement, false);
    unsubscribe();
  }
});

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
  const chat = await broker.createChat(principal, { requestId: '11111111-1111-4111-8111-111111111111', title: 'Planning' });
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
  const chat = await broker.createChat(principal, { requestId: '22222222-2222-4222-8222-222222222222' });
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

test('unmarked Agent data is preserved and quarantined per App across restarts', async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'lingxi-agent-prototype-preserve-'));
  const appPath = join(root, 'apps', principal.appId);
  const otherPath = join(root, 'apps', other.appId);
  const originalFiles = new Map<string, Buffer>([
    [join(appPath, 'agent', 'tasks', 'legacy.json'), Buffer.from('{"legacy":"task"}\n')],
    [join(appPath, 'agent', 'chats', 'old-chat', 'history.bin'), Buffer.from([0, 255, 16, 42])],
    [join(appPath, 'agent', 'assets', 'image.bin'), Buffer.from([1, 2, 3, 0, 254])],
    [join(appPath, 'workspace', 'keep.txt'), Buffer.from('business data')],
  ]);
  for (const [path, bytes] of originalFiles) { await fs.mkdir(dirname(path), { recursive: true }); await fs.writeFile(path, bytes); }
  for (const group of ['tasks', 'chats', 'assets']) await fs.mkdir(join(otherPath, 'agent', group), { recursive: true });
  const options: LocalAppAgentBrokerOptions = { root, authorize: async () => auth(), revalidate: async () => true,
    execute: async execution => ({ result: { text: 'healthy', output: { ok: true }, attachments: [], messageId: `assistant-${execution.runId}` }, usage: {} }) };
  const broker = new LocalAppAgentBroker(options);
  t.after(async () => { await broker.dispose().catch(() => undefined); await fs.rm(root, { recursive: true, force: true }); });
  const request = { requestId: 'a1111111-1111-4111-8111-111111111111', title: 'Healthy' };
  const unsupportedOperations = [
    () => broker.createChat(principal, request), () => broker.chatCreationStatus(principal, request),
    () => broker.listChats(principal), () => broker.getChat(principal, request.requestId),
    () => broker.chatHistory(principal, request.requestId), () => broker.deleteChat(principal, request.requestId),
    () => broker.archiveChat(principal, request.requestId), () => broker.cancelChat(principal, request.requestId),
    () => broker.run(principal, runInput('unsupported-run')), () => broker.get(principal, 'unsupported-run'),
    () => broker.list(principal), () => broker.events(principal, 'unsupported-run'), () => broker.cancel(principal, 'unsupported-run'),
    () => broker.sendChat(principal, request.requestId, runInput('unsupported-chat-run', request.requestId)),
  ];
  for (const operation of unsupportedOperations) await assert.rejects(operation(), { code: 'agent_format_unsupported', message: /data has been preserved/i });
  assert.equal((await broker.createChat(other, request)).chatId, request.requestId);
  assert.equal((await broker.listChats(other)).chats.length, 1);
  const input = runInput('healthy-run');
  await broker.run(other, input);
  assert.equal((await terminal(broker, other, input.runId)).state, 'succeeded');
  for (const [path, bytes] of originalFiles) assert.deepEqual(await fs.readFile(path), bytes);
  const marker = JSON.parse(await fs.readFile(join(root, 'desktop-agent-format.json'), 'utf8'));
  assert.deepEqual(marker.quarantinedAppIds, [principal.appId]);
  const restarted = new LocalAppAgentBroker(options);
  t.after(async () => { await restarted.dispose().catch(() => undefined); });
  await assert.rejects(restarted.createChat(principal, request), { code: 'agent_format_unsupported' });
  assert.equal((await restarted.chatCreationStatus(other, request))?.chatId, request.requestId);
  assert.equal((await restarted.get(other, input.runId)).state, 'succeeded');
  for (const [path, bytes] of originalFiles) assert.deepEqual(await fs.readFile(path), bytes);
});

test('empty unmarked Agent directories are usable and unsafe old stores stay isolated', { skip: process.platform === 'win32' }, async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'lingxi-agent-empty-store-'));
  const unsafePath = join(root, 'apps', principal.appId, 'agent');
  const emptyPath = join(root, 'apps', other.appId, 'agent');
  const target = join(root, 'unsafe-original');
  await fs.mkdir(target, { recursive: true });
  await fs.writeFile(join(target, 'keep.txt'), 'untouched');
  await fs.mkdir(dirname(unsafePath), { recursive: true });
  await fs.symlink(target, unsafePath);
  for (const group of ['tasks', 'chats', 'assets']) await fs.mkdir(join(emptyPath, group), { recursive: true });
  const broker = new LocalAppAgentBroker({ root, authorize: async () => auth(), revalidate: async () => true,
    execute: async () => { throw new Error('not run'); } });
  t.after(async () => { await broker.dispose().catch(() => undefined); await fs.rm(root, { recursive: true, force: true }); });
  await assert.rejects(broker.listChats(principal), { code: 'agent_format_unsupported' });
  const chat = await broker.createChat(other, { requestId: 'a2222222-2222-4222-8222-222222222222' });
  assert.equal(chat.chatId, 'a2222222-2222-4222-8222-222222222222');
  assert.equal((await fs.lstat(unsafePath)).isSymbolicLink(), true);
  assert.equal(await fs.readFile(join(target, 'keep.txt'), 'utf8'), 'untouched');
});


test('Chat creation is atomic by app and request, survives restart, and deletion retains its tombstone', async t => {
  const { root, broker, options } = await fixture(t);
  const input = { requestId: 'a1111111-1111-4111-8111-111111111111', title: '  Plan  ' };
  const [first, second] = await Promise.all([broker.createChat(principal, input), broker.createChat(principal, input)]);
  assert.deepEqual(first, second);
  assert.equal(first.chatId, input.requestId);
  assert.equal(first.title, 'Plan');
  assert.deepEqual(await broker.chatCreationStatus(principal, input), first);
  await assert.rejects(broker.createChat(principal, { ...input, title: 'Plan' }), { code: 'chat_create_conflict' });
  await assert.rejects(broker.chatCreationStatus(principal, { requestId: input.requestId }), { code: 'chat_create_conflict' });
  assert.equal(await broker.chatCreationStatus(other, input), null);
  const isolated = await broker.createChat(other, input);
  assert.equal(isolated.chatId, first.chatId);
  assert.equal((await broker.listChats(principal)).chats.length, 1);

  const resumed = new LocalAppAgentBroker(options);
  t.after(async () => { await resumed.dispose().catch(() => undefined); });
  assert.deepEqual(await resumed.chatCreationStatus(principal, input), first);
  await resumed.deleteChat(principal, first.chatId);
  await assert.rejects(resumed.createChat(principal, input), { code: 'chat_create_deleted' });
  await assert.rejects(resumed.chatCreationStatus(principal, { ...input, title: 'changed' }), { code: 'chat_create_conflict' });
  assert.deepEqual(await resumed.chatCreationStatus(other, input), isolated);
  const restarted = new LocalAppAgentBroker(options);
  t.after(async () => { await restarted.dispose().catch(() => undefined); });
  await assert.rejects(restarted.chatCreationStatus(principal, input), { code: 'chat_create_deleted' });
  await assert.rejects(restarted.createChat(principal, input), { code: 'chat_create_deleted' });
  const files = await fs.readdir(join(root, 'apps', principal.appId, 'agent', 'chats', input.requestId));
  assert.deepEqual(files, ['chat.json']);
});

test('incomplete Chat storage fails closed after restart without blocking unrelated creations', async t => {
  const { root, broker, options } = await fixture(t);
  await broker.initialize();
  const input = { requestId: 'b2222222-2222-4222-8222-222222222222' };
  const path = join(root, 'apps', principal.appId, 'agent', 'chats', input.requestId);
  await fs.mkdir(path, { recursive: true }); // Simulates a crash after claiming the directory.
  await assert.rejects(broker.createChat(principal, input), { code: 'chat_storage_uncertain' });
  await assert.rejects(broker.chatCreationStatus(principal, input), { code: 'chat_storage_uncertain' });
  const restarted = new LocalAppAgentBroker(options);
  t.after(async () => { await restarted.dispose().catch(() => undefined); });
  await assert.rejects(restarted.createChat(principal, input), { code: 'chat_storage_uncertain' });
  const unrelated = { requestId: 'c3333333-3333-4333-8333-333333333333' };
  assert.equal((await restarted.createChat(principal, unrelated)).chatId, unrelated.requestId);
  assert.deepEqual(await fs.readdir(path), []);
});

test('Chat creation requires a canonical UUID and exact bounded Unicode title', async t => {
  const { broker } = await fixture(t);
  const requestId = 'd4444444-4444-4444-8444-444444444444';
  for (const input of [undefined, {}, { requestId: requestId.toUpperCase() }, { requestId, title: '' },
    { requestId, title: '\u2003' }, { requestId, title: 'a'.repeat(81) },
    { requestId, title: 'a\u0085b' }, { requestId, title: '\ud800' }, { requestId, title: 'x', extra: true }]) {
    await assert.rejects(broker.createChat(principal, input), { code: 'invalid_chat' });
  }
  assert.equal(await broker.chatCreationStatus(principal, { requestId }), null);
  const chat = await broker.createChat(principal, { requestId, title: '\u2003好\u2003' });
  assert.equal(chat.title, '好');
});


test('Chat creation rechecks authorization when access is revoked during admission', async t => {
  const started = deferred<void>();
  const authorization = deferred<LocalAppAgentAuthorization>();
  const { broker } = await fixture(t, { authorize: async () => { started.resolve(); return authorization.promise; } });
  const input = { requestId: 'e5555555-5555-4555-8555-555555555555' };
  const creation = broker.createChat(principal, input);
  await started.promise;
  const cancellation = broker.cancelApp(principal.appId);
  authorization.resolve(auth());
  await assert.rejects(creation, { code: 'permission_revoked' });
  await cancellation;
  assert.equal(await broker.chatCreationStatus(principal, input), null);
});

test('committed Chat tombstones resume cleanup after failure and after host restart', async t => {
  const { root, broker, options } = await fixture(t);
  const first = { requestId: 'f6666666-6666-4666-8666-666666666666' };
  const second = { requestId: 'a7777777-7777-4777-8777-777777777777' };
  async function failAssetCleanup(input: { requestId: string }) {
    await broker.createChat(principal, input);
    const directory = join(root, 'apps', principal.appId, 'agent', 'chats', input.requestId);
    const assets = join(directory, 'assets');
    await fs.writeFile(join(assets, 'private-data'), 'private');
    const original = fs.rm;
    (fs as any).rm = async (path: unknown, ...args: unknown[]) => {
      if (path === assets) throw new Error('injected cleanup failure');
      return (original as any)(path, ...args);
    };
    try { await assert.rejects(broker.deleteChat(principal, input.requestId), { code: 'chat_storage_uncertain' }); }
    finally { (fs as any).rm = original; }
    await assert.rejects(broker.createChat(principal, input), { code: 'chat_storage_uncertain' });
    return directory;
  }
  const firstDirectory = await failAssetCleanup(first);
  await broker.deleteChat(principal, first.requestId);
  await assert.rejects(broker.createChat(principal, first), { code: 'chat_create_deleted' });
  assert.deepEqual(await fs.readdir(firstDirectory), ['chat.json']);
  const secondDirectory = await failAssetCleanup(second);
  const restarted = new LocalAppAgentBroker(options);
  t.after(async () => { await restarted.dispose().catch(() => undefined); });
  await assert.rejects(restarted.createChat(principal, second), { code: 'chat_create_deleted' });
  assert.deepEqual(await fs.readdir(secondDirectory), ['chat.json']);
});

test('concurrent Chat requests with different input bind only the winner', async t => {
  const { broker } = await fixture(t);
  const requestId = 'b8888888-8888-4888-8888-888888888888';
  const outcomes = await Promise.allSettled([
    broker.createChat(principal, { requestId, title: 'first' }),
    broker.createChat(principal, { requestId, title: 'second' }),
  ]);
  assert.equal(outcomes.filter(result => result.status === 'fulfilled').length, 1);
  const rejection = outcomes.find(result => result.status === 'rejected') as PromiseRejectedResult;
  assert.equal(rejection.reason.code, 'chat_create_conflict');
  assert.equal((await broker.listChats(principal)).chats.length, 1);
});

test('Chat directory preparation failure hides the mutated in-memory state until restart', async t => {
  const { root, broker, options } = await fixture(t);
  const input = { requestId: 'c9999999-9999-4999-8999-999999999999' };
  await broker.createChat(principal, input);
  const chatDirectory = join(root, 'apps', principal.appId, 'agent', 'chats', input.requestId);
  const original = fs.mkdir;
  (fs as any).mkdir = async (path: unknown, ...args: unknown[]) => {
    if (path === chatDirectory) throw new Error('injected directory failure');
    return (original as any)(path, ...args);
  };
  try { await assert.rejects(broker.archiveChat(principal, input.requestId), { code: 'chat_storage_uncertain' }); }
  finally { (fs as any).mkdir = original; }
  await assert.rejects(broker.chatCreationStatus(principal, input), { code: 'chat_storage_uncertain' });
  await assert.rejects(broker.getChat(principal, input.requestId), { code: 'chat_storage_uncertain' });
  assert.deepEqual(await broker.listChats(principal, { includeArchived: true }), { chats: [], hasUncertainChats: true, unavailableChats: [{ chatId: input.requestId, code: 'chat_storage_uncertain' }] });
  const restarted = new LocalAppAgentBroker(options);
  t.after(async () => { await restarted.dispose().catch(() => undefined); });
  assert.equal((await restarted.chatCreationStatus(principal, input))?.archived, false);
});

test('queued duplicate creation and status fail after the application is revoked', async t => {
  let block = false;
  const started = deferred<void>(), release = deferred<void>();
  const { broker } = await fixture(t, { authorize: async () => {
    if (block) { started.resolve(); await release.promise; }
    return auth();
  } });
  const existing = { requestId: 'daaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa' };
  await broker.createChat(principal, existing);
  block = true;
  const blocking = broker.createChat(principal, { requestId: 'ebbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb' });
  await started.promise;
  const duplicate = broker.createChat(principal, existing);
  const status = broker.chatCreationStatus(principal, existing);
  const outcomes = Promise.allSettled([blocking, duplicate, status]);
  await new Promise(resolve => setImmediate(resolve));
  const revoked = broker.cancelApp(principal.appId);
  release.resolve();
  const settled = await outcomes;
  await revoked;
  assert.deepEqual(settled.map(result => result.status), ['rejected', 'rejected', 'rejected']);
  for (const result of settled as PromiseRejectedResult[]) assert.equal(result.reason.code, 'permission_revoked');
  block = false;
  assert.equal((await broker.chatCreationStatus(principal, existing))?.chatId, existing.requestId);
});

test('Chat reads wait for an in-flight write and reject its uncommitted state after failure', async t => {
  const { root, broker } = await fixture(t);
  const input = { requestId: 'ccaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa' };
  await broker.createChat(principal, input);
  const chatPath = join(root, 'apps', principal.appId, 'agent', 'chats', input.requestId, 'chat.json');
  const original = fs.rename, started = deferred<void>(), release = deferred<void>();
  (fs as any).rename = async (from: unknown, to: unknown) => {
    if (to === chatPath) { started.resolve(); await release.promise; throw new Error('injected commit failure'); }
    return (original as any)(from, to);
  };
  let getFinished = false, listFinished = false;
  const write = broker.archiveChat(principal, input.requestId);
  await started.promise;
  const read = broker.getChat(principal, input.requestId).finally(() => { getFinished = true; });
  const list = broker.listChats(principal, { includeArchived: true }).finally(() => { listFinished = true; });
  const outcomes = Promise.allSettled([write, read, list]);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(getFinished, false);
  assert.equal(listFinished, false);
  release.resolve();
  const settled = await outcomes;
  (fs as any).rename = original;
  assert.deepEqual(settled.map(result => result.status), ['rejected', 'rejected', 'fulfilled']);
  for (const result of settled.slice(0, 2) as PromiseRejectedResult[]) assert.equal(result.reason.code, 'chat_storage_uncertain');
  assert.deepEqual((settled[2] as PromiseFulfilledResult<unknown>).value, { chats: [], hasUncertainChats: true, unavailableChats: [{ chatId: input.requestId, code: 'chat_storage_uncertain' }] });
});

test('failed terminal task journal write never publishes success or partial Chat history', async t => {
  const started = deferred<LocalAppAgentExecution>();
  const completion = deferred<{ result: LocalAppAgentPrivateResult; usage: { turns: number } }>();
  const { root, broker, options } = await fixture(t, { execute: execution => { started.resolve(execution); return completion.promise; } });
  const input = { requestId: 'dcbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb' };
  const chat = await broker.createChat(principal, input);
  await broker.sendChat(principal, chat.chatId, runInput('failed-terminal-write', chat.chatId));
  const execution = await started.promise;
  const path = join(root, 'apps', principal.appId, 'agent', 'tasks', `${execution.taskStorageId}.json`);
  const attempted = deferred<void>(), original = fs.rename;
  (fs as any).rename = async (from: unknown, to: unknown) => {
    if (to === path) { attempted.resolve(); throw new Error('injected task journal failure'); }
    return (original as any)(from, to);
  };
  try {
    completion.resolve({ result: { text: 'answer', output: { ok: true }, attachments: [], chatId: chat.chatId, messageId: 'assistant-failed-write' }, usage: { turns: 1 } });
    await attempted.promise;
    await assert.rejects(broker.get(principal, 'failed-terminal-write'), { code: 'agent_storage_unavailable' });
    await assert.rejects(broker.events(principal, 'failed-terminal-write'), { code: 'agent_storage_unavailable' });
    await assert.rejects(broker.list(principal), { code: 'agent_storage_unavailable' });
    await assert.rejects(broker.chatCreationStatus(principal, input), { code: 'chat_storage_uncertain' });
    const disk = JSON.parse(await fs.readFile(path, 'utf8'));
    assert.equal(disk.run.state, 'running');
    assert.equal(disk.events.some((event: { type: string }) => event.type === 'RUN_FINISHED'), false);
  } finally { (fs as any).rename = original; }
  const restarted = new LocalAppAgentBroker(options);
  t.after(async () => { await restarted.dispose().catch(() => undefined); });
  assert.equal((await restarted.get(principal, 'failed-terminal-write')).state, 'interrupted');
  assert.equal((await restarted.chatCreationStatus(principal, input))?.state, 'interrupted');
});

test('task admission does not replay after a committed journal rename loses its acknowledgement', async t => {
  let executions = 0;
  const { broker, options } = await fixture(t, { execute: async () => {
    executions++; return { result: { text: 'unexpected', attachments: [], messageId: 'unexpected' }, usage: { turns: 1 } };
  } });
  const original = fs.rename, input = runInput('ambiguous-admission');
  (fs as any).rename = async (from: unknown, to: unknown) => {
    if (typeof to === 'string' && to.includes('/tasks/') && to.endsWith('.json')) {
      await (original as any)(from, to);
      throw new Error('injected lost rename acknowledgement');
    }
    return (original as any)(from, to);
  };
  try {
    await assert.rejects(broker.run(principal, input), { code: 'agent_storage_unavailable' });
    await assert.rejects(broker.run(principal, input), { code: 'agent_storage_unavailable' });
    await assert.rejects(broker.list(principal), { code: 'agent_storage_unavailable' });
    assert.equal(executions, 0);
  } finally { (fs as any).rename = original; }
  const restarted = new LocalAppAgentBroker(options);
  t.after(async () => { await restarted.dispose().catch(() => undefined); });
  assert.equal((await restarted.get(principal, input.runId)).state, 'interrupted');
  assert.deepEqual(await restarted.run(principal, input), { runId: input.runId });
  assert.equal(executions, 0);
});

test('directory sync failure after Chat rename fences the request until restart', { skip: process.platform === 'win32' }, async t => {
  const { root, broker, options } = await fixture(t);
  const input = { requestId: 'cdeeeeee-eeee-4eee-8eee-eeeeeeeeeeee' };
  const directory = join(root, 'apps', principal.appId, 'agent', 'chats', input.requestId);
  const original = fs.open;
  (fs as any).open = async (path: unknown, ...args: unknown[]) => {
    const handle = await (original as any)(path, ...args);
    if (path === directory && args[0] === 'r') return { sync: async () => { throw new Error('injected directory sync failure'); }, close: () => handle.close() };
    return handle;
  };
  try {
    await assert.rejects(broker.createChat(principal, input), { code: 'chat_storage_uncertain' });
    await assert.rejects(broker.chatCreationStatus(principal, input), { code: 'chat_storage_uncertain' });
  } finally { (fs as any).open = original; }
  const restarted = new LocalAppAgentBroker(options);
  t.after(async () => { await restarted.dispose().catch(() => undefined); });
  assert.equal((await restarted.chatCreationStatus(principal, input))?.chatId, input.requestId);
  assert.equal((await restarted.createChat(principal, input)).chatId, input.requestId);
  assert.equal((await restarted.listChats(principal)).chats.length, 1);
});

test('directory sync failure after task rename prevents replay until restart', { skip: process.platform === 'win32' }, async t => {
  let executions = 0;
  const { root, broker, options } = await fixture(t, { execute: async () => {
    executions++; return { result: { text: 'unexpected', attachments: [], messageId: 'unexpected' }, usage: { turns: 1 } };
  } });
  const directory = join(root, 'apps', principal.appId, 'agent', 'tasks');
  const original = fs.open, input = runInput('ambiguous-directory-sync');
  (fs as any).open = async (path: unknown, ...args: unknown[]) => {
    const handle = await (original as any)(path, ...args);
    if (path === directory && args[0] === 'r') return { sync: async () => { throw new Error('injected directory sync failure'); }, close: () => handle.close() };
    return handle;
  };
  try {
    await assert.rejects(broker.run(principal, input), { code: 'agent_storage_unavailable' });
    await assert.rejects(broker.run(principal, input), { code: 'agent_storage_unavailable' });
  } finally { (fs as any).open = original; }
  const restarted = new LocalAppAgentBroker(options);
  t.after(async () => { await restarted.dispose().catch(() => undefined); });
  assert.equal((await restarted.get(principal, input.runId)).state, 'interrupted');
  assert.deepEqual(await restarted.run(principal, input), { runId: input.runId });
  assert.equal(executions, 0);
});

test('restart validates committed Chat metadata before exposing its identity', async t => {
  const { root, broker, options } = await fixture(t);
  const input = { requestId: 'efaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa' };
  await broker.createChat(principal, input);
  const path = join(root, 'apps', principal.appId, 'agent', 'chats', input.requestId, 'chat.json');
  const saved = JSON.parse(await fs.readFile(path, 'utf8'));
  await fs.writeFile(path, JSON.stringify({ ...saved, archived: 'not a boolean' }));
  const restarted = new LocalAppAgentBroker(options);
  t.after(async () => { await restarted.dispose().catch(() => undefined); });
  await assert.rejects(restarted.chatCreationStatus(principal, input), { code: 'chat_storage_uncertain' });
  await assert.rejects(restarted.createChat(principal, input), { code: 'chat_storage_uncertain' });
  assert.deepEqual(await restarted.listChats(principal), { chats: [], hasUncertainChats: true, unavailableChats: [{ chatId: input.requestId, code: 'chat_storage_uncertain' }] });
});

test('task success remains unavailable until its Chat outcome is durably saved and recovered', async t => {
  const started = deferred<LocalAppAgentExecution>();
  const completion = deferred<{ result: LocalAppAgentPrivateResult; usage: { turns: number } }>();
  const { root, broker, options } = await fixture(t, { execute: execution => { started.resolve(execution); return completion.promise; } });
  const input = { requestId: 'fdcccccc-cccc-4ccc-8ccc-cccccccccccc' };
  const chat = await broker.createChat(principal, input);
  await broker.sendChat(principal, chat.chatId, runInput('chat-save-failed', chat.chatId));
  await started.promise;
  const path = join(root, 'apps', principal.appId, 'agent', 'chats', chat.chatId, 'chat.json');
  const attempted = deferred<void>(), original = fs.rename;
  (fs as any).rename = async (from: unknown, to: unknown) => {
    if (to === path) { attempted.resolve(); throw new Error('injected Chat commit failure'); }
    return (original as any)(from, to);
  };
  try {
    completion.resolve({ result: { text: 'answer', output: { ok: true }, attachments: [], chatId: chat.chatId, messageId: 'assistant-chat-save' }, usage: { turns: 1 } });
    await attempted.promise;
    await assert.rejects(broker.get(principal, 'chat-save-failed'), { code: 'agent_storage_unavailable' });
    await assert.rejects(broker.events(principal, 'chat-save-failed'), { code: 'agent_storage_unavailable' });
    await assert.rejects(broker.list(principal), { code: 'agent_storage_unavailable' });
    await assert.rejects(broker.chatHistory(principal, chat.chatId), { code: 'chat_storage_uncertain' });
  } finally { (fs as any).rename = original; }
  const restarted = new LocalAppAgentBroker(options);
  t.after(async () => { await restarted.dispose().catch(() => undefined); });
  await assert.rejects(restarted.get(principal, 'chat-save-failed'), { code: 'agent_storage_unavailable' });
  await assert.rejects(restarted.chatHistory(principal, chat.chatId), { code: 'chat_storage_uncertain' });
});

test('revocation during a pending terminal rename cannot publish success', async t => {
  const started = deferred<LocalAppAgentExecution>();
  const completion = deferred<{ result: LocalAppAgentPrivateResult; usage: { turns: number } }>();
  const { root, broker, options } = await fixture(t, { execute: execution => { started.resolve(execution); return completion.promise; } });
  const input = runInput('revoked-during-terminal-write');
  await broker.run(principal, input);
  const execution = await started.promise;
  const path = join(root, 'apps', principal.appId, 'agent', 'tasks', `${execution.taskStorageId}.json`);
  const beforeRename = deferred<void>(), releaseRename = deferred<void>(), original = fs.rename;
  let hold = true;
  (fs as any).rename = async (from: unknown, to: unknown) => {
    if (to === path && hold) { hold = false; beforeRename.resolve(); await releaseRename.promise; }
    return (original as any)(from, to);
  };
  try {
    completion.resolve({ result: { text: 'answer', output: { ok: true }, attachments: [], messageId: 'assistant-revoked' }, usage: { turns: 1 } });
    await beforeRename.promise;
    const revoked = broker.cancelApp(principal.appId);
    releaseRename.resolve();
    await revoked;
    assert.equal((await broker.get(principal, input.runId)).state, 'cancelled');
    const events = (await broker.events(principal, input.runId)).events;
    assert.equal(events.some(event => event.type === 'TEXT_MESSAGE_CONTENT'), false);
    const terminalEvent = events.at(-1) as { type: string; outcome?: { type: string } };
    assert.equal(terminalEvent.type, 'RUN_FINISHED');
    assert.equal(terminalEvent.outcome?.type, 'cancelled');
  } finally { releaseRename.resolve(); (fs as any).rename = original; }
  const restarted = new LocalAppAgentBroker(options);
  t.after(async () => { await restarted.dispose().catch(() => undefined); });
  assert.equal((await restarted.get(principal, input.runId)).state, 'cancelled');
});

test('revocation during Chat outcome save removes the unconfirmed assistant result', async t => {
  const started = deferred<LocalAppAgentExecution>();
  const completion = deferred<{ result: LocalAppAgentPrivateResult; usage: { turns: number } }>();
  const { root, broker, options } = await fixture(t, { execute: execution => { started.resolve(execution); return completion.promise; } });
  const creation = { requestId: 'feeeeeee-eeee-4eee-8eee-eeeeeeeeeeee' };
  const chat = await broker.createChat(principal, creation);
  const input = runInput('revoked-during-chat-write', chat.chatId);
  await broker.sendChat(principal, chat.chatId, input);
  await started.promise;
  const path = join(root, 'apps', principal.appId, 'agent', 'chats', chat.chatId, 'chat.json');
  const beforeRename = deferred<void>(), releaseRename = deferred<void>(), original = fs.rename;
  let hold = true;
  (fs as any).rename = async (from: unknown, to: unknown) => {
    if (to === path && hold) { hold = false; beforeRename.resolve(); await releaseRename.promise; }
    return (original as any)(from, to);
  };
  try {
    completion.resolve({ result: { text: 'answer', output: { ok: true }, attachments: [], chatId: chat.chatId, messageId: 'assistant-revoked-chat' }, usage: { turns: 1 } });
    await beforeRename.promise;
    const revoked = broker.cancelApp(principal.appId);
    releaseRename.resolve();
    await revoked;
    assert.equal((await broker.get(principal, input.runId)).state, 'cancelled');
    assert.equal((await broker.chatHistory(principal, chat.chatId)).messages.some(message => message.id === 'assistant-revoked-chat'), false);
  } finally { releaseRename.resolve(); (fs as any).rename = original; }
  const restarted = new LocalAppAgentBroker(options);
  t.after(async () => { await restarted.dispose().catch(() => undefined); });
  assert.equal((await restarted.get(principal, input.runId)).state, 'cancelled');
  assert.equal((await restarted.chatHistory(principal, chat.chatId)).messages.some(message => message.id === 'assistant-revoked-chat'), false);
});

test('restart fences a Chat success interrupted between task and Chat commits', async t => {
  const started = deferred<LocalAppAgentExecution>();
  const completion = deferred<{ result: LocalAppAgentPrivateResult; usage: { turns: number } }>();
  const { root, broker, options } = await fixture(t, { execute: execution => { started.resolve(execution); return completion.promise; } });
  const creation = { requestId: 'aaeeeeee-eeee-4eee-8eee-eeeeeeeeeeee' };
  const healthy = await broker.createChat(principal, { requestId: '11999999-9999-4999-8999-999999999999' });
  const chat = await broker.createChat(principal, creation);
  const input = runInput('completion-crash-window', chat.chatId);
  await broker.sendChat(principal, chat.chatId, input);
  const execution = await started.promise;
  const chatPath = join(root, 'apps', principal.appId, 'agent', 'chats', chat.chatId, 'chat.json');
  const taskPath = join(root, 'apps', principal.appId, 'agent', 'tasks', `${execution.taskStorageId}.json`);
  const beforeChatCommit = deferred<void>(), releaseChatCommit = deferred<void>(), originalRename = fs.rename;
  let hold = true;
  (fs as any).rename = async (from: unknown, to: unknown) => {
    if (to === chatPath && hold) { hold = false; beforeChatCommit.resolve(); await releaseChatCommit.promise; }
    return (originalRename as any)(from, to);
  };
  try {
    completion.resolve({ result: { text: 'answer', output: { ok: true }, attachments: [], chatId: chat.chatId, messageId: 'assistant-crash-window' }, usage: { turns: 1 } });
    await beforeChatCommit.promise;
    assert.equal(JSON.parse(await fs.readFile(taskPath, 'utf8')).completionPending, true);
    const snapshot = await fs.mkdtemp(join(tmpdir(), 'lingxi-chat-crash-snapshot-'));
    t.after(async () => { await fs.rm(snapshot, { recursive: true, force: true }); });
    await fs.cp(root, snapshot, { recursive: true });
    const frozenTask = join(snapshot, 'apps', principal.appId, 'agent', 'tasks', `${execution.taskStorageId}.json`);
    const frozenChat = join(snapshot, 'apps', principal.appId, 'agent', 'chats', chat.chatId, 'chat.json');
    const taskBytes = await fs.readFile(frozenTask), chatBytes = await fs.readFile(frozenChat);
    const recovered = new LocalAppAgentBroker({ ...options, root: snapshot });
    t.after(async () => { await recovered.dispose().catch(() => undefined); });
    await assert.rejects(recovered.get(principal, input.runId), { code: 'agent_storage_unavailable' });
    await assert.rejects(recovered.events(principal, input.runId), { code: 'agent_storage_unavailable' });
    await assert.rejects(recovered.chatCreationStatus(principal, creation), { code: 'chat_storage_uncertain' });
    const page = await recovered.listChats(principal);
    assert.deepEqual(page.chats.map(row => row.chatId), [healthy.chatId]);
    assert.deepEqual(page.unavailableChats, [{ chatId: chat.chatId, code: 'chat_storage_uncertain' }]);
    assert.equal(page.hasUncertainChats, true);
    assert.deepEqual(await fs.readFile(frozenTask), taskBytes);
    assert.deepEqual(await fs.readFile(frozenChat), chatBytes);
    releaseChatCommit.resolve();
    assert.equal((await terminal(broker, principal, input.runId)).state, 'succeeded');
    assert.equal(Object.hasOwn(JSON.parse(await fs.readFile(taskPath, 'utf8')), 'completionPending'), false);
  } finally { releaseChatCommit.resolve(); (fs as any).rename = originalRename; }
  const restarted = new LocalAppAgentBroker(options);
  t.after(async () => { await restarted.dispose().catch(() => undefined); });
  assert.equal((await restarted.get(principal, input.runId)).state, 'succeeded');
  assert.equal((await restarted.chatHistory(principal, chat.chatId)).messages.at(-1)?.id, 'assistant-crash-window');
});

test('asset cleanup failure fences the staged Chat outcome across restart', async t => {
  const started = deferred<LocalAppAgentExecution>();
  const completion = deferred<{ result: LocalAppAgentPrivateResult; usage: { turns: number } }>();
  const { broker, options } = await fixture(t, { execute: execution => { started.resolve(execution); return completion.promise; } });
  const creation = { requestId: 'bbffffff-ffff-4fff-8fff-ffffffffffff' };
  const chat = await broker.createChat(principal, creation);
  const input = runInput('cleanup-failure', chat.chatId);
  await broker.sendChat(principal, chat.chatId, input);
  await started.promise;
  const attempted = deferred<void>(), original = LocalAppChatAssetJournal.prototype.pruneUnreferenced;
  LocalAppChatAssetJournal.prototype.pruneUnreferenced = async () => { attempted.resolve(); throw new Error('injected cleanup failure'); };
  try {
    completion.resolve({ result: { text: 'answer', output: { ok: true }, attachments: [], chatId: chat.chatId, messageId: 'assistant-cleanup' }, usage: { turns: 1 } });
    await attempted.promise;
    await assert.rejects(broker.get(principal, input.runId), { code: 'agent_storage_unavailable' });
    await assert.rejects(broker.chatHistory(principal, chat.chatId), { code: 'chat_storage_uncertain' });
  } finally { LocalAppChatAssetJournal.prototype.pruneUnreferenced = original; }
  const restarted = new LocalAppAgentBroker(options);
  t.after(async () => { await restarted.dispose().catch(() => undefined); });
  await assert.rejects(restarted.get(principal, input.runId), { code: 'agent_storage_unavailable' });
  await assert.rejects(restarted.chatCreationStatus(principal, creation), { code: 'chat_storage_uncertain' });
});

test('revocation during asset cleanup cannot publish the staged Chat success', async t => {
  const started = deferred<LocalAppAgentExecution>();
  const completion = deferred<{ result: LocalAppAgentPrivateResult; usage: { turns: number } }>();
  const { broker, options } = await fixture(t, { execute: execution => { started.resolve(execution); return completion.promise; } });
  const creation = { requestId: 'ccdddddd-dddd-4ddd-8ddd-dddddddddddd' };
  const chat = await broker.createChat(principal, creation);
  const input = runInput('revoked-during-cleanup', chat.chatId);
  await broker.sendChat(principal, chat.chatId, input);
  await started.promise;
  const entered = deferred<void>(), release = deferred<void>(), original = LocalAppChatAssetJournal.prototype.pruneUnreferenced;
  let hold = true;
  LocalAppChatAssetJournal.prototype.pruneUnreferenced = async function (...args) {
    if (hold) { hold = false; entered.resolve(); await release.promise; }
    return original.apply(this, args);
  };
  try {
    completion.resolve({ result: { text: 'answer', output: { ok: true }, attachments: [], chatId: chat.chatId, messageId: 'assistant-revoked-cleanup' }, usage: { turns: 1 } });
    await entered.promise;
    const revoked = broker.cancelApp(principal.appId);
    release.resolve(); await revoked;
    assert.equal((await broker.get(principal, input.runId)).state, 'cancelled');
    assert.equal((await broker.chatHistory(principal, chat.chatId)).messages.some(message => message.id === 'assistant-revoked-cleanup'), false);
  } finally { release.resolve(); LocalAppChatAssetJournal.prototype.pruneUnreferenced = original; }
  const restarted = new LocalAppAgentBroker(options);
  t.after(async () => { await restarted.dispose().catch(() => undefined); });
  assert.equal((await restarted.get(principal, input.runId)).state, 'cancelled');
  assert.equal((await restarted.chatHistory(principal, chat.chatId)).messages.some(message => message.id === 'assistant-revoked-cleanup'), false);
});

test('Chat inventory paginates available and uncertain IDs without leaking other apps', async t => {
  const { root, broker } = await fixture(t, { now: () => 123 });
  const liveIds = ['a1111111-1111-4111-8111-111111111111', 'b2222222-2222-4222-8222-222222222222'];
  const uncertainIds = ['c3333333-3333-4333-8333-333333333333', 'd4444444-4444-4444-8444-444444444444'];
  for (const requestId of liveIds) await broker.createChat(principal, { requestId });
  for (const requestId of uncertainIds) {
    await fs.mkdir(join(root, 'apps', principal.appId, 'agent', 'chats', requestId), { recursive: true });
    await assert.rejects(broker.createChat(principal, { requestId }), { code: 'chat_storage_uncertain' });
  }
  const otherId = 'e5555555-5555-4555-8555-555555555555';
  await broker.createChat(other, { requestId: otherId });
  const seen: string[] = [], states: string[] = [];
  let cursor: string | undefined;
  for (let pageNumber = 0; pageNumber < 8; pageNumber++) {
    const page = await broker.listChats(principal, { limit: 1, ...(cursor ? { cursor } : {}) });
    assert.equal(page.hasUncertainChats, true);
    assert.equal(page.chats.length + (page.unavailableChats?.length ?? 0), 1);
    for (const chat of page.chats) { seen.push(chat.chatId); states.push('available'); }
    for (const chat of page.unavailableChats ?? []) { seen.push(chat.chatId); states.push(chat.code); }
    cursor = page.nextCursor;
    if (!cursor) break;
  }
  assert.equal(cursor, undefined);
  assert.deepEqual(seen, [...liveIds, ...uncertainIds]);
  assert.deepEqual(states, ['available', 'available', 'chat_storage_uncertain', 'chat_storage_uncertain']);
  assert.deepEqual((await broker.listChats(other)).chats.map(chat => chat.chatId), [otherId]);
  assert.equal((await broker.listChats(other)).hasUncertainChats, undefined);
});

test('termination fence survives snapshots taken around the final successful rename', async t => {
  for (const scenario of ['stateless-before', 'stateless-after', 'chat-after'] as const) {
    const started = deferred<LocalAppAgentExecution>();
    const completion = deferred<{ result: LocalAppAgentPrivateResult; usage: { turns: number } }>();
    const { root, broker, options } = await fixture(t, { execute: execution => { started.resolve(execution); return completion.promise; } });
    const creation = { requestId: 'ddaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa' };
    const chat = scenario === 'chat-after' ? await broker.createChat(principal, creation) : undefined;
    const input = runInput(`termination-fence-${scenario}`, chat?.chatId);
    if (chat) await broker.sendChat(principal, chat.chatId, input); else await broker.run(principal, input);
    const execution = await started.promise;
    const taskPath = join(root, 'apps', principal.appId, 'agent', 'tasks', `${execution.taskStorageId}.json`);
    const reached = deferred<void>(), release = deferred<void>(), original = fs.rename;
    let commits = 0;
    (fs as any).rename = async (from: unknown, to: unknown) => {
      if (to === taskPath) {
        commits++;
        if (commits === 2) {
          if (scenario !== 'stateless-before') await (original as any)(from, to);
          reached.resolve(); await release.promise;
          if (scenario === 'stateless-before') await (original as any)(from, to);
          return;
        }
      }
      return (original as any)(from, to);
    };
    try {
      completion.resolve({ result: { text: 'answer', output: { ok: true }, attachments: [],
        ...(chat ? { chatId: chat.chatId } : {}), messageId: `assistant-${scenario}` }, usage: { turns: 1 } });
      await reached.promise;
      if (scenario !== 'stateless-before') {
        const disk = JSON.parse(await fs.readFile(taskPath, 'utf8'));
        assert.equal(disk.run.state, 'succeeded');
        assert.equal(Object.hasOwn(disk, 'completionPending'), false);
      }
      // cancelApp writes its fence synchronously before its first await.
      const revoked = broker.cancelApp(principal.appId);
      assert.equal((await fs.lstat(`${taskPath}.termination`)).isFile(), true);
      const snapshot = await fs.mkdtemp(join(tmpdir(), 'lingxi-termination-snapshot-'));
      t.after(async () => { await fs.rm(snapshot, { recursive: true, force: true }); });
      await fs.cp(root, snapshot, { recursive: true });
      const snapshotTask = join(snapshot, 'apps', principal.appId, 'agent', 'tasks', `${execution.taskStorageId}.json`);
      const before = await fs.readFile(snapshotTask);
      const recovered = new LocalAppAgentBroker({ ...options, root: snapshot });
      t.after(async () => { await recovered.dispose().catch(() => undefined); });
      await assert.rejects(recovered.get(principal, input.runId), { code: 'agent_storage_unavailable' });
      await assert.rejects(recovered.events(principal, input.runId), { code: 'agent_storage_unavailable' });
      if (chat) {
        await assert.rejects(recovered.chatCreationStatus(principal, creation), { code: 'chat_storage_uncertain' });
        const page = await recovered.listChats(principal);
        assert.deepEqual(page.unavailableChats, [{ chatId: chat.chatId, code: 'chat_storage_uncertain' }]);
      }
      assert.deepEqual(await fs.readFile(snapshotTask), before);
      release.resolve(); await revoked;
      assert.equal((await broker.get(principal, input.runId)).state, 'cancelled');
      await assert.rejects(fs.lstat(`${taskPath}.termination`), { code: 'ENOENT' });
    } finally { release.resolve(); (fs as any).rename = original; }
    const restarted = new LocalAppAgentBroker(options);
    t.after(async () => { await restarted.dispose().catch(() => undefined); });
    assert.equal((await restarted.get(principal, input.runId)).state, 'cancelled');
  }
});

test('direct task cancellation fences a terminal rename before exposing its acknowledgement', async t => {
  const started = deferred<LocalAppAgentExecution>();
  const completion = deferred<{ result: LocalAppAgentPrivateResult; usage: { turns: number } }>();
  const { root, broker, options } = await fixture(t, { execute: execution => { started.resolve(execution); return completion.promise; } });
  const input = runInput('direct-cancel-after-rename');
  await broker.run(principal, input);
  const execution = await started.promise;
  const path = join(root, 'apps', principal.appId, 'agent', 'tasks', `${execution.taskStorageId}.json`);
  const reached = deferred<void>(), release = deferred<void>(), original = fs.rename;
  let hold = true;
  (fs as any).rename = async (from: unknown, to: unknown) => {
    if (to === path && hold) { hold = false; await (original as any)(from, to); reached.resolve(); await release.promise; return; }
    return (original as any)(from, to);
  };
  try {
    completion.resolve({ result: { text: 'answer', output: { ok: true }, attachments: [], messageId: 'assistant-direct-cancel' }, usage: { turns: 1 } });
    await reached.promise;
    const cancelled = broker.cancel(principal, input.runId);
    // The public cancel path awaits initialization before installing its fence.
    for (let retry = 0; retry < 50; retry++) {
      try { await fs.lstat(`${path}.termination`); break; }
      catch { await new Promise(resolve => setTimeout(resolve, 1)); }
    }
    assert.equal((await fs.lstat(`${path}.termination`)).isFile(), true);
    const snapshot = await fs.mkdtemp(join(tmpdir(), 'lingxi-direct-cancel-snapshot-'));
    t.after(async () => { await fs.rm(snapshot, { recursive: true, force: true }); });
    await fs.cp(root, snapshot, { recursive: true });
    const recovered = new LocalAppAgentBroker({ ...options, root: snapshot });
    t.after(async () => { await recovered.dispose().catch(() => undefined); });
    await assert.rejects(recovered.get(principal, input.runId), { code: 'agent_storage_unavailable' });
    release.resolve(); await cancelled;
    assert.equal((await broker.get(principal, input.runId)).state, 'cancelled');
  } finally { release.resolve(); (fs as any).rename = original; }
});

test('host shutdown fences a successful rename until termination is durably settled', async t => {
  const started = deferred<LocalAppAgentExecution>();
  const completion = deferred<{ result: LocalAppAgentPrivateResult; usage: { turns: number } }>();
  const { root, broker, options } = await fixture(t, { execute: execution => { started.resolve(execution); return completion.promise; } });
  const input = runInput('shutdown-after-rename');
  await broker.run(principal, input);
  const execution = await started.promise;
  const path = join(root, 'apps', principal.appId, 'agent', 'tasks', `${execution.taskStorageId}.json`);
  const reached = deferred<void>(), release = deferred<void>(), original = fs.rename;
  let hold = true;
  (fs as any).rename = async (from: unknown, to: unknown) => {
    if (to === path && hold) { hold = false; await (original as any)(from, to); reached.resolve(); await release.promise; return; }
    return (original as any)(from, to);
  };
  try {
    completion.resolve({ result: { text: 'answer', output: { ok: true }, attachments: [], messageId: 'assistant-shutdown' }, usage: { turns: 1 } });
    await reached.promise;
    const shuttingDown = broker.dispose();
    assert.equal((await fs.lstat(`${path}.termination`)).isFile(), true);
    const snapshot = await fs.mkdtemp(join(tmpdir(), 'lingxi-shutdown-snapshot-'));
    t.after(async () => { await fs.rm(snapshot, { recursive: true, force: true }); });
    await fs.cp(root, snapshot, { recursive: true });
    const recovered = new LocalAppAgentBroker({ ...options, root: snapshot });
    t.after(async () => { await recovered.dispose().catch(() => undefined); });
    await assert.rejects(recovered.get(principal, input.runId), { code: 'agent_storage_unavailable' });
    release.resolve(); await shuttingDown;
    assert.notEqual((await broker.get(principal, input.runId)).state, 'succeeded');
  } finally { release.resolve(); (fs as any).rename = original; }
});

test('Chat cancellation fences a completed final rename before it can report success', async t => {
  const started = deferred<LocalAppAgentExecution>();
  const completion = deferred<{ result: LocalAppAgentPrivateResult; usage: { turns: number } }>();
  const { root, broker, options } = await fixture(t, { execute: execution => { started.resolve(execution); return completion.promise; } });
  const creation = { requestId: 'eebbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb' };
  const chat = await broker.createChat(principal, creation);
  const input = runInput('chat-cancel-after-rename', chat.chatId);
  await broker.sendChat(principal, chat.chatId, input);
  const execution = await started.promise;
  const path = join(root, 'apps', principal.appId, 'agent', 'tasks', `${execution.taskStorageId}.json`);
  const reached = deferred<void>(), release = deferred<void>(), original = fs.rename;
  let commits = 0;
  (fs as any).rename = async (from: unknown, to: unknown) => {
    if (to === path && ++commits === 2) { await (original as any)(from, to); reached.resolve(); await release.promise; return; }
    return (original as any)(from, to);
  };
  try {
    completion.resolve({ result: { text: 'answer', output: { ok: true }, attachments: [], chatId: chat.chatId, messageId: 'assistant-chat-cancel' }, usage: { turns: 1 } });
    await reached.promise;
    const cancelled = broker.cancelChat(principal, chat.chatId);
    for (let retry = 0; retry < 50; retry++) {
      try { await fs.lstat(`${path}.termination`); break; }
      catch { await new Promise(resolve => setTimeout(resolve, 1)); }
    }
    assert.equal((await fs.lstat(`${path}.termination`)).isFile(), true);
    const snapshot = await fs.mkdtemp(join(tmpdir(), 'lingxi-chat-cancel-snapshot-'));
    t.after(async () => { await fs.rm(snapshot, { recursive: true, force: true }); });
    await fs.cp(root, snapshot, { recursive: true });
    const recovered = new LocalAppAgentBroker({ ...options, root: snapshot });
    t.after(async () => { await recovered.dispose().catch(() => undefined); });
    await assert.rejects(recovered.get(principal, input.runId), { code: 'agent_storage_unavailable' });
    await assert.rejects(recovered.chatCreationStatus(principal, creation), { code: 'chat_storage_uncertain' });
    release.resolve(); await cancelled;
    assert.equal((await broker.get(principal, input.runId)).state, 'cancelled');
    assert.equal((await broker.chatHistory(principal, chat.chatId)).messages.some(row => row.id === 'assistant-chat-cancel'), false);
  } finally { release.resolve(); (fs as any).rename = original; }
  const restarted = new LocalAppAgentBroker(options);
  t.after(async () => { await restarted.dispose().catch(() => undefined); });
  assert.equal((await restarted.get(principal, input.runId)).state, 'cancelled');
});

test('unsafe in-flight fence prevents task dispatch and remains unavailable after restart', { skip: process.platform === 'win32' }, async t => {
  let executions = 0;
  const privateId = 'aabbccdd-1111-4111-8111-111111111111';
  const { root, broker, options } = await fixture(t, { uuid: () => privateId, execute: async () => {
    executions++; return { result: { text: 'unexpected', attachments: [], messageId: 'unexpected' }, usage: { turns: 1 } };
  } });
  await broker.initialize();
  const tasks = join(root, 'apps', principal.appId, 'agent', 'tasks');
  await fs.mkdir(tasks, { recursive: true });
  const fence = join(tasks, `${privateId}.json.termination`);
  await fs.symlink(join(root, 'untrusted-target'), fence);
  const input = runInput('unsafe-inflight-fence');
  await assert.rejects(broker.run(principal, input), { code: 'agent_storage_unavailable' });
  assert.equal(executions, 0);
  await assert.rejects(broker.get(principal, input.runId), { code: 'agent_storage_unavailable' });
  const restarted = new LocalAppAgentBroker(options);
  t.after(async () => { await restarted.dispose().catch(() => undefined); });
  await assert.rejects(restarted.get(principal, input.runId), { code: 'agent_storage_unavailable' });
  assert.equal((await fs.lstat(fence)).isSymbolicLink(), true);
  assert.equal(executions, 0);
});

test('cancellation write failure retains the existing fence and reports uncertainty', { skip: process.platform === 'win32' }, async t => {
  const started = deferred<LocalAppAgentExecution>();
  const completion = deferred<{ result: LocalAppAgentPrivateResult; usage: { turns: number } }>();
  const { root, broker, options } = await fixture(t, { execute: execution => { started.resolve(execution); return completion.promise; } });
  const input = runInput('cancel-storage-failure');
  await broker.run(principal, input);
  const execution = await started.promise;
  const tasks = join(root, 'apps', principal.appId, 'agent', 'tasks');
  const fence = join(tasks, `${execution.taskStorageId}.json.termination`);
  assert.equal((await fs.lstat(fence)).isFile(), true);
  await fs.chmod(tasks, 0o500);
  try {
    await assert.rejects(broker.cancelApp(principal.appId), { code: 'agent_storage_unavailable' });
    assert.equal((await fs.lstat(fence)).isFile(), true);
    await assert.rejects(broker.get(principal, input.runId), { code: 'agent_storage_unavailable' });
  } finally {
    await fs.chmod(tasks, 0o700);
    completion.resolve({ result: { text: 'answer', output: { ok: true }, attachments: [], messageId: 'assistant-cancel-storage' }, usage: { turns: 1 } });
  }
  const restarted = new LocalAppAgentBroker(options);
  t.after(async () => { await restarted.dispose().catch(() => undefined); });
  assert.equal((await restarted.get(principal, input.runId)).state, 'interrupted');
});

test('a killed host recovers an in-flight Chat as interrupted without replaying external effects', { timeout: 15000 }, async t => {
  let replayExecutions = 0;
  const { root, broker, options } = await fixture(t, { execute: async () => {
    replayExecutions++;
    return { result: { text: 'unexpected', attachments: [], messageId: 'unexpected' }, usage: { turns: 1 } };
  } });
  const loader = resolve(process.cwd(), '../../packages/bridge-client/node_modules/tsx/dist/loader.mjs');
  const script = fileURLToPath(new URL('./fixtures/local-app-agent-crash-child.ts', import.meta.url));
  const child = spawn(process.execPath, ['--import', loader, script], {
    cwd: process.cwd(), env: { ...process.env, LINGXI_TEST_AGENT_ROOT: root }, stdio: ['ignore', 'pipe', 'pipe'],
  });
  t.after(async () => { if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL'); });
  let output = '', errors = '';
  child.stderr.on('data', chunk => { errors += chunk; });
  const admitted = await new Promise<{ runId: string; chatId: string; taskId: string }>((resolveResult, reject) => {
    const timer = setTimeout(() => reject(new Error(`Crash fixture did not start: ${errors}`)), 7000);
    child.stdout.on('data', chunk => {
      output += chunk;
      if (!output.includes('\n')) return;
      clearTimeout(timer);
      try { resolveResult(JSON.parse(output.slice(0, output.indexOf('\n')))); } catch (error) { reject(error); }
    });
    child.once('error', error => { clearTimeout(timer); reject(error); });
    child.once('exit', (code, signal) => { if (!output.includes('\n')) { clearTimeout(timer); reject(new Error(`Crash fixture exited ${code}/${signal}: ${errors}`)); } });
  });
  const childExited = new Promise<void>(resolveExit => child.once('exit', () => resolveExit()));
  child.kill('SIGKILL');
  await childExited;
  const effectPath = join(root, 'external-effect.log');
  assert.equal(await fs.readFile(effectPath, 'utf8'), 'performed\n');
  const taskPath = join(root, 'apps', principal.appId, 'agent', 'tasks', `${admitted.taskId}.json`);
  const chatPath = join(root, 'apps', principal.appId, 'agent', 'chats', admitted.chatId, 'chat.json');
  const fence = `${taskPath}.termination`;
  assert.equal((await fs.lstat(fence)).isFile(), true);

  // Stop a first recovery after it has saved interrupted task and Chat rows,
  // then restart from that intermediate disk image as if the host died again.
  const beforeUnlink = deferred<void>(), releaseUnlink = deferred<void>(), original = fs.unlink;
  let hold = true;
  (fs as any).unlink = async (path: unknown) => {
    if (path === fence && hold) { hold = false; beforeUnlink.resolve(); await releaseUnlink.promise; }
    return (original as any)(path);
  };
  try {
    const recovering = broker.initialize();
    await beforeUnlink.promise;
    const stagedTask = JSON.parse(await fs.readFile(taskPath, 'utf8'));
    const stagedChat = JSON.parse(await fs.readFile(chatPath, 'utf8'));
    assert.equal(stagedTask.run.state, 'interrupted');
    assert.equal(stagedTask.interruptionPending, true);
    assert.equal(stagedChat.state, 'interrupted');
    const snapshot = await fs.mkdtemp(join(tmpdir(), 'lingxi-second-crash-snapshot-'));
    t.after(async () => { await fs.rm(snapshot, { recursive: true, force: true }); });
    await fs.cp(root, snapshot, { recursive: true });
    const second = new LocalAppAgentBroker({ ...options, root: snapshot });
    t.after(async () => { await second.dispose().catch(() => undefined); });
    const secondTask = await second.get(principal, admitted.runId);
    assert.equal(secondTask.state, 'interrupted');
    assert.equal(secondTask.error?.code, 'host_restarted');
    assert.match(secondTask.error?.message ?? '', /effects and results may be unknown/i);
    assert.equal((await second.chatHistory(principal, admitted.chatId)).messages[0]?.id, 'message-fixture-1');
    assert.equal((await second.events(principal, admitted.runId)).events.filter(event => event.type === 'RUN_ERROR').length, 1);
    await assert.rejects(fs.lstat(join(snapshot, 'apps', principal.appId, 'agent', 'tasks', `${admitted.taskId}.json.termination`)), { code: 'ENOENT' });

    // A failed fence cleanup must not subsequently run orphan repair on a
    // quarantined Chat or rewrite the preserved message bytes.
    const failedRoot = await fs.mkdtemp(join(tmpdir(), 'lingxi-recovery-failed-'));
    t.after(async () => { await fs.rm(failedRoot, { recursive: true, force: true }); });
    await fs.cp(root, failedRoot, { recursive: true });
    const failedTask = join(failedRoot, 'apps', principal.appId, 'agent', 'tasks', `${admitted.taskId}.json`);
    const failedChat = join(failedRoot, 'apps', principal.appId, 'agent', 'chats', admitted.chatId, 'chat.json');
    const withOrphan = JSON.parse(await fs.readFile(failedChat, 'utf8'));
    withOrphan.messages.push({ runId: 'unknown-run', message: { id: 'orphan-preserve', role: 'user', content: 'preserve' } });
    await fs.writeFile(failedChat, JSON.stringify(withOrphan));
    const earlierUnlink = fs.unlink;
    let atFailure: Buffer | undefined;
    (fs as any).unlink = async (path: unknown) => {
      if (path === `${failedTask}.termination`) {
        atFailure = await fs.readFile(failedChat);
        throw Object.assign(new Error('injected fence cleanup failure'), { code: 'EIO' });
      }
      return (earlierUnlink as any)(path);
    };
    const failed = new LocalAppAgentBroker({ ...options, root: failedRoot });
    t.after(async () => { await failed.dispose().catch(() => undefined); });
    try {
      await assert.rejects(failed.get(principal, admitted.runId), { code: 'agent_storage_unavailable' });
      await assert.rejects(failed.chatHistory(principal, admitted.chatId), { code: 'chat_storage_uncertain' });
      assert.ok(atFailure);
      assert.deepEqual(await fs.readFile(failedChat), atFailure);
      assert.equal(JSON.parse(atFailure.toString()).messages.at(-1).message.id, 'orphan-preserve');
    } finally { (fs as any).unlink = earlierUnlink; }

    // A second crash after fence unlink, but before clearing the durable
    // interruption marker, remains recoverable without executing anything.
    const afterUnlinkRoot = await fs.mkdtemp(join(tmpdir(), 'lingxi-recovery-unlink-window-'));
    t.after(async () => { await fs.rm(afterUnlinkRoot, { recursive: true, force: true }); });
    await fs.cp(root, afterUnlinkRoot, { recursive: true });
    const afterTask = join(afterUnlinkRoot, 'apps', principal.appId, 'agent', 'tasks', `${admitted.taskId}.json`);
    const finalRename = deferred<void>(), releaseRename = deferred<void>(), oldRename = fs.rename;
    (fs as any).rename = async (source: unknown, destination: unknown) => {
      if (destination === afterTask) { finalRename.resolve(); await releaseRename.promise; }
      return (oldRename as any)(source, destination);
    };
    const unlinkWindow = new LocalAppAgentBroker({ ...options, root: afterUnlinkRoot });
    t.after(async () => { await unlinkWindow.dispose().catch(() => undefined); });
    try {
      const recovery = unlinkWindow.initialize();
      await finalRename.promise;
      await assert.rejects(fs.lstat(`${afterTask}.termination`), { code: 'ENOENT' });
      assert.equal(JSON.parse(await fs.readFile(afterTask, 'utf8')).interruptionPending, true);
      const lastSnapshot = await fs.mkdtemp(join(tmpdir(), 'lingxi-recovery-after-unlink-'));
      t.after(async () => { await fs.rm(lastSnapshot, { recursive: true, force: true }); });
      await fs.cp(afterUnlinkRoot, lastSnapshot, { recursive: true });
      const last = new LocalAppAgentBroker({ ...options, root: lastSnapshot });
      t.after(async () => { await last.dispose().catch(() => undefined); });
      assert.equal((await last.get(principal, admitted.runId)).state, 'interrupted');
      assert.equal((await last.chatHistory(principal, admitted.chatId)).messages[0]?.id, 'message-fixture-1');
      assert.equal((await last.events(principal, admitted.runId)).events.filter(event => event.type === 'RUN_ERROR').length, 1);
      releaseRename.resolve(); await recovery;
    } finally { releaseRename.resolve(); (fs as any).rename = oldRename; }
    releaseUnlink.resolve(); await recovering;
  } finally { releaseUnlink.resolve(); (fs as any).unlink = original; }

  const task = await broker.get(principal, admitted.runId);
  assert.equal(task.state, 'interrupted');
  assert.equal(task.error?.code, 'host_restarted');
  assert.match(task.error?.message ?? '', /effects and results may be unknown/i);
  assert.equal((await broker.chatCreationStatus(principal, { requestId: admitted.chatId, title: 'Crash recovery' }))?.state, 'interrupted');
  assert.equal((await broker.chatHistory(principal, admitted.chatId)).messages[0]?.id, 'message-fixture-1');
  assert.equal((await broker.events(principal, admitted.runId)).events.filter(event => event.type === 'RUN_ERROR').length, 1);
  const retryInput = runInput(admitted.runId, admitted.chatId);
  assert.deepEqual(await broker.sendChat(principal, admitted.chatId, retryInput), { runId: admitted.runId });
  assert.equal(replayExecutions, 0);
  assert.equal(await fs.readFile(effectPath, 'utf8'), 'performed\n');
  await assert.rejects(fs.lstat(fence), { code: 'ENOENT' });
});

test('a killed stateless task becomes interrupted without repeating a possible external effect', { timeout: 15000 }, async t => {
  let replayExecutions = 0;
  const { root, broker } = await fixture(t, { execute: async () => {
    replayExecutions++;
    return { result: { text: 'unexpected', attachments: [], messageId: 'unexpected' }, usage: { turns: 1 } };
  } });
  const child = spawn(process.execPath, ['--import', resolve(process.cwd(), '../../packages/bridge-client/node_modules/tsx/dist/loader.mjs'),
    fileURLToPath(new URL('./fixtures/local-app-agent-crash-child.ts', import.meta.url))], {
    cwd: process.cwd(), env: { ...process.env, LINGXI_TEST_AGENT_ROOT: root, LINGXI_TEST_AGENT_STATELESS: '1' }, stdio: ['ignore', 'pipe', 'pipe'],
  });
  t.after(async () => { if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL'); });
  let output = '', errors = '';
  child.stderr.on('data', chunk => { errors += chunk; });
  const admitted = await new Promise<{ runId: string; threadId: string; taskId: string }>((resolveResult, reject) => {
    const timer = setTimeout(() => reject(new Error(`Crash fixture did not start: ${errors}`)), 7000);
    child.stdout.on('data', chunk => {
      output += chunk;
      if (!output.includes('\n')) return;
      clearTimeout(timer);
      try { resolveResult(JSON.parse(output.slice(0, output.indexOf('\n')))); } catch (error) { reject(error); }
    });
    child.once('error', error => { clearTimeout(timer); reject(error); });
    child.once('exit', (code, signal) => { if (!output.includes('\n')) { clearTimeout(timer); reject(new Error(`Crash fixture exited ${code}/${signal}: ${errors}`)); } });
  });
  const childExited = new Promise<void>(resolveExit => child.once('exit', () => resolveExit()));
  child.kill('SIGKILL'); await childExited;
  const task = await broker.get(principal, admitted.runId);
  assert.equal(task.state, 'interrupted');
  assert.match(task.error?.message ?? '', /effects and results may be unknown/i);
  assert.deepEqual(await broker.run(principal, runInput(admitted.runId, admitted.threadId)), { runId: admitted.runId });
  assert.equal(replayExecutions, 0);
  assert.equal(await fs.readFile(join(root, 'external-effect.log'), 'utf8'), 'performed\n');
  await assert.rejects(fs.lstat(join(root, 'apps', principal.appId, 'agent', 'tasks', `${admitted.taskId}.json.termination`)), { code: 'ENOENT' });
});

test('in-flight records with partial result evidence stay isolated on restart', async t => {
  const started = deferred<LocalAppAgentExecution>();
  const never = deferred<{ result: LocalAppAgentPrivateResult; usage: { turns: number } }>();
  const { root, broker, options } = await fixture(t, { execute: execution => { started.resolve(execution); return never.promise; } });
  const creation = { requestId: 'db111111-1111-4111-8111-111111111111' };
  const chat = await broker.createChat(principal, creation);
  const input = runInput('inflight-with-partial-result', chat.chatId);
  await broker.sendChat(principal, chat.chatId, input);
  const execution = await started.promise;
  const snapshot = await fs.mkdtemp(join(tmpdir(), 'lingxi-partial-result-snapshot-'));
  t.after(async () => { await fs.rm(snapshot, { recursive: true, force: true }); });
  await fs.cp(root, snapshot, { recursive: true });
  const path = join(snapshot, 'apps', principal.appId, 'agent', 'tasks', `${execution.taskStorageId}.json`);
  const record = JSON.parse(await fs.readFile(path, 'utf8'));
  record.events.push({ type: 'TEXT_MESSAGE_START', messageId: 'partial', role: 'assistant' });
  record.eventSequence = record.events.length;
  const bytes = Buffer.from(JSON.stringify(record));
  await fs.writeFile(path, bytes);
  const restarted = new LocalAppAgentBroker({ ...options, root: snapshot });
  t.after(async () => { await restarted.dispose().catch(() => undefined); });
  await assert.rejects(restarted.get(principal, input.runId), { code: 'agent_storage_unavailable' });
  await assert.rejects(restarted.chatCreationStatus(principal, creation), { code: 'chat_storage_uncertain' });
  assert.deepEqual(await fs.readFile(path), bytes);
  // Allow the original test fixture to finish; the snapshot must remain untouched.
  never.resolve({ result: { text: 'answer', output: { ok: true }, attachments: [], chatId: chat.chatId, messageId: 'assistant-original' }, usage: { turns: 1 } });
  await terminal(broker, principal, input.runId);
});
