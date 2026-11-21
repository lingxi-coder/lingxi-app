import assert from 'node:assert/strict';
import { test } from 'node:test';
import { buildRequest, normalize, parseSse } from '../src/llm-upstream.mjs';

const tool = { name: 'dataset_put', description: 'Write a proposed Dataset row', parameters: {
  type: 'object', required: ['qty'], properties: { qty: { type: 'integer' } },
} };
const tools = [tool];
const user = { role: 'user', content: '记录西区-A的库存' };
const chatDelta = (delta, finish_reason) => ({ choices: [{ delta, finish_reason }] });
const chatTool = (index, fn, id) => chatDelta({ tool_calls: [{ index, id, function: fn }] });

async function events(protocol, frames, options = {}) {
  const wire = frames.map(frame => 'data: ' + (typeof frame === 'string' ? frame : JSON.stringify(frame)) + '\r\n\r\n').join('');
  const bytes = Buffer.from(wire);
  async function* chunks() {
    for (let offset = 0; offset < bytes.length; offset += 3) yield bytes.subarray(offset, offset + 3);
  }
  const result = [];
  for await (const event of normalize(protocol, parseSse(chunks()), { tools, ...options })) result.push(event);
  return result;
}

function continuation(protocol, round) {
  const results = Object.fromEntries(round.calls.map(({ id }) => [id, { revision: 1 }]));
  const request = buildRequest({ protocol, baseUrl: 'https://model.example/v1' }, 'test-only-key', {
    model: 'test-model', messages: [user], tools, rounds: [{ ...round, results }],
  });
  return JSON.parse(request.body);
}

test('Chat Completions assembles interleaved indexed calls and returns matching results', async () => {
  const protocol = 'open_ai_chat';
  const result = await events(protocol, [
    chatTool(1, { name: 'dataset_put', arguments: '{"qty":' }, 'call_b'),
    chatTool(0, { name: 'dataset_put', arguments: '{"qty":' }, 'call_a'),
    chatTool(1, { arguments: '0}' }),
    chatTool(0, { arguments: '9}' }),
    chatDelta({}, 'tool_calls'), '[DONE]',
  ]);
  assert.equal(result.length, 1);
  const round = result[0];
  assert.deepEqual(round.calls.map(({ id, arguments: args }) => [id, args.qty]), [['call_a', 9], ['call_b', 0]]);
  const request = continuation(protocol, round);
  assert.deepEqual(request.tools, [{ type: 'function', function: tool }]);
  assert.deepEqual(request.messages.slice(2), ['call_a', 'call_b'].map(id => ({
    role: 'tool', tool_call_id: id, content: '{"revision":1}',
  })));
});

test('Gemini Chat Completions accepts its unindexed terminal call and returns the thought signature', async () => {
  const protocol = 'open_ai_chat';
  const extra = { google: { thought_signature: 'synthetic' } };
  const call = { id: 'call_a', type: 'function', function: { name: 'dataset_put', arguments: '{"qty":1}' }, extra_content: extra };
  const observed = chatDelta({ tool_calls: [call] });
  const complete = [observed, chatDelta({}, 'stop'), '[DONE]'];
  const [round] = await events(protocol, complete, { geminiCompatibility: true });
  assert.deepEqual(round.calls, [{ id: 'call_a', name: 'dataset_put', arguments: { qty: 1 } }]);
  assert.deepEqual(continuation(protocol, round).messages[1].tool_calls, [call]);
  const rejected = [{ type: 'error', code: 'INVALID_TOOL_ROUND' }];
  assert.deepEqual(await events(protocol, complete), rejected);
  assert.deepEqual(await events(protocol, [observed, '[DONE]'], { geminiCompatibility: true }), rejected);
});

