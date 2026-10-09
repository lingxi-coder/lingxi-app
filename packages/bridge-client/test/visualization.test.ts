import assert from 'node:assert/strict';
import { test } from 'node:test';
import { BridgeClient } from '../src/client.js';
import type { Frame } from '../src/protocol.js';
import {
  validateClientEvent,
  validateVisualizationList,
  validateVisualizationMount,
  validateVisualizationServe,
  validateVisualizationStateWrite,
} from '../src/validation.js';

function wire(reply: unknown) {
  const client = new BridgeClient();
  const internal = client as unknown as {
    ws: unknown; onMessage(data: Buffer): void; pendingResponses: Map<number, unknown>;
  };
  const sent: Array<Extract<Frame, { type: 'request' }>> = [];
  internal.ws = { readyState: 1, send(value: string) {
    const frame = JSON.parse(value) as Extract<Frame, { type: 'request' }>;
    sent.push(frame);
    internal.onMessage(Buffer.from(JSON.stringify({
      type: 'response', payload: { id: frame.payload.id, result: reply },
    })));
  } };
  return { client, internal, sent };
}

const TOKEN = 'a'.repeat(64);

test('visualization_block events carry a reference exactly when ready', () => {
  assert.deepEqual(
    validateClientEvent({ type: 'visualization_block', status: 'ready', reference: { id: 'chart', revision: 2 } }),
    { type: 'visualization_block', status: 'ready', reference: { id: 'chart', revision: 2 } },
  );
  for (const status of ['pending', 'unavailable', 'discarded']) {
    assert.deepEqual(validateClientEvent({ type: 'visualization_block', status }), { type: 'visualization_block', status });
  }
  for (const bad of [
    { type: 'visualization_block', status: 'ready' },
    { type: 'visualization_block', status: 'pending', reference: { id: 'chart', revision: 2 } },
    { type: 'visualization_block', status: 'ready', reference: { id: '../x', revision: 2 } },
    { type: 'visualization_block', status: 'ready', reference: { id: 'chart', revision: 0 } },
    { type: 'visualization_block', status: 'ready', reference: { id: 'chart', revision: 1, extra: 1 } },
    { type: 'visualization_block', status: 'done' },
    { type: 'visualization_block', status: 'pending', extra: true },
  ]) {
    assert.throws(() => validateClientEvent(bad), undefined, JSON.stringify(bad));
  }
});

test('session agent transcripts accept visualization blocks and follow-up context', () => {
  const message = {
    role: 'user',
    blocks: [{ type: 'visualization', reference: { id: 'chart', revision: 1 } }, { type: 'visualization' }],
    visualization_context: { id: 'chart', revision: 1, title: 'Sales' },
  };
  const event = validateClientEvent({
    type: 'session_agent_transcript',
    session_id: 's',
    agent_id: 'a',
    messages: [{ message_index: 0, message_uuid: 'u', message }],
    next_message_index: 1,
    revision: 1,
  });
  assert.equal(event.type, 'session_agent_transcript');
  assert.throws(() => validateClientEvent({
    type: 'session_agent_transcript',
    session_id: 's',
    agent_id: 'a',
    messages: [{ message_index: 0, message_uuid: 'u', message: { ...message, visualization_context: { id: 'chart', revision: 1 } } }],
    next_message_index: 1,
    revision: 1,
  }));
});

test('mount, serve and state writes ride correlated visualization requests', async () => {
  const mount = { token: TOKEN, generation: 3, doc_url: 'lingxi-viz://visualization/doc/' + TOKEN, title: 'Chart' };
  const fixture = wire(mount);
  const theme = { dark: true, tokens: { '--color-background-primary': '#000' } };
  assert.deepEqual(await fixture.client.visualizationMount('sess', { id: 'chart', revision: 1 }, theme, 'en', false), mount);
  assert.equal(fixture.sent[0]?.payload.method, 'visualization');
  assert.deepEqual(fixture.sent[0]?.payload.params, {
    op: 'mount', session_id: 'sess', id: 'chart', revision: 1, theme, locale: 'en', expanded: false,
  });
  assert.equal(fixture.internal.pendingResponses.size, 0);
  assert.equal(await wire(null).client.visualizationMount('sess', { id: 'chart', revision: 1 }, theme, 'en', false), null);

  const served = { status: 200, headers: [['Content-Type', 'text/html; charset=utf-8']], body_base64: 'PGgxPg==' };
  assert.deepEqual(await wire(served).client.visualizationServe('/shell.html'), served);

  const saved = { saved: true, version: 4 };
  const write = wire(saved);
  assert.deepEqual(await write.client.visualizationWriteState(TOKEN, 3, 3, '{}', 'null'), saved);
  assert.deepEqual(write.sent[0]?.payload.params, {
    op: 'write_state', token: TOKEN, generation: 3, base_version: 3, model_content: '{}', private_content: 'null',
  });
});

test('visualization responses are validated strictly', () => {
  for (const bad of [
    { token: 'short', generation: 1, doc_url: 'x', title: 't' },
    { token: TOKEN, generation: 0, doc_url: 'x', title: 't' },
    { token: TOKEN, generation: 1, doc_url: 'x', title: 't', extra: 1 },
  ]) assert.throws(() => validateVisualizationMount(bad));
  assert.throws(() => validateVisualizationServe({ status: 500, headers: [], body_base64: '' }));
  assert.throws(() => validateVisualizationServe({ status: 200, headers: [['a']], body_base64: '' }));
  assert.throws(() => validateVisualizationStateWrite({ saved: true, version: 1, reason: 'conflict' }));
  assert.throws(() => validateVisualizationStateWrite({ saved: false, version: 1 }));
  assert.deepEqual(
    validateVisualizationStateWrite({ saved: false, version: 2, reason: 'conflict', current_state: { version: 2 } }),
    { saved: false, version: 2, reason: 'conflict', current_state: { version: 2 } },
  );
  assert.deepEqual(validateVisualizationList([{ id: 'v1', revision: 1, title: 't', created_at_ms: 5 }]), [
    { id: 'v1', revision: 1, title: 't', created_at_ms: 5 },
  ]);
  assert.throws(() => validateVisualizationList([{ id: 'v 1', revision: 1, title: 't', created_at_ms: 5 }]));
});

test('send_prompt carries the follow-up visualization context only when given', () => {
  const fixture = wire(undefined);
  fixture.client.sendPrompt('why?', { visualizationContext: { id: 'chart', revision: 2 } });
  fixture.client.sendPrompt('plain');
  assert.deepEqual(fixture.sent[0]?.payload.params, {
    type: 'send_prompt', text: 'why?', images: [], visualization_context: { id: 'chart', revision: 2 },
  });
  assert.deepEqual(fixture.sent[1]?.payload.params, { type: 'send_prompt', text: 'plain', images: [] });
});
