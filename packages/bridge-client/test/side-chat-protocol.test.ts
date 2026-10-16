import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';

import type { ClientCommand, ClientEvent } from '../src/protocol.js';
import { validateClientEvent } from '../src/validation.js';

const fixture = JSON.parse(readFileSync(
  join(import.meta.dirname, 'fixtures', 'side-chat-protocol.json'),
  'utf8',
)) as { commands: ClientCommand[]; events: ClientEvent[] };

test('side-chat command fixtures preserve UTF-16 initial input and nested command JSON', () => {
  assert.deepEqual(fixture.commands.map(({ type }) => type), [
    'side_chat_create',
    'side_chat_list',
    'side_chat_get',
    'side_chat_end',
    'side_chat_submit',
  ]);

  const create = fixture.commands[0];
  assert.equal(create.type, 'side_chat_create');
  if (create.type === 'side_chat_create') {
    assert.equal(create.initial_input?.length, 1);
    assert.equal(create.initial_input?.charCodeAt(0), 0xd800);
  }

  const submit = fixture.commands.at(-1);
  assert.equal(submit?.type, 'side_chat_submit');
  if (submit?.type === 'side_chat_submit') {
    assert.equal(typeof submit.command_json, 'string');
    const nested = JSON.parse(submit.command_json) as Extract<ClientCommand, { type: 'send_prompt' }>;
    assert.equal(nested.type, 'send_prompt');
    assert.equal(nested.text.length, 1);
    assert.equal(nested.text.charCodeAt(0), 0xdc00);
  }
});

test('all side-chat event fixtures pass the strict bridge event validator', () => {
  assert.deepEqual(fixture.events.map(({ type }) => type), [
    'side_chat_created',
    'side_chat_list',
    'side_chat_snapshot',
    'side_chat_event',
    'side_chat_permission',
    'side_chat_submitted',
    'side_chat_ended',
    'side_chat_error',
  ]);
  for (const event of fixture.events) assert.deepEqual(validateClientEvent(event), event);

  assert.throws(() => validateClientEvent({
    type: 'side_chat_list',
    side_ids: [17],
  }));
  assert.throws(() => validateClientEvent({
    type: 'side_chat_created',
    request_id: 'create-1',
    side_id: 'side-1',
    unexpected: true,
  }));
});
