import { test } from 'node:test';
import assert from 'node:assert/strict';

import type { ClientEvent, ModelDetailsDto } from '@lingxi/bridge-client';
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
    {
      type: 'model_list',
      models: ['openai/gpt-5.4', 'anthropic/claude-sonnet-5'],
      current: 'openai/gpt-5.4',
      details: [{ reference: 'openai/gpt-5.4', supports_fast_mode: true } as ModelDetailsDto],
    },
    { type: 'model_changed', model: 'anthropic/claude-sonnet-5' },
    { type: 'fast_mode_changed', enabled: true },
    { type: 'permission_mode_changed', mode: 'auto' },
  ]);
  assert.deepEqual(state.sessions, sessions);
  assert.equal(state.activeSessionId, 's1');
  assert.deepEqual(state.models, ['openai/gpt-5.4', 'anthropic/claude-sonnet-5']);
  assert.equal(state.modelDetails[0]?.supports_fast_mode, true);
  assert.equal(state.currentModel, 'anthropic/claude-sonnet-5');
  assert.equal(state.fastMode, true);
  assert.equal(state.permissionMode, 'auto');
});

test('a started session stays visible until the file-backed catalog catches up', () => {
  const started = reduceDesktopEvent(emptyDesktopState(), {
    type: 'session_started',
    session_id: 'fresh-session',
  });

  assert.equal(started.activeSessionId, 'fresh-session');
  assert.deepEqual(started.sessions.map((session) => session.uuid), ['fresh-session']);
  assert.equal(started.sessions[0]?.message_count, 0);
  assert.equal(started.sessions[0]?.mode, 'code');
  assert.equal(started.sessions[0]?.path, '');

  const beforeFirstTurnPersists = reduceDesktopEvent(started, {
    type: 'session_list',
    sessions: [],
  });
  assert.deepEqual(beforeFirstTurnPersists.sessions.map((session) => session.uuid), ['fresh-session']);

  const durable = {
    uuid: 'fresh-session',
    title: 'First prompt',
    modified_rfc3339: '2026-08-26T12:00:00Z',
    message_count: 2,
    path: '/tmp/fresh-session.jsonl',
  };
  const afterFirstTurnPersists = reduceDesktopEvent(beforeFirstTurnPersists, {
    type: 'session_list',
    sessions: [durable],
  });
  assert.deepEqual(afterFirstTurnPersists.sessions, [durable]);
});

test('session_started preserves the engine-provided mode when present', () => {
  const started = reduceDesktopEvent(emptyDesktopState(), {
    type: 'session_started',
    session_id: 'chat-session',
    mode: 'chat',
  });

  assert.equal(started.sessions[0]?.mode, 'chat');
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

test('slash command catalogs replace stale commands for initial and live updates', () => {
  const initial = reduceDesktopEvent(emptyDesktopState(), {
    type: 'slash_command_catalog',
    commands: [{ name: 'model', description: 'Switch model', source: 'builtin' }],
  });
  assert.deepEqual(initial.slashCommands.map((command) => command.name), ['model']);

  const changed = reduceDesktopEvent(initial, {
    type: 'commands_changed',
    commands: [{ name: 'review-pr', description: 'Review a pull request', source: 'plugin' }],
  });
  assert.deepEqual(changed.slashCommands.map((command) => command.name), ['review-pr']);
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

test('desktop state ignores Local App mobile-only events without crashing', () => {
  const before = reduceDesktopEvents(emptyDesktopState(), [
    {
      type: 'app_event',
      event: {
        type: 'plugin_inventory_changed',
        inventory: {
          pluginId: 'lingxi-local-app',
          displayName: 'LingXi Local App',
          source: 'builtin',
          version: '2.0.0-dev',
          bundleSha256: 'a'.repeat(64),
          state: 'loaded',
          manifestDefaultEnabled: true,
          counts: { skills: 27, agents: 1, workflows: 6, templates: 4 },
        },
      },
    },
    {
      type: 'app_event',
      event: {
        type: 'verification_summary_changed',
        app_id: 'habits-1a2b',
        publication_state: 'published_unverified',
        mcp_verification: { status: 'passed', summary: 'MCP verification passed.' },
        ui_verification: { status: 'unavailable', summary: 'UI runner unavailable.', code: 'verification_unavailable' },
      },
    },
  ] as ClientEvent[]);

  assert.deepEqual(before, emptyDesktopState());
});
