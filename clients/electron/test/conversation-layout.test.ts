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
  const narrations = state.items.filter((item) => item.type === 'narration');
  assert.deepEqual(narrations.map((item) => item.role), ['user', 'assistant']);
  assert.deepEqual(narrations.map((item) => item.streamed), [undefined, undefined]);
});

test('optimistic user prompts retain inline image attachments for the messages view', () => {
  const image = { media_type: 'image/png', base64: 'iVBORw0KGgo=' };
  const item = appendUserPrompt(emptyConversation(), 'describe this', [image]).items[0];
  assert.deepEqual(item.type === 'narration' ? item.images : undefined, [{
    media_type: image.media_type,
    url: `data:${image.media_type};base64,${image.base64}`,
  }]);
});

test('resumed user prompts recover durable inline image projections', () => {
  const image = { media_type: 'image/jpeg', url: 'data:image/jpeg;base64,/9j/4AAQ' };
  const state = conversationFromMessages([{ role: 'user', blocks: [{ type: 'text', text: 'describe this' }], images: [image] }]);
  const item = state.items[0];
  assert.deepEqual(item.type === 'narration' ? item.images : undefined, [image]);
});