test('Responses uses the completed items once and preserves encrypted reasoning in the next request', async () => {
  const protocol = 'open_ai_responses';
  const call = { type: 'function_call', id: 'fc_a', call_id: 'call_a', name: 'dataset_put', arguments: '{"qty":12}' };
  const reasoning = { type: 'reasoning', id: 'rs_a', summary: [], encrypted_content: 'synthetic' };
  const delta = { type: 'response.function_call_arguments.delta', sequence_number: 2, item_id: call.id, delta: '{"qty":' };
  const result = await events(protocol, [
    { type: 'response.output_item.added', sequence_number: 1, item: { ...call, arguments: '' } },
    delta, delta,
    { type: 'response.completed', sequence_number: 3, response: { output: [reasoning, call] } },
  ]);
  assert.deepEqual(result[0].calls, [{ id: 'call_a', name: 'dataset_put', arguments: { qty: 12 } }]);
  const request = continuation(protocol, result[0]);
  assert.deepEqual(request.include, ['reasoning.encrypted_content']);
  assert.deepEqual(request.input, [user, reasoning, call, { type: 'function_call_output', call_id: 'call_a', output: '{"revision":1}' }]);
});

test('Anthropic assembles JSON fragments and preserves the signed thinking block', async () => {
  const protocol = 'anthropic_messages';
  const frame = (type, index, body) => ({ type, index, ...body });
  const start = (index, content_block) => frame('content_block_start', index, { content_block });
  const delta = (index, value) => frame('content_block_delta', index, { delta: value });
  const stop = index => frame('content_block_stop', index);
  const result = await events(protocol, [
    start(0, { type: 'thinking', thinking: '', signature: '' }),
    delta(0, { type: 'thinking_delta', thinking: '盘点' }),
    delta(0, { type: 'signature_delta', signature: 'synthetic' }), stop(0),
    start(1, { type: 'tool_use', id: 'tool_a', name: 'dataset_put', input: {} }),
    delta(1, { type: 'input_json_delta', partial_json: '{"qty":-' }),
    delta(1, { type: 'input_json_delta', partial_json: '2}' }), stop(1),
    { type: 'message_delta', delta: { stop_reason: 'tool_use' } }, { type: 'message_stop' },
  ]);
  const request = continuation(protocol, result.at(-1));
  assert.deepEqual(request.messages[1].content, [
    { type: 'thinking', thinking: '盘点', signature: 'synthetic' },
    { type: 'tool_use', id: 'tool_a', name: 'dataset_put', input: { qty: -2 } },
  ]);
  assert.deepEqual(request.messages[2].content, [{ type: 'tool_result', tool_use_id: 'tool_a', content: '{"revision":1}' }]);
});

test('Gemini preserves function parts, thought signatures and provider call IDs', async () => {
  const protocol = 'gemini_generate_content';
  const part = { thoughtSignature: 'synthetic', functionCall: { id: 'gem_a', name: 'dataset_put', args: { qty: 0 } } };
  const result = await events(protocol, [
    { candidates: [{ content: { parts: [part] } }] },
    { candidates: [{ finishReason: 'STOP' }] },
  ]);
  const request = continuation(protocol, result[0]);
  assert.deepEqual(request.contents.slice(1), [
    { role: 'model', parts: [part] },
    { role: 'user', parts: [{ functionResponse: { id: 'gem_a', name: 'dataset_put', response: { result: { revision: 1 } } } }] },
  ]);
});

test('unfinished, undeclared, invalid and duplicate calls cannot be proposed', async () => {
  const finish = chatDelta({}, 'tool_calls');
  const invalid = { type: 'error', code: 'INVALID_TOOL_ROUND' };
  const valid = chatTool(0, { name: 'dataset_put', arguments: '{"qty":1}' }, 'call_a');
  assert.deepEqual(await events('open_ai_chat', [valid]), [invalid]);
  assert.deepEqual(await events('open_ai_chat', [chatTool(0, { name: 'unknown', arguments: '{}' }, 'a'), finish]), [invalid]);
  assert.deepEqual(await events('open_ai_chat', [chatTool(0, { name: 'dataset_put', arguments: '{"qty":' }, 'a'), finish]), [invalid]);
  assert.deepEqual(await events('open_ai_chat', [valid, chatTool(1, { name: 'dataset_put', arguments: '{}' }, 'call_a'), finish]), [invalid]);
  assert.deepEqual(await events('open_ai_responses', [
    { type: 'response.output_item.added', item: { type: 'function_call' } },
    { type: 'response.incomplete', response: { incomplete_details: { reason: 'max_output_tokens' } } },
  ]), [invalid]);
  assert.deepEqual(await events('gemini_generate_content', [
    { candidates: [{ content: { parts: [{ functionCall: { name: 'dataset_put', args: { qty: 1 } } }] } }] },
  ]), [invalid]);
});
