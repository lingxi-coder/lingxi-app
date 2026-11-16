import assert from 'node:assert/strict';
import { test } from 'node:test';
import React, { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { emptyConversation, reduceEvent } from '../src/renderer/bridge/conversation';
import { transcriptRows } from '../src/renderer/components/transcriptRows';
import { transcriptActivities, isTranscriptActivity } from '../src/renderer/components/transcriptActivities';
import { TranscriptActivity } from '../src/renderer/components/TranscriptActivity';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import type { ToolRunItem } from '../src/renderer/model/runItem';
Object.assign(globalThis, { React });

const tool = (id: string, verb = 'read'): ToolRunItem => ({ type: 'tool', id, tool: verb === 'read' ? 'Read' : 'Bash', status: 'running',
  view: { verb, label: 'Read', title: id, primary: id } } as ToolRunItem);
test('thinking, the first tool, mixed tools and terminal reasoning keep one activity identity', () => {
  let state = { ...emptyConversation(), running: true };
  const events = [
    { type: 'thinking_delta', thinking: 'Check layout' },
    { type: 'tool_use_started', id: 'read', tool: 'Read', input_json: '{}', header: tool('read').view },
    { type: 'tool_use_result', id: 'read', tool: 'Read', result_json: '{}', is_error: false },
    { type: 'thinking_delta', thinking: 'Check another operation' },
    { type: 'tool_use_started', id: 'bash', tool: 'Bash', input_json: '{}', header: tool('bash', 'shell').view },
    { type: 'turn_ended', outcome: { type: 'completed' } },
  ] as Parameters<typeof reduceEvent>[1][];
  for (const event of events) {
    state = reduceEvent(state, event);
    const rows = transcriptActivities(transcriptRows(state.items, state.running, false, true), state.running);
    assert.equal(rows.length, 1);
    assert.equal(rows[0]!.id, 'activity:after:start');
    assert.ok(isTranscriptActivity(rows[0]!));
  }
  assert.equal(transcriptActivities(transcriptRows(state.items, false, false, true), false)[0]!.type, 'tool-group');
});
test('prose and standalone deliverables keep their chronological boundaries', () => {
  const rows = transcriptActivities([
    { type: 'tool-group', id: 'one', tools: [tool('one')] },
    { type: 'narration', id: 'prose', role: 'assistant', text: 'A boundary' },
    { type: 'tool-group', id: 'two', tools: [tool('two')] },
    { type: 'tool-group', id: 'question', tools: [{ ...tool('q'), tool: 'AskUserQuestion' }] },
  ], true);
  assert.deepEqual(rows.map(row => row.id), ['activity:after:start', 'prose', 'activity:after:prose', 'question']);
});
test('one closed activity header owns thinking and calls without an extra status row', () => {
  const [activity] = transcriptActivities([
    { type: 'thinking', id: 'thought', text: 'Inspect spacing', streamed: true },
    { type: 'tool-group', id: 'read', tools: [tool('read')] },
  ], true);
  assert.ok(activity && isTranscriptActivity(activity));
  const render = (open: boolean) => renderToStaticMarkup(createElement(Theme.Provider, { value: tokens(false) }, createElement(TranscriptActivity, {
    activity, open, toolOpen: () => false, onSetOpen: () => {},
  })));
  assert.equal((render(false).match(/<button/g) ?? []).length, 1);
  assert.doesNotMatch(render(false), /data-thinking-slot|transcript-thinking|transcript-tool-row/);
  assert.match(render(true), /Inspect spacing/);
  assert.match(render(true), /transcript-tool-row/);
});
