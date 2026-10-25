import { test } from 'node:test';
import assert from 'node:assert/strict';
import { File } from 'node:buffer';
import { webcrypto } from 'node:crypto';
import { runInNewContext } from 'node:vm';
import type { BaseEvent, RunAgentInput } from '@lingxi/local-app-agui';
import { LOCAL_APP_SDK } from '../src/preload/localAppSdk.ts';

function createPage() {
  const calls: Array<{ method: string; args?: unknown }> = [];
  const dispatched: Array<{ type: string; detail?: unknown }> = [];
  let activeInput: RunAgentInput | undefined;
  let history: unknown[] = [];
  const historyCursors = new Map<string, number>();
  const sandbox: Record<string, any> = {
    URL,
    Request,
    Headers,
    File,
    ArrayBuffer,
    TextEncoder,
    AbortController,
    crypto: webcrypto,
    btoa: globalThis.btoa,
    setTimeout,
    clearTimeout,
    CustomEvent: class {
      type: string;
      detail: unknown;
      constructor(type: string, options: { detail?: unknown } = {}) { this.type = type; this.detail = options.detail; }
    },
    location: { protocol: 'lingxi-app:', host: 'test-app', href: 'lingxi-app://test-app/' },
    fetch: globalThis.fetch.bind(globalThis),
    dispatchEvent: (event: { type: string; detail?: unknown }) => { dispatched.push(event); return true; },
  };
  sandbox.window = sandbox;
  sandbox.top = sandbox;
  const fromJson = runInNewContext('(json) => JSON.parse(json)', sandbox) as (json: string) => any;
  sandbox.structuredClone = runInNewContext('(value) => JSON.parse(JSON.stringify(value))', sandbox);
  const inPage = (value: unknown) => fromJson(JSON.stringify(value));

  const runEvents = (input: RunAgentInput): BaseEvent[] => inPage([
    { type: 'RUN_STARTED', threadId: input.threadId, runId: input.runId, protocolVersion: '1.0' },
    { type: 'TEXT_MESSAGE_START', messageId: 'assistant-1', role: 'assistant' },
    { type: 'TEXT_MESSAGE_CONTENT', messageId: 'assistant-1', delta: 'answer' },
    { type: 'TEXT_MESSAGE_END', messageId: 'assistant-1' },
    { type: 'RUN_FINISHED', threadId: input.threadId, runId: input.runId, outcome: { type: 'success' }, result: 'answer' },
  ]);

  const bridge = {
    info: async () => inPage({ id: 'app-one', name: 'Meal', revision: 1, platform: 'desktop', services: ['api'] }),
    clipboard: { read: async () => '', write: async () => {} },
    notify: async () => {},
    process: { exec: async () => ({ exitCode: 0, stdout: '', stderr: '' }) },
    serviceSocketUrl: async () => 'ws://localhost/__services/api/',
    files: {
      pick: async () => inPage({ files: [] }),
      pickDirectory: async () => null,
      read: async () => '', write: async () => {}, list: async () => [],
    },
    agent: {
      tasks: {
        start: async (input: RunAgentInput) => {
          calls.push({ method: 'agent.tasks.start', args: JSON.parse(JSON.stringify(input)) });
          activeInput = input;
          return inPage({ runId: input.runId });
        },
        get: async (runId: string) => inPage({
          runId, threadId: activeInput?.threadId ?? 'thread-one', state: 'running', sequence: 1,
          createdAt: 1, updatedAt: 1, phase: 'thinking',
        }),
        list: async options => {
          calls.push({ method: 'agent.tasks.list', args: options });
          return inPage({ tasks: [], nextCursor: 'next-task' });
        },
        events: async (request: { runId: string; afterSequence?: number }) => {
          calls.push({ method: 'agent.tasks.events', args: request });
          const input = activeInput ?? { threadId: 'thread-one', runId: request.runId, messages: [] } as RunAgentInput;
          return inPage({ events: runEvents({ ...input, runId: request.runId }), sequence: 5 });
        },
        cancel: async (runId: string) => { calls.push({ method: 'agent.tasks.cancel', args: { runId } }); },
      },
      chats: {
        create: async () => inPage({ chatId: 'chat-one', title: 'Meal', createdAt: 1, updatedAt: 1, archived: false, state: 'idle' }),
        open: async (chatId: string) => inPage({ chatId, title: 'Meal', createdAt: 1, updatedAt: 1, archived: false, state: 'idle' }),
        get: async (chatId: string) => inPage({ chatId, title: 'Meal', createdAt: 1, updatedAt: 1, archived: false, state: 'idle' }),
        list: async () => inPage({ chats: [{ chatId: 'chat-one', title: 'Meal', createdAt: 1, updatedAt: 1, archived: false, state: 'idle' }] }),
        archive: async () => {},
        delete: async () => {},
        history: async (chatId: string, options: { cursor?: string; limit?: number }) => {
          calls.push({ method: 'agent.chats.history', args: JSON.parse(JSON.stringify({ chatId, ...options })) });
          const offset = options.cursor === undefined ? 0 : historyCursors.get(options.cursor);
          if (offset === undefined) throw Object.assign(new Error('invalid opaque cursor'), { code: 'invalid_cursor' });
          const limit = options.limit ?? 50;
          const messages = history.slice(offset, offset + limit);
          const nextOffset = offset + messages.length;
          let nextCursor: string | undefined;
          if (nextOffset < history.length) {
            nextCursor = `opaque-offset-${nextOffset}-${historyCursors.size + 1}`;
            historyCursors.set(nextCursor, nextOffset);
          }
          return inPage({ messages, ...(nextCursor === undefined ? {} : { nextCursor }) });
        },
        send: async (chatId: string, input: RunAgentInput) => {
          calls.push({ method: 'agent.chats.send', args: { chatId, input: JSON.parse(JSON.stringify(input)) } });
          activeInput = input;
          return inPage({ runId: input.runId });
        },
        cancel: async () => {},
      },
    },
  };
  sandbox.__lingxiLocalAppBridge = bridge;
  runInNewContext(LOCAL_APP_SDK, sandbox);

  return {
    api: sandbox.localApp as any,
    calls,
    dispatched,
    inPage,
    setHistory(messages: unknown[]) { history = messages; },
  };
}

