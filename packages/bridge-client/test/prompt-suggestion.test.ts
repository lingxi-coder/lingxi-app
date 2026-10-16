import assert from 'node:assert/strict';
import { test } from 'node:test';

import type { ClientCommand, ClientEvent } from '../src/protocol.js';
import { validateClientEvent } from '../src/validation.js';

test('prompt suggestion commands match the current Harness wire fields', () => {
  const commands: ClientCommand[] = [
    {
      type: 'update_prompt_suggestion_context',
      session_id: 'session-1',
      focus: 'composer',
      input_empty: false,
      pending_permission: false,
      elicitation_active: false,
    },
    {
      type: 'prompt_suggestion_feedback',
      session_id: 'session-1',
      uuid: 'suggestion-1',
      action: 'dismissed',
    },
    { type: 'record_prompt_suggestion_submission', text: 'Please review the diff.' },
  ];

  assert.deepEqual(commands.map(({ type }) => type), [
    'update_prompt_suggestion_context',
    'prompt_suggestion_feedback',
    'record_prompt_suggestion_submission',
  ]);
});

test('prompt suggestion events validate their exact current wire shape', () => {
  const events: ClientEvent[] = [
    {
      type: 'prompt_suggestion',
      suggestion: 'Run the focused test suite',
      uuid: '44444444-4444-4444-8444-444444444444',
      session_id: '33333333-3333-4333-8333-333333333333',
    },
    { type: 'prompt_suggestion_state', enabled: true, notice: null },
    { type: 'prompt_suggestion_state', enabled: false, notice: 'Suggestions are unavailable.' },
  ];

  for (const event of events) assert.deepEqual(validateClientEvent(event), event);
});

test('prompt suggestion event validation rejects malformed fields and extensions', () => {
  assert.throws(() => validateClientEvent({
    type: 'prompt_suggestion', suggestion: 'Run tests', uuid: 'suggestion-1', session_id: 'session-1', extra: true,
  }), /unsupported fields/);
  assert.throws(() => validateClientEvent({
    type: 'prompt_suggestion', suggestion: 7, uuid: 'suggestion-1', session_id: 'session-1',
  }), /prompt suggestion/);
  assert.throws(() => validateClientEvent({
    type: 'prompt_suggestion_state', enabled: true, notice: 7,
  }), /prompt suggestion notice/);
  assert.throws(() => validateClientEvent({
    type: 'prompt_suggestion_state', enabled: 'yes', notice: null,
  }), /prompt suggestion enabled/);
});
