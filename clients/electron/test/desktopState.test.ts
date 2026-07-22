import { test } from 'node:test';
import assert from 'node:assert/strict';

import type { ClientEvent } from '@lingxi/bridge-client';
import {
  beginTaskRefresh,
  emptyDesktopState,
  orderedTasks,
  reduceDesktopEvent,
  reduceDesktopEvents,
} from '../src/renderer/bridge/desktopState';

test('session and model events replace authoritative host state', () => {
  const sessions = [{
    uuid: 's1', title: 'First', modified_rfc3339: '2026-07-21T00:00:00Z', message_count: 3, path: '/tmp/s1.jsonl',
  }];
  const state = reduceDesktopEvents(emptyDesktopState(), [
    { type: 'session_list', sessions },
    { type: 'session_resumed', session_id: 's1', messages: [] },
    { type: 'model_list', models: ['m1', 'm2'], current: 'm1' },
    { type: 'model_changed', model: 'm2' },
  ]);
  assert.deepEqual(state.sessions, sessions);
  assert.equal(state.activeSessionId, 's1');
  assert.deepEqual(state.models, ['m1', 'm2']);
  assert.equal(state.currentModel, 'm2');
});

test('task rows, output and status updates remain correlated by id', () => {
  const events: ClientEvent[] = [
    {
      type: 'task_row',
      task: { task_id: 'b', task_type: 'agent', status: { type: 'running' }, description: 'Run B' },
    },
    {
      type: 'task_row',
      task: { task_id: 'a', task_type: 'agent', status: { type: 'pending' }, description: 'Run A' },
    },
    { type: 'task_output_chunk', task_id: 'b', content: 'line', total_lines: 1, truncated: false },
    { type: 'task_status_changed', task_id: 'b', status: { type: 'completed' } },
  ];
  const state = reduceDesktopEvents(emptyDesktopState(), events);
  assert.equal(state.tasks['b']?.status.type, 'completed');
  assert.equal(state.taskOutput['b']?.content, 'line');
  assert.deepEqual(orderedTasks(state).map((task) => task.task_id), ['a', 'b']);
});

test('unknown task status update is ignored without inventing mock rows', () => {
  const before = emptyDesktopState();
  const after = reduceDesktopEvent(before, {
    type: 'task_status_changed', task_id: 'missing', status: { type: 'failed' },
  });
  assert.equal(after, before);
});

test('task refresh replaces stale rows and outputs instead of merging forever', () => {
  const populated = reduceDesktopEvents(emptyDesktopState(), [
    {
      type: 'task_row',
      task: { task_id: 'stale', task_type: 'agent', status: { type: 'running' }, description: 'Old task' },
    },
    { type: 'task_output_chunk', task_id: 'stale', content: 'old output', total_lines: 1, truncated: false },
  ]);

  const cleared = beginTaskRefresh(populated);
  assert.deepEqual(cleared.tasks, {});
  assert.deepEqual(cleared.taskOutput, {});

  const refreshed = reduceDesktopEvent(cleared, {
    type: 'task_row',
    task: { task_id: 'fresh', task_type: 'agent', status: { type: 'pending' }, description: 'New task' },
  });
  assert.deepEqual(Object.keys(refreshed.tasks), ['fresh']);
  assert.equal(refreshed.taskOutput['stale'], undefined);
});