test('Local App Agent uses official RunAgentInput, ContentPart, BaseEvent and RunAgentResult', async () => {
  const page = createPage();
  const photo = await page.api.agent.file(new File([new Uint8Array([0x89, 0x50, 0x4e, 0x47])], 'photo.png', { type: 'image/png' }));
  const input: RunAgentInput = page.inPage({
    threadId: 'thread-one', runId: 'run-one',
    messages: [{ id: 'user-one', role: 'user', content: [{ type: 'text', text: 'Inspect this.' }, photo] }],
  });
  const result = await page.api.agent.run(input);

  const start = page.calls.find(call => call.method === 'agent.tasks.start');
  assert.ok(start);
  const sentInput = start.args as RunAgentInput;
  assert.equal(sentInput.runId, input.runId);
  assert.equal(sentInput.threadId, input.threadId);
  assert.equal(sentInput.protocolVersion, '1.0');
  assert.deepEqual(sentInput.tools, []);
  assert.deepEqual(sentInput.context, []);
  assert.deepEqual(sentInput.state, {});
  assert.equal(sentInput.forwardedProps?.['com.lingxi.localApp']?.version, 1);
  const sentPart = (sentInput.messages[0]!.content as Array<{ type: string; source?: { type: string; value: string; mimeType?: string }; metadata?: Record<string, unknown> }>)[1]!;
  assert.equal(sentPart.type, 'image');
  assert.equal(sentPart.source?.type, 'data');
  assert.equal(sentPart.source?.value, 'iVBORw==');
  assert.equal(sentPart.source?.mimeType, 'image/png');
  assert.deepEqual(JSON.parse(JSON.stringify(result)), {
    result: 'answer',
    newMessages: [{ id: 'assistant-1', role: 'assistant', content: 'answer' }],
  });

  const delivered: BaseEvent[] = [];
  const stop = page.api.agent.tasks.subscribe(input.runId, (event: BaseEvent) => delivered.push(event));
  await new Promise(resolve => setTimeout(resolve, 25));
  stop();
  assert.deepEqual(delivered.map(event => event.type), ['RUN_STARTED', 'TEXT_MESSAGE_START', 'TEXT_MESSAGE_CONTENT', 'TEXT_MESSAGE_END', 'RUN_FINISHED']);
  assert.equal(page.calls.some(call => call.method === 'agent.tasks.events'), true);
  assert.equal(page.api.agent.answer, undefined);
  assert.equal(page.api.agent.tasks.state, undefined);
});

test('cold AbstractAgent reconnect reads standard event history without issuing a start', async () => {
  const page = createPage();
  const client = page.api.agent.createClient({ threadId: 'thread-one' });
  const result = await client.connectAgent({ runId: 'existing-run' });
  assert.equal(result.result, 'answer');
  assert.equal(result.newMessages[0]?.role, 'assistant');
  assert.equal(page.calls.some(call => call.method === 'agent.tasks.start'), false);
  assert.equal(page.calls.filter(call => call.method === 'agent.tasks.events').length, 1);
});

test('persistent chat uses canonical chat resources and standard Message history', async () => {
  const page = createPage();
  const chat = await page.api.agent.chats.create({ title: 'Meal' });
  const userMessage = page.inPage({ id: 'user-chat-1', role: 'user', content: 'hello' });
  page.setHistory([userMessage]);
  const history = await chat.history(page.inPage({ limit: 25 }));
  assert.equal(history.messages[0]?.id, 'user-chat-1');
  const input = page.inPage({ threadId: chat.id, runId: 'chat-run-1', messages: [userMessage] });
  const result = await chat.send(input);
  assert.equal(result.result, 'answer');
  const sent = page.calls.find(call => call.method === 'agent.chats.send');
  assert.ok(sent);
  assert.equal((sent.args as { chatId: string }).chatId, chat.id);
  assert.equal(((sent.args as { input: RunAgentInput }).input).threadId, chat.id);
  assert.equal(page.api.agent.tasks.state, undefined);
});

test('chat subscription resumes from the last opaque page cursor and deduplicates its replay', async () => {
  const page = createPage();
  const chat = await page.api.agent.chats.create(page.inPage({ title: 'History' }));
  const existing = Array.from({ length: 60 }, (_, index) => page.inPage({
    id: `message-${index}`, role: 'user', content: `message ${index}`,
  }));
  page.setHistory(existing);
  const delivered: string[] = [];
  const stop = chat.subscribe(message => delivered.push(message.id));
  await new Promise(resolve => setTimeout(resolve, 50));
  assert.equal(delivered.length, 60);

  const appended = page.inPage({ id: 'message-appended', role: 'user', content: 'new message' });
  page.setHistory([...existing, appended]);
  await new Promise(resolve => setTimeout(resolve, 1100));
  stop();

  assert.equal(delivered.filter(id => id === 'message-appended').length, 1);
  assert.equal(new Set(delivered).size, 61);
  const historyCalls = page.calls.filter(call => call.method === 'agent.chats.history');
  assert.deepEqual(historyCalls.map(call => (call.args as { cursor?: string }).cursor), [undefined, 'opaque-offset-50-1', 'opaque-offset-50-1']);
  assert.equal(page.dispatched.some(event => event.type === 'localApp:chat-history-error'), false);
});
