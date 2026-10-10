import assert from 'node:assert/strict';
import { test } from 'node:test';

import { validateClientEvent } from '../src/validation.js';

const message = { role: 'assistant', blocks: [{ type: 'text', text: 'answer' }] };
const apiErrorJson = '{ "type": "api_error", "error": { "message": "rate limited" } }';

test('session-agent transcript snapshots retain UUID-addressed rows and raw API error envelopes', () => {
  const event = validateClientEvent({
    type: 'session_agent_transcript',
    session_id: 'session-a',
    agent_id: 'agent:11111111-2222-4333-8444-555555555555',
    messages: [
      { message_index: 0, message_uuid: '11111111-1111-4111-8111-111111111111', message },
      {
        message_index: 3,
        message_uuid: '33333333-3333-4333-8333-333333333333',
        message,
        api_error_json: apiErrorJson,
      },
    ],
    next_message_index: 4,
    revision: 8,
  });
  assert.equal(event.type, 'session_agent_transcript');
  if (event.type !== 'session_agent_transcript') return;
  assert.equal(event.messages[1]?.message_index, 3);
  assert.equal(event.messages[1]?.message_uuid, '33333333-3333-4333-8333-333333333333');
  assert.equal(event.messages[1]?.api_error_json, apiErrorJson);
});

test('session-agent live rows and UUID tombstones use the strict current wire shape', () => {
  assert.deepEqual(validateClientEvent({
    type: 'session_agent_message',
    session_id: 'session-a',
    agent_id: 'agent:11111111-2222-4333-8444-555555555555',
    message_index: 7,
    message_uuid: '77777777-7777-4777-8777-777777777777',
    message,
    api_error_json: apiErrorJson,
  }), {
    type: 'session_agent_message',
    session_id: 'session-a',
    agent_id: 'agent:11111111-2222-4333-8444-555555555555',
    message_index: 7,
    message_uuid: '77777777-7777-4777-8777-777777777777',
    message,
    api_error_json: apiErrorJson,
  });
  assert.deepEqual(validateClientEvent({
    type: 'session_agent_tombstone',
    session_id: 'session-a',
    agent_id: 'agent:11111111-2222-4333-8444-555555555555',
    message_uuid: '77777777-7777-4777-8777-777777777777',
    display_only: true,
  }), {
    type: 'session_agent_tombstone',
    session_id: 'session-a',
    agent_id: 'agent:11111111-2222-4333-8444-555555555555',
    message_uuid: '77777777-7777-4777-8777-777777777777',
    display_only: true,
  });
  assert.throws(() => validateClientEvent({
    type: 'session_agent_message',
    session_id: 'session-a',
    agent_id: 'agent-a',
    message_index: 0,
    message,
  }), /message UUID/);
  assert.throws(() => validateClientEvent({
    type: 'session_agent_message',
    session_id: 'session-a',
    agent_id: 'agent-a',
    message_index: 0,
    message_uuid: 'uuid-a',
    message,
    api_error_json: '{broken',
  }), /API error JSON/);
});
