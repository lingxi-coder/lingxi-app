import './support/remoteSharedUi';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import test from 'node:test';
import { createElement } from 'react';
import type { ChatEvent, ChatTurn, VaultProvider, VaultUpsert } from '../../../packages/llm-vault/contracts';
import { protocolOfDesktopType, desktopTypeOfProtocol, isProviderId } from '../../../packages/llm-vault/contracts';
import { AgentHost } from '../src/chat/agent/AgentPane';
import { createLlmClient, LlmError, type LlmClient } from '../src/chat/agent/client';
import { contextOf, memoryHistory, titleOf, type AgentMessage } from '../src/chat/agent/history';
import { PRESETS } from '../src/chat/agent/ProviderSettings';
import { createSseParser } from '../src/chat/agent/sse';
import type { ChatTarget } from '../src/app/router';

const { create, act } = createRequire(import.meta.url)('react-test-renderer');
const textOf = (node: any): string => typeof node === 'string' ? node : (node?.children ?? []).map(textOf).join('');
const flush = () => act(async () => { for (let i = 0; i < 5; i++) await new Promise(resolve => setImmediate(resolve)); });

test('agent sse: events come out whole however the text is split', () => {
  const text = 'event: start\ndata: {"provider":"deepseek","model":"m"}\n\n: ping\n\nevent: delta\r\ndata: {"text":"你好"}\r\n\r\nevent: done\ndata: {"reason":"stop"}\n\n';
  for (const size of [1, 3, 8, 1000]) {
    const parser = createSseParser(), out: ChatEvent[] = [];
    for (let i = 0; i < text.length; i += size) out.push(...parser.push(text.slice(i, i + size)));
    out.push(...parser.end());
    assert.deepEqual(out, [{ type: 'start', provider: 'deepseek', model: 'm' }, { type: 'delta', text: '你好' }, { type: 'done', reason: 'stop' }], `size ${size}`);
  }
});

const streamOf = (...parts: string[]) => new ReadableStream<Uint8Array>({ start(controller) { for (const part of parts) controller.enqueue(new TextEncoder().encode(part)); controller.close(); } });
test('agent client: requests carry the token and no cookies, and a stream ends with its last event', async () => {
  const seen: { url: string; init: RequestInit }[] = [];
  const fetcher = (async (url: string, init: RequestInit) => {
    seen.push({ url, init });
    if (url.endsWith('/providers') && init.method !== 'POST') return new Response(JSON.stringify({ providers: [{ id: 'deepseek' }] }));
    if (url.endsWith('/chat')) return new Response(streamOf('event: delta\ndata: {"text":"a"}\n\nevent: de', 'lta\ndata: {"text":"b"}\n\nevent: done\ndata: {"reason":"stop"}\n\n'));
    return new Response(JSON.stringify({ error: { code: 'UPSTREAM_AUTH' } }), { status: 502 });
  }) as unknown as typeof fetch;
  const client = createLlmClient(async () => 'id-token', 'https://llm.example/llmApi', fetcher);
  assert.deepEqual(await client.listProviders(), [{ id: 'deepseek' }]);
  const events: ChatEvent[] = [];
  const last = await client.streamChat({ providerId: 'deepseek', model: 'm', messages: [{ role: 'user', content: 'hi' }] }, event => events.push(event));
  assert.deepEqual(events.map(event => event.type), ['delta', 'delta', 'done']);
  assert.deepEqual(last, { type: 'done', reason: 'stop' });
  assert.equal(seen[0]!.url, 'https://llm.example/llmApi/providers');
  assert.equal((seen[0]!.init.headers as Record<string, string>).Authorization, 'Bearer id-token');
  assert.equal(seen[0]!.init.credentials, 'omit');
  await assert.rejects(client.testProvider('deepseek'), (error: unknown) => error instanceof LlmError && error.code === 'UPSTREAM_AUTH' && error.status === 502);
});

test('agent client: a stream that stops without done or error is INTERRUPTED, and a lost connection is NETWORK', async () => {
  const cut = createLlmClient(async () => 't', 'https://x', (async () => new Response(streamOf('event: delta\ndata: {"text":"a"}\n\n'))) as unknown as typeof fetch);
  assert.deepEqual(await cut.streamChat({ providerId: 'p', model: 'm', messages: [] }, () => undefined), { type: 'error', code: 'INTERRUPTED' });
  const offline = createLlmClient(async () => 't', 'https://x', (async () => { throw new TypeError('Failed to fetch'); }) as unknown as typeof fetch);
  await assert.rejects(offline.listProviders(), { code: 'NETWORK' });
});

