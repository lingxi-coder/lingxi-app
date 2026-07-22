import test from 'node:test';
import assert from 'node:assert/strict';

import { appendUserPrompt, emptyConversation, reduceEvent, conversationFromMessages } from '../src/renderer/bridge/conversation';

test('conversation items keep user and assistant roles for split message layout', () => {
  const user = appendUserPrompt(emptyConversation(), 'hello').items[0];
  assert.equal(user.type, 'narration');
  assert.equal(user.role, 'user');

  const assistant = reduceEvent(emptyConversation(), { type: 'text_delta', text: 'hi' }).items[0];
  assert.equal(assistant.type, 'narration');
  assert.equal(assistant.role, 'assistant');
});

test('resumed transcripts assign the correct side to each message', () => {
  const state = conversationFromMessages([
    { role: 'user', blocks: [{ type: 'text', text: 'question' }] },
    { role: 'assistant', blocks: [{ type: 'text', text: 'answer' }] },
  ]);
  assert.deepEqual(state.items.filter((item) => item.type === 'narration').map((item) => item.role), ['user', 'assistant']);
});
