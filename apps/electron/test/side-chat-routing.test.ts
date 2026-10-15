import { test } from 'node:test';
import assert from 'node:assert/strict';
import { validateClientCommand, assertCommandAllowedDuringTurn } from '../src/main/validation';
import { validateClientEvent } from '@lingxi/bridge-client';

const submit = (command: unknown) => ({ type: 'side_chat_submit', side_id: 'side-1', request_id: 'send-1', command_json: JSON.stringify(command) });

test('side commands retain admission correlation and work during a parent turn', () => {
  const create = validateClientCommand({ type: 'side_chat_create', request_id: 'create-1', initial_input: 'hello' });
  assert.doesNotThrow(() => assertCommandAllowedDuringTurn(create, true));
  assert.deepEqual(validateClientCommand(submit({ type: 'cancel' })), submit({ type: 'cancel' }));
  assert.deepEqual(validateClientCommand(submit({ type: 'approve_permission', request_id: 7, response: { type: 'allow_once' } })), submit({ type: 'approve_permission', request_id: 7, response: { type: 'allow_once' } }));
});

test('side control cannot start authentication, persistence, nested sides or forbidden slash', () => {
  for (const type of ['login', 'logout', 'new_session', 'resume_session', 'update_settings', 'task_message', 'side_chat_create']) {
    assert.throws(() => validateClientCommand(submit({ type })), /not allowed/);
  }
  assert.throws(() => validateClientCommand(submit({ type: 'send_prompt', text: '/side nested', images: [] })), /nested/);
  assert.throws(() => validateClientCommand(submit({ type: 'run_slash_command', raw: '/loop do work' })), /not allowed/);
  assert.throws(() => validateClientCommand({ ...submit({ type: 'cancel' }), request_id: undefined }), /request id/);
});

test('child event envelope preserves a valid side identity and rejects an empty side id', () => {
  const event = { type: 'side_chat_event', side_id: 'side-1', event_json: JSON.stringify({ type: 'error', message: 'side failure' }) };
  assert.deepEqual(validateClientEvent(event), event);
  assert.deepEqual(validateClientEvent({ type: 'side_chat_submitted', side_id: 'side-1', request_id: 'send-1' }), { type: 'side_chat_submitted', side_id: 'side-1', request_id: 'send-1' });
  assert.throws(() => validateClientEvent({ ...event, side_id: '' }), { message: 'invalid side chat id' });
  assert.throws(() => validateClientEvent({ ...event, side_id: 7 }), { message: 'invalid side chat id' });
});