test('agent history: titles, the context window and the per-user limit', async () => {
  assert.equal(titleOf('  How do I\n  write a haiku?  '), 'How do I write a haiku?');
  assert.equal(titleOf('x'.repeat(60)).length, 40);
  const m = (role: 'user' | 'assistant', content: string, more: Partial<AgentMessage> = {}): AgentMessage => ({ id: content, role, content, at: 0, ...more });
  assert.deepEqual(contextOf([m('user', 'a'), m('assistant', 'b'), m('user', 'c')]), [{ role: 'user', content: 'a' }, { role: 'assistant', content: 'b' }, { role: 'user', content: 'c' }]);
  assert.deepEqual(contextOf([m('user', 'a'), m('assistant', '', { error: 'TIMEOUT' }), m('user', 'c')]), [{ role: 'user', content: 'a\n\nc' }], 'an empty failed answer is dropped and the two prompts join');
  assert.deepEqual(contextOf([m('user', 'x'.repeat(50)), m('assistant', 'y'.repeat(50)), m('user', 'z'.repeat(30))], 100), [{ role: 'user', content: 'z'.repeat(30) }], 'older turns beyond the budget drop, never leaving an answer first');
  assert.deepEqual(contextOf([m('user', 'q'.repeat(500))], 100), [{ role: 'user', content: 'q'.repeat(100) }]);

  const history = memoryHistory(2);
  for (const [id, updatedAt] of [['a', 1], ['b', 3], ['c', 2]] as const) await history.put({ uid: 'u', id, title: id, createdAt: 0, updatedAt, messages: [] });
  await history.put({ uid: 'other', id: 'z', title: 'z', createdAt: 0, updatedAt: 0, messages: [] });
  assert.deepEqual((await history.list('u')).map(chat => chat.id), ['b', 'c'], 'the oldest goes beyond the limit');
  assert.equal(await history.get('u', 'z'), undefined, 'chats are per user');
});

test('llm-vault contracts: ids and Desktop provider types map both ways', () => {
  assert.ok(isProviderId('deepseek') && !isProviderId('builtin') && !isProviderId('Deep'));
  assert.equal(protocolOfDesktopType('anthropic'), 'anthropic_messages');
  assert.equal(protocolOfDesktopType('azure-openai'), 'other');
  assert.equal(desktopTypeOfProtocol('gemini_generate_content'), 'gemini');
  assert.ok(PRESETS.some(preset => preset.id === 'deepseek' && preset.protocol === 'open_ai_chat'));
});

const deepseek: VaultProvider = { id: 'deepseek', presetId: 'deepseek', label: 'DeepSeek', protocol: 'open_ai_chat', desktopType: null, baseUrl: 'https://api.deepseek.com',
  models: [{ id: 'deepseek-flash', name: 'DeepSeek V4.1 Flash' }], defaultModel: 'deepseek-flash', keyHint: 'sk-…0001', keyFingerprint: 'k1:x', source: 'desktop',
  revision: 1, webUsable: true, createdAt: 0, updatedAt: 0 };

function fakeClient(providers: VaultProvider[], stream: LlmClient['streamChat'] = async (_turn, onEvent) => { onEvent({ type: 'delta', text: 'Hel' }); onEvent({ type: 'delta', text: 'lo **there**' }); const done: ChatEvent = { type: 'done', reason: 'stop' }; onEvent(done); return done; }) {
  const calls: { upserts: VaultUpsert[]; turns: ChatTurn[]; tests: string[] } = { upserts: [], turns: [], tests: [] };
  const client: LlmClient = {
    listProviders: async () => providers,
    upsertProvider: async input => { calls.upserts.push(input); return { ...deepseek, id: input.id, source: 'web' }; },
    deleteProvider: async () => undefined,
    testProvider: async id => { calls.tests.push(id); return { ok: true }; },
    streamChat: async (turn, onEvent, signal) => { calls.turns.push(turn); return stream(turn, onEvent, signal); },
  };
  return { client, calls };
}

function mount(client: LlmClient, target: ChatTarget = { kind: 'home' }, history = memoryHistory()) {
  const opened: [ChatTarget, boolean | undefined][] = [], lists: unknown[] = [];
  const props = { uid: 'alice', name: 'Alice', locale: 'en', client, history, frame: (callback: () => void) => callback(),
    onAgentChats: (chats: unknown) => lists.push(chats), open: (next: ChatTarget, replace?: boolean) => opened.push([next, replace]) };
  let root: any;
  act(() => { root = create(createElement(AgentHost, { ...props, target })); });
  return { root, opened, lists, history, rerender: (next: ChatTarget) => act(() => root.update(createElement(AgentHost, { ...props, target: next }))) };
}
const type = (root: any, text: string) => act(() => root.root.findByType('textarea').props.onChange({ target: { value: text } }));
const button = (root: any, label: string) => root.root.find((node: any) => node.type === 'button' && (node.props['aria-label'] === label || textOf(node) === label));

test('agent pane: without a model service it says so, and sending opens the setup with the draft kept', async () => {
  const { client } = fakeClient([]);
  const { root } = mount(client);
  await flush();
  assert.match(textOf(root.toJSON()), /Connect a model service to start chatting/u);
  type(root, 'Draft question');
  act(() => button(root, 'Send').props.onClick());
  const dialog = root.root.findByProps({ role: 'dialog' });
  assert.match(textOf(dialog), /DeepSeek/u);
  assert.equal(root.root.findByType('textarea').props.value, 'Draft question');
});

