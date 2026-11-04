import assert from 'node:assert/strict';
import test from 'node:test';

import type { ClientEvent } from '@lingxi/bridge-client';
import { followUpPrompt, taskPrompt } from '../src/main/bots/taskEnvelope';
import { appendUserPrompt, emptyConversation, reduceEvent, type ConversationState } from '../src/renderer/bridge/conversation';
import { crossSessionMessage } from '../src/renderer/components/messageOrigin';

const NOW = Date.UTC(2026, 9, 27, 12, 0, 0);
const SESSION = '11111111-2222-4333-8444-555555555555';
const envelope = { taskId: 'T'.repeat(22), from: 'Alice', goal: 'Summarise the report.\nUse the second sheet.', inputs: [], deadline: NOW + 20 * 60_000, attachTool: 'mcp__lingxi_bots__attach_artifact' };
const PROMPT = taskPrompt(envelope, NOW);

const assistant = (text: string) => ({ role: 'assistant' as const, blocks: [{ type: 'text' as const, text }] });
const cost = { total_usd: 0.1, input_tokens: 1, output_tokens: 1, api_calls: 1, session_duration_secs: 1, formatted: '$0.10' };

function rows(state: ConversationState): Array<[string, string]> {
  return state.items.flatMap((item) => (item.type === 'narration' ? [[item.role, item.text] as [string, string]] : []));
}

/** What the window does as a task's session is started and does its work: the host's prompt is shown as the engine's events arrive. */
function liveTask(): ConversationState {
  let state = emptyConversation();
  state = reduceEvent(state, { type: 'session_started', session_id: SESSION, mode: 'code' } as ClientEvent);
  state = appendUserPrompt(state, PROMPT);
  state = reduceEvent(state, { type: 'text_delta', text: 'Revenue ' } as ClientEvent);
  state = reduceEvent(state, { type: 'text_delta', text: 'grew.' } as ClientEvent);
  state = reduceEvent(state, { type: 'message_complete', stop_reason: 'end_turn', message: assistant('Revenue grew.') } as ClientEvent);
  return reduceEvent(state, { type: 'turn_ended', outcome: { type: 'end_turn' }, stop_reason: 'end_turn', cost } as ClientEvent);
}

test('the prompt a task began with is the first message of the chat, as the task runs, followed by the Bot\'s answer', () => {
  const state = liveTask();
  assert.deepEqual(rows(state), [['user', PROMPT], ['assistant', 'Revenue grew.']]);
  assert.equal(state.running, false);
  const first = state.items[0]!;
  assert.equal(first.type === 'narration' && first.role === 'user' ? first.origin : 'not a user row', undefined, 'it carries no caption naming a device or a scheduled run');
});

test('a chat opened after the task ended shows the same prompt first, as the transcript gives it back', () => {
  const state = reduceEvent(emptyConversation(), {
    type: 'session_resumed', session_id: SESSION,
    messages: [{ role: 'user', blocks: [{ type: 'text', text: PROMPT }] }, assistant('Revenue grew.')],
  } as ClientEvent);
  assert.deepEqual(rows(state), [['user', PROMPT], ['assistant', 'Revenue grew.']]);
  assert.deepEqual(rows(state), rows(liveTask()), 'live and replayed read alike');
});

test('a follow-up is shown after the answer it follows, and the answer to it after that', () => {
  let state = liveTask();
  const followUp = followUpPrompt({ taskId: envelope.taskId, from: 'Alice', deadline: NOW + 20 * 60_000, attachTool: envelope.attachTool }, 'And the totals?', NOW);
  state = appendUserPrompt(state, followUp);
  state = reduceEvent(state, { type: 'text_delta', text: 'The totals are 14.' } as ClientEvent);
  state = reduceEvent(state, { type: 'message_complete', stop_reason: 'end_turn', message: assistant('The totals are 14.') } as ClientEvent);
  assert.deepEqual(rows(state).map(([role]) => role), ['user', 'assistant', 'user', 'assistant']);
  assert.equal(rows(state)[2]![1], followUp);
});

test('the prompt is not taken for a message from another session, which the chat would relabel', () => {
  assert.equal(crossSessionMessage(PROMPT), null);
  assert.equal(crossSessionMessage(followUpPrompt({ taskId: envelope.taskId, from: 'Alice', deadline: NOW, attachTool: envelope.attachTool }, 'x', NOW)), null);
});

test('the prompt names the task, who it is from, and what is asked', () => {
  const [title] = rows(liveTask())[0]![1].split('\n');
  assert.equal(title, 'Task from Alice: Summarise the report. Use the second sheet.', 'the first line, which also titles the chat');
  assert.match(PROMPT, /<delegated-task from="Alice" task="T{22}"/);
  assert.match(PROMPT, /Use the second sheet\./);
});
