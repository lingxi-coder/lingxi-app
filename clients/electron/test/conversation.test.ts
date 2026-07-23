/**
 * Unit tests for the live-conversation reducer (M10 A1 — C3).
 *
 * Pure logic only — no React, no `window` — so it runs under Node's built-in
 * test runner. Run from this package with the shared SDK's `tsx`:
 *
 *   node --import ../shared/node_modules/tsx --test test/conversation.test.ts
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import type { ClientEvent } from '@lingxi/bridge-client';
import {
  emptyConversation,
  reduceEvent,
  reduceEvents,
  appendUserPrompt,
  type ConversationState,
} from '../src/renderer/bridge/conversation';

type Narration = Extract<ConversationState['items'][number], { type: 'narration' }>;
type Agent = Extract<ConversationState['items'][number], { type: 'agent' }>;
type Meta = Extract<ConversationState['items'][number], { type: 'meta' }>;
type Thinking = Extract<ConversationState['items'][number], { type: 'thinking' }>;

const COST = {
  total_usd: 0.01,
  input_tokens: 10,
  output_tokens: 20,
  api_calls: 1,
  session_duration_secs: 5,
  formatted: '0m 5s · 30 tokens · $0.01',
};

test('empty conversation is not running and has no items', () => {
  const s = emptyConversation();
  assert.equal(s.running, false);
  assert.deepEqual(s.items, []);
  assert.equal(s.lastError, null);
});

test('turn_started flips running on; turn_ended flips it off and appends a meta row', () => {
  let s = emptyConversation();
  s = reduceEvent(s, { type: 'turn_started', turn_id: 1 });
  assert.equal(s.running, true);
  s = reduceEvent(s, { type: 'turn_ended', outcome: { type: 'end_turn' }, cost: COST });
  assert.equal(s.running, false);
  const meta = s.items.at(-1) as Meta;
  assert.equal(meta.type, 'meta');
  assert.equal(meta.dur, COST.formatted);
});

test('text_delta accumulates into a single open assistant narration line', () => {
  let s = emptyConversation();
  s = reduceEvent(s, { type: 'turn_started' });
  s = reduceEvent(s, { type: 'text_delta', text: 'Hel' });
  s = reduceEvent(s, { type: 'text_delta', text: 'lo' });
  s = reduceEvent(s, { type: 'text_delta', text: ' world' });
  const lines = s.items.filter((i) => i.type === 'narration') as Narration[];
  assert.equal(lines.length, 1);
  assert.equal(lines[0].text, 'Hello world');
});

test('message_complete closes the open line so the next delta starts a new one', () => {
  let s = emptyConversation();
  s = reduceEvent(s, { type: 'text_delta', text: 'first' });
  s = reduceEvent(s, { type: 'message_complete' });
  s = reduceEvent(s, { type: 'text_delta', text: 'second' });
  const lines = s.items.filter((i) => i.type === 'narration') as Narration[];
  assert.deepEqual(lines.map((l) => l.text), ['first', 'second']);
});

test('tool_use_started adds a running agent card; tool_use_result flips it to done', () => {
  let s = emptyConversation();
  s = reduceEvent(s, {
    type: 'tool_use_started',
    id: 'tu-1',
    tool: 'Read',
    input_json: '{"file_path":"src/main.rs"}',
  });
  let card = s.items.find((i) => i.type === 'agent') as Agent;
  assert.equal(card.state, 'running');
  assert.equal(card.title, 'Read');
  assert.equal(card.sub, 'src/main.rs');

  s = reduceEvent(s, {
    type: 'tool_use_result',
    id: 'tu-1',
    tool: 'Read',
    result_json: '"ok"',
    is_error: false,
  });
  card = s.items.find((i) => i.type === 'agent') as Agent;
  assert.equal(card.state, 'done');
});

test('a tool that interrupts streaming text reopens a fresh line afterwards', () => {
  let s = emptyConversation();
  s = reduceEvent(s, { type: 'text_delta', text: 'before' });
  s = reduceEvent(s, { type: 'tool_use_started', id: 'x', tool: 'Bash', input_json: '{"command":"ls"}' });
  s = reduceEvent(s, { type: 'text_delta', text: 'after' });
  const lines = s.items.filter((i) => i.type === 'narration') as Narration[];
  assert.deepEqual(lines.map((l) => l.text), ['before', 'after']);
  const card = s.items.find((i) => i.type === 'agent') as Agent;
  assert.equal(card.sub, 'ls');
});

test('error event records lastError, appends a strong line, and stops running', () => {
  let s = emptyConversation();
  s = reduceEvent(s, { type: 'turn_started' });
  s = reduceEvent(s, { type: 'error', kind: { type: 'server' }, message: 'boom' });
  assert.equal(s.running, false);
  assert.equal(s.lastError, 'boom');
  const last = s.items.at(-1) as Narration;
  assert.match(last.text, /boom/);
  assert.equal(last.strong, true);
});

test('system_notice remains non-terminal while surfacing its severity', () => {
  let s = reduceEvent(emptyConversation(), { type: 'turn_started' });
  s = reduceEvent(s, {
    type: 'system_notice',
    message: 'Conversation changes could not be saved.',
    is_error: true,
  });
  assert.equal(s.running, true);
  assert.equal(s.lastError, 'Conversation changes could not be saved.');
  assert.equal((s.items.at(-1) as Narration).strong, true);

  s = reduceEvent(s, {
    type: 'system_notice',
    message: 'Recovered persisted state.',
    is_error: false,
  });
  assert.equal(s.running, true);
  assert.equal((s.items.at(-1) as Narration).text, 'Recovered persisted state.');
});

test('failing tool_use_result surfaces an error line', () => {
  let s = emptyConversation();
  s = reduceEvent(s, { type: 'tool_use_started', id: 't', tool: 'Bash', input_json: '{"command":"x"}' });
  s = reduceEvent(s, { type: 'tool_use_result', id: 't', tool: 'Bash', result_json: '""', is_error: true });
  const card = s.items.find((i) => i.type === 'agent') as Agent;
  assert.equal(card.state, 'done');
  assert.equal(s.lastError, 'Bash failed');
});

test('appendUserPrompt echoes a strong narration line and ignores blank input', () => {
  let s = emptyConversation();
  s = appendUserPrompt(s, '   ');
  assert.equal(s.items.length, 0);
  s = appendUserPrompt(s, 'fix the bug');
  const line = s.items.at(-1) as Narration;
  assert.equal(line.text, 'fix the bug');
  assert.equal(line.strong, true);
});

test('reduceEvents folds a full turn end-to-end', () => {
  const events: ClientEvent[] = [
    { type: 'turn_started', turn_id: 7 },
    { type: 'text_delta', text: 'Looking at the code… ' },
    { type: 'tool_use_started', id: 'a', tool: 'Read', input_json: '{"file_path":"a.rs"}' },
    { type: 'tool_use_result', id: 'a', tool: 'Read', result_json: '"…"', is_error: false },
    { type: 'text_delta', text: 'Done.' },
    { type: 'message_complete' },
    { type: 'turn_ended', outcome: { type: 'end_turn' }, cost: COST },
  ];
  const s = reduceEvents(emptyConversation(), events);
  assert.equal(s.running, false);
  const kinds = s.items.map((i) => i.type);
  assert.deepEqual(kinds, ['narration', 'agent', 'narration', 'meta']);
});

test('thinking_delta accumulates into a single open, streaming thinking block', () => {
  let s = emptyConversation();
  s = reduceEvent(s, { type: 'turn_started' });
  s = reduceEvent(s, { type: 'thinking_delta', thinking: 'Let me ' });
  s = reduceEvent(s, { type: 'thinking_delta', thinking: 'consider ' });
  s = reduceEvent(s, { type: 'thinking_delta', thinking: 'the options.' });
  const blocks = s.items.filter((i) => i.type === 'thinking') as Thinking[];
  assert.equal(blocks.length, 1);
  assert.equal(blocks[0].text, 'Let me consider the options.');
  // Still streaming — not yet sealed.
  assert.notEqual(blocks[0].done, true);
});

test('thinking_delta is a distinct block from the assistant answer text', () => {
  let s = emptyConversation();
  s = reduceEvent(s, { type: 'thinking_delta', thinking: 'reasoning…' });
  s = reduceEvent(s, { type: 'text_delta', text: 'the answer' });
  const kinds = s.items.map((i) => i.type);
  assert.deepEqual(kinds, ['thinking', 'narration']);
  const block = s.items[0] as Thinking;
  // The arrival of answer text seals the reasoning block.
  assert.equal(block.done, true);
  const line = s.items[1] as Narration;
  assert.equal(line.text, 'the answer');
});

test('message_complete seals the open thinking block', () => {
  let s = emptyConversation();
  s = reduceEvent(s, { type: 'thinking_delta', thinking: 'pondering' });
  s = reduceEvent(s, { type: 'message_complete' });
  const block = s.items[0] as Thinking;
  assert.equal(block.done, true);
});

test('turn_ended seals an open thinking block (no answer text streamed)', () => {
  let s = emptyConversation();
  s = reduceEvent(s, { type: 'turn_started' });
  s = reduceEvent(s, { type: 'thinking_delta', thinking: 'quiet thought' });
  s = reduceEvent(s, { type: 'turn_ended', outcome: { type: 'end_turn' }, cost: COST });
  const block = s.items.find((i) => i.type === 'thinking') as Thinking;
  assert.equal(block.done, true);
});

test('usage_update captures the live token snapshot without emitting a scrollback item', () => {
  let s = emptyConversation();
  assert.equal(s.usage, null);
  s = reduceEvent(s, {
    type: 'usage_update',
    input_tokens: 1200,
    output_tokens: 340,
    cache_read_tokens: 800,
    cache_creation_tokens: 64,
  });
  assert.equal(s.items.length, 0);
  assert.deepEqual(s.usage, {
    inputTokens: 1200,
    outputTokens: 340,
    cacheReadTokens: 800,
    cacheCreationTokens: 64,
  });
  // A later update overwrites with the newest cumulative snapshot.
  s = reduceEvent(s, {
    type: 'usage_update',
    input_tokens: 1500,
    output_tokens: 900,
    cache_read_tokens: 800,
    cache_creation_tokens: 64,
  });
  assert.equal(s.usage?.outputTokens, 900);
});

test('session_resumed atomically replaces the transcript with lowered history', () => {
  let s = appendUserPrompt(emptyConversation(), 'stale optimistic prompt');
  s = reduceEvent(s, {
    type: 'session_resumed',
    session_id: 'abc',
    messages: [
      { role: 'user', blocks: [{ type: 'text', text: 'prior question' }] },
      {
        role: 'assistant',
        blocks: [
          { type: 'thinking', thinking: 'considering' },
          { type: 'tool_use', id: 'tool-1', tool: 'Read', input_json: '{"file_path":"src/lib.rs"}' },
          { type: 'tool_result', id: 'tool-1', tool: 'Read', result_json: '"contents"', is_error: false },
          { type: 'text', text: 'prior answer' },
        ],
      },
    ],
  });
  assert.equal(s.running, false);
  assert.equal(s.items.length, 4);
  assert.deepEqual(s.items.map((item) => item.type), ['narration', 'thinking', 'agent', 'narration']);
  const user = s.items[0] as Narration;
  assert.equal(user.text, 'prior question');
  assert.equal(user.strong, true);
  const tool = s.items[2] as Agent;
  assert.equal(tool.state, 'done');
  assert.equal(tool.detail, 'contents');
});

test('session_started and session_ended clear stale conversation state', () => {
  const populated = appendUserPrompt(emptyConversation(), 'old');
  assert.deepEqual(reduceEvent(populated, { type: 'session_started', session_id: 'new' }), emptyConversation());
  assert.deepEqual(reduceEvent(populated, { type: 'session_ended' }), emptyConversation());
});

test('tool result details redact common credential shapes', () => {
  let s = reduceEvent(emptyConversation(), {
    type: 'tool_use_started', id: 'secret', tool: 'Bash', input_json: '{"command":"curl -H Authorization:Bearer sk-ant-example123456789"}',
  });
  s = reduceEvent(s, {
    type: 'tool_use_result', id: 'secret', tool: 'Bash', result_json: '"token=super-secret-value"', is_error: false,
  });
  const tool = s.items[0] as Agent;
  assert.doesNotMatch(tool.sub ?? '', /sk-ant-example/);
  assert.doesNotMatch(tool.detail ?? '', /super-secret-value/);
});