test('agent pane: a sent prompt becomes a chat, streams its answer and is kept in this browser', async () => {
  const { client, calls } = fakeClient([deepseek]);
  const { root, opened, lists, history, rerender } = mount(client);
  await flush();
  type(root, 'Say hello');
  act(() => button(root, 'Send').props.onClick());
  await flush();
  assert.equal(opened[0]![0].kind, 'agent');
  assert.equal(opened[0]![1], true, 'the new chat takes over the URL without a history entry');
  const id = (opened[0]![0] as { id: string }).id;
  rerender({ kind: 'agent', id });
  await flush();
  assert.deepEqual(calls.turns[0], { providerId: 'deepseek', model: 'deepseek-flash', messages: [{ role: 'user', content: 'Say hello' }] });
  const text = textOf(root.toJSON());
  assert.match(text, /Say hello/u);
  assert.match(text, /Hello there/u);
  const saved = await history.get('alice', id);
  assert.equal(saved?.title, 'Say hello');
  assert.deepEqual(saved?.messages.map(message => [message.role, message.content]), [['user', 'Say hello'], ['assistant', 'Hello **there**']]);
  assert.ok(lists.some(list => Array.isArray(list) && list.some((chat: any) => chat.id === id)), 'the sidebar learns of the chat');
});

test('agent pane: Stop ends the answer and keeps what arrived', async () => {
  const { client } = fakeClient([deepseek], (_turn, onEvent, signal) => new Promise((_, reject) => {
    onEvent({ type: 'delta', text: 'Partial' });
    signal?.addEventListener('abort', () => reject(Object.assign(new Error('aborted'), { name: 'AbortError' })));
  }));
  const { root, opened, history, rerender } = mount(client);
  await flush();
  type(root, 'Long question');
  act(() => button(root, 'Send').props.onClick());
  rerender(opened[0]![0]);
  await flush();
  act(() => button(root, 'Stop').props.onClick());
  await flush();
  const text = textOf(root.toJSON());
  assert.match(text, /Partial/u);
  assert.match(text, /Stopped/u);
  const saved = await history.get('alice', (opened[0]![0] as { id: string }).id);
  assert.equal(saved?.messages.at(-1)?.stopped, true);
});

test('agent pane: an upstream error shows its meaning, and Retry asks again', async () => {
  let attempt = 0;
  const { client, calls } = fakeClient([deepseek], async (_turn, onEvent) => {
    attempt++;
    const last: ChatEvent = attempt === 1 ? { type: 'error', code: 'UPSTREAM_AUTH' } : { type: 'done', reason: 'stop' };
    if (attempt > 1) onEvent({ type: 'delta', text: 'Second try' });
    onEvent(last); return last;
  });
  const { root, opened, rerender } = mount(client);
  await flush();
  type(root, 'Hi');
  act(() => button(root, 'Send').props.onClick());
  rerender(opened[0]![0]);
  await flush();
  assert.match(textOf(root.toJSON()), /rejected this key/u);
  act(() => button(root, 'Retry').props.onClick());
  await flush();
  assert.equal(calls.turns.length, 2);
  assert.deepEqual(calls.turns[1]!.messages, [{ role: 'user', content: 'Hi' }]);
  assert.match(textOf(root.toJSON()), /Second try/u);
  assert.doesNotMatch(textOf(root.toJSON()), /rejected this key/u);
});

test('agent pane: a link to a chat this browser does not have says so', async () => {
  const { client } = fakeClient([deepseek]);
  const { root } = mount(client, { kind: 'agent', id: 'not-here' });
  await flush();
  assert.match(textOf(root.toJSON()), /not in this browser/u);
});

test('model settings: a preset key is sent once, cleared from the form and checked', async () => {
  const { client, calls } = fakeClient([]);
  const { root } = mount(client, { kind: 'providers' });
  await flush();
  act(() => root.root.find((node: any) => node.type === 'button' && /^DeepSeek/u.test(node.props['aria-label'] ?? '')).props.onClick());
  const key = root.root.find((node: any) => node.type === 'input' && node.props.type === 'password');
  act(() => key.props.onChange({ target: { value: '  sk-test-0001  ' } }));
  await act(async () => { root.root.findByType('form').props.onSubmit({ preventDefault() {} }); });
  await flush();
  assert.deepEqual(calls.upserts, [{ action: 'upsert', id: 'deepseek', presetId: 'deepseek', apiKey: 'sk-test-0001' }]);
  assert.deepEqual(calls.tests, ['deepseek']);
  assert.match(textOf(root.toJSON()), /DeepSeek is connected/u);
  assert.equal(root.root.findAll((node: any) => node.type === 'input' && node.props.type === 'password').length, 0, 'the form closed with the key gone');
});

test('model settings: saved services show where their key came from and never the key', async () => {
  const { client } = fakeClient([deepseek, { ...deepseek, id: 'azure', label: 'Azure', presetId: null, protocol: 'other', webUsable: false, source: 'desktop' }]);
  const { root } = mount(client, { kind: 'providers' });
  await flush();
  const text = textOf(root.toJSON());
  assert.match(text, /sk-…0001 · From the desktop app/u);
  assert.match(text, /Desktop only/u);
});
