import { test } from 'node:test';
import assert from 'node:assert/strict';
import { runInNewContext } from 'node:vm';
import { LOCAL_APP_SDK } from '../src/preload/localAppSdk.ts';

async function sdkReply(task: unknown): Promise<unknown> {
  const bridge = { agent: { tasks: {
    start: async () => ({ runId: 'run-1' }),
    get: async () => task,
    list: async () => [], cancel: async () => {}, subscribe: async () => {}, unsubscribe: async () => {}, onEvent: () => () => {},
  }, chats: {} } };
  const window: Record<string, unknown> = {
    fetch: globalThis.fetch.bind(globalThis),
    location: { protocol: 'lingxi-app:', host: 'test-app', href: 'lingxi-app://test-app/' },
  };
  window.top = window;
  window.__lingxiLocalAppBridge = bridge;
  runInNewContext(LOCAL_APP_SDK, { window, setTimeout, clearTimeout, Request, Headers, URL });
  return (window.localApp as any).agent.run({ requestId: 'request-1', prompt: 'hello' });
}

test('SDK returns only finite nonnegative known usage metrics from a successful task', async () => {
  const reply = await sdkReply({ runId: 'run-1', state: 'succeeded', result: { text: 'answer', attachments: [] },
    usage: { inputTokens: 12, outputTokens: 5, costUsd: 0.01, costEstimated: true, turns: 2, internal: 'discard' } });
  assert.deepEqual(JSON.parse(JSON.stringify(reply)), { runId: 'run-1', text: 'answer', attachments: [],
    usage: { inputTokens: 12, outputTokens: 5, costUsd: 0.01, costEstimated: true, turns: 2 } });
});

test('SDK rejects successful tasks with missing or malformed usage instead of fabricating an empty object', async () => {
  const base = { runId: 'run-1', state: 'succeeded', result: { text: 'answer', attachments: [] } };
  await assert.rejects(sdkReply(base), error => (error as { code?: string }).code === 'invalid_result');
  await assert.rejects(sdkReply({ ...base, usage: { inputTokens: -1 } }), error => (error as { code?: string }).code === 'invalid_result');
  await assert.rejects(sdkReply({ ...base, usage: { costEstimated: 'true' } }), error => (error as { code?: string }).code === 'invalid_result');
});
