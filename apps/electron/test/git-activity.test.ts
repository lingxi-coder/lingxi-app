import { test } from 'node:test';
import assert from 'node:assert/strict';
import { GitActivityTracker } from '../src/main/git-activity';
import type { ClientEvent, TaskRowDto } from '@lingxi/bridge-client';
test('Git worktree guard tracks background workers independently of the parent turn', () => {
  const tracker = new GitActivityTracker();
  tracker.accept({type:'session_agent_updated',session_id:'s',agent:{agent_id:'a',name:'Agent',agent_type:'task',status:'running'}});
  assert.equal(tracker.active,true);
  tracker.accept({type:'turn_ended'} as any);
  assert.equal(tracker.active,true);
  tracker.accept({type:'session_agent_updated',session_id:'s',agent:{agent_id:'a',name:'Agent',agent_type:'task',status:'completed'}});
  assert.equal(tracker.active,false);
  tracker.accept({type:'coordinator_status',active_workers:1});
  assert.equal(tracker.active,true);
  tracker.accept({type:'coordinator_status',active_workers:0});
  assert.equal(tracker.active,false);
  tracker.accept({type:'session_agent_list',session_id:'s',agents:[{agent_id:'b',name:'B',agent_type:'task',status:'pending'}]});
  assert.equal(tracker.active,true);
  tracker.reset(); assert.equal(tracker.active,false);
});

function taskRow(id: string, type = 'local_bash', status: TaskRowDto['status']['type'] = 'running'): ClientEvent {
  return { type: 'task_row', task: { task_id: id, task_type: type, description: id, status: { type: status } } };
}

function lifecycle(task_id: string, subtype: string, fields: Record<string, unknown> = {}): ClientEvent {
  return { type: 'task_lifecycle', event_json: JSON.stringify({ type: 'system', subtype, task_id, ...fields }) };
}

function snapshot(tasks: ClientEvent[] = []): ClientEvent[] {
  return [
    { type: 'session_agent_list', session_id: 's', agents: [] },
    { type: 'coordinator_status', active_workers: 0 },
    ...tasks,
    { type: 'task_list_complete', request_id: 'desktop-runtime-snapshot', active_count: tasks.filter((event) =>
      event.type === 'task_row' && ['pending', 'running', 'paused'].includes(event.task.status.type)).length },
  ];
}

for (const type of ['local_bash', 'local_workflow', 'local_fusion', 'local_agent']) {
  test(`${type} guards survive foreground completion and release on terminal task status`, () => {
    const tracker = new GitActivityTracker();
    for (const status of ['pending', 'running', 'paused'] as const) {
      tracker.accept(taskRow('task', type, status));
      tracker.accept({ type: 'turn_ended' } as ClientEvent);
      assert.equal(tracker.active, true);
    }
    for (const status of ['completed', 'failed', 'cancelled'] as const) {
      tracker.accept(taskRow('task', type));
      tracker.accept({ type: 'task_status_changed', task_id: 'task', status: { type: status } });
      assert.equal(tracker.active, false, 'parked local agents report completed, just like terminal tasks');
    }
  });
}

test('SDK lifecycle receipts guard task starts and changes before any task-list refresh', () => {
  const tracker = new GitActivityTracker();
  tracker.accept(lifecycle('shell', 'task_started', { task_type: 'local_bash' }));
  assert.equal(tracker.active, true);
  tracker.accept(lifecycle('shell', 'task_updated', { patch: { description: 'still running' } }));
  assert.equal(tracker.active, true);
  tracker.accept(lifecycle('shell', 'task_updated', { patch: { status: 'completed' } }));
  assert.equal(tracker.active, false);
  tracker.accept(lifecycle('shell', 'task_updated', { patch: { status: 'running' } }));
  assert.equal(tracker.active, true);
  tracker.accept(lifecycle('shell', 'task_notification', { status: 'stopped' }));
  assert.equal(tracker.active, false);
  tracker.accept({ type: 'task_status_changed', task_id: 'fusion', status: { type: 'running' } });
  assert.equal(tracker.active, true, 'a status edge alone is authoritative without a cached row');
  tracker.accept(lifecycle('fusion', 'task_notification', { status: 'failed' }));
  assert.equal(tracker.active, false);
});

test('unrelated or malformed SDK lifecycle records do not release task ownership', () => {
  const tracker = new GitActivityTracker();
  tracker.accept(taskRow('shell'));
  for (const event_json of ['not JSON', 'null', '[]', '{"subtype":"task_updated","task_id":"shell","patch":{"status":"completed"}}',
    '{"type":"system","subtype":"task_updated","task_id":"shell","patch":{"status":"unknown"}}']) {
    tracker.accept({ type: 'task_lifecycle', event_json });
    assert.equal(tracker.active, true);
  }
});

test('ordinary filtered or failed task lists cannot clear missing live tasks', () => {
  const tracker = new GitActivityTracker();
  tracker.accept(taskRow('shell'));
  tracker.accept(taskRow('other', 'local_workflow', 'completed'));
  tracker.accept({ type: 'task_list_complete', request_id: 'filtered-list', active_count: 0 });
  tracker.accept({ type: 'session_agent_list', session_id: 's', agents: [] });
  tracker.accept({ type: 'coordinator_status', active_workers: 0 });
  assert.equal(tracker.active, true);
  for (const events of [snapshot().filter((event) => event.type !== 'task_list_complete'),
    [...snapshot().slice(0, -1), { type: 'task_list_complete', request_id: 'desktop-runtime-snapshot', active_count: 0, error: 'registry unavailable' } as ClientEvent]]) {
    assert.throws(() => tracker.replaceSnapshot(events), /incomplete/);
    assert.equal(tracker.active, true);
  }
  tracker.replaceSnapshot(snapshot());
  assert.equal(tracker.active, false);
});

test('authoritative task snapshots retain live tasks and independent coordinator ownership', () => {
  const tracker = new GitActivityTracker();
  tracker.replaceSnapshot(snapshot([taskRow('workflow', 'local_workflow')]));
  assert.equal(tracker.active, true);
  tracker.accept({ type: 'coordinator_status', active_workers: 1 });
  tracker.accept(lifecycle('workflow', 'task_notification', { status: 'completed' }));
  assert.equal(tracker.active, true);
  tracker.accept({ type: 'coordinator_status', active_workers: 0 });
  assert.equal(tracker.active, false);
  tracker.accept(taskRow('previous', 'local_workflow', 'paused'));
  tracker.accept({ type: 'workflow_resumed', previous_task_id: 'previous', run_id: 'new-run',
    task: { task_id: 'next', task_type: 'local_workflow', description: 'next run', status: { type: 'running' } } });
  tracker.accept({ type: 'task_status_changed', task_id: 'next', status: { type: 'completed' } });
  assert.equal(tracker.active, false, 'resuming transfers ownership away from the old paused run');
});
