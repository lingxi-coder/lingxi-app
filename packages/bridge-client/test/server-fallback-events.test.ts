import assert from 'node:assert/strict';
import { test } from 'node:test';

import type { ClientEvent } from '../src/protocol.js';
import { validateClientEvent } from '../src/validation.js';

const TST: Extract<ClientEvent, { type: 'tombstone' }> = {
  type: 'tombstone',
  display_only: true,
  message: {
    uuid: '8d7c3e14-1b4d-4ce2-a929-06c0a10bf7d1',
    type: 'assistant',
    timestamp: '2026-10-03T12:00:00.000Z',
    request_id: 'request-1',
    request_ref_json: '{"lane":"main"}',
    message: {
      id: 'provider-message-1',
      model: 'claude-sonnet-4',
      stop_reason: 'refusal',
      stop_details_json: '{"type":"refusal"}',
      usage_json: '{"input_tokens":2}',
      content_json: '[{"type":"text","text":"discarded"}]',
    },
    supersedes_uuids: ['1ff46d7c-1e86-41e8-a8ca-440f6b965865'],
  },
};

test('server-fallback event DTOs round-trip with their exact snake_case envelopes', () => {
  const events: ClientEvent[] = [
    { type: 'query_model_change', to_model: 'claude-sonnet-4' },
    { type: 'assistant_block_start', block_key: 41 },
    { type: 'assistant_block_identity', block_key: 41, message_uuid: TST.message.uuid },
    TST,
    {
      type: 'refusal_continuation',
      phase: 'begin',
      salvage_text: 'retained🙂',
      join: 'exact',
      replaces_uuids: [TST.message.uuid],
      display_salvage_text: true,
    },
    { type: 'user_transcript_row_identity', row_token: 'ui-row-1', uuid: TST.message.uuid },
    { type: 'assistant_transcript_row_uuids', message_id: 'response-1', uuids: [TST.message.uuid, null] },
  ];

  for (const event of events) assert.deepEqual(validateClientEvent(event), event);
});

test('server-fallback validators reject malformed identity, row, and continuation facts', () => {
  assert.throws(() => validateClientEvent({ type: 'assistant_block_start', block_key: -1 }), /assistant block key/);
  assert.throws(() => validateClientEvent({ type: 'assistant_block_identity', block_key: 1 }), /assistant row UUID/);
  assert.throws(() => validateClientEvent({ type: 'user_transcript_row_identity', row_token: '', uuid: 'u' }), /row token/);
  assert.throws(() => validateClientEvent({
    type: 'assistant_transcript_row_uuids', message_id: 'response-1', uuids: ['row-1', 9],
  }), /assistant transcript UUID/);
  assert.throws(() => validateClientEvent({ ...TST, display_only: 'yes' }), /display_only/);
  assert.throws(() => validateClientEvent({
    type: 'tombstone', display_only: true, message: { ...TST.message, parent_uuid: 'not-forwarded' },
  }), /unsupported fields/);
  assert.throws(() => validateClientEvent({
    type: 'refusal_continuation', phase: 'end', salvage_text: 'retained', join: 'exact',
    replaces_uuids: [], display_salvage_text: true,
  }), /phase/);
});
