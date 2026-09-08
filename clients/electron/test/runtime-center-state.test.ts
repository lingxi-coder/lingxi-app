import { mock, test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import type { ClientEvent, MessageDto, PlanTaskDto } from '@lingxi/bridge-client';

import {
  shouldPollOverviewTasks,
  startPollingWhileActive,
} from '../src/renderer/components/RuntimeCenter';
import {
  addRuntimeResources,
  closeRuntimeCenterItem,
  commitRuntimeResources,
  emptyRuntimeCenterState,
  openRuntimeCenterItem,
  planRuntimeItemId,
  promptRuntimeResources,
  reduceRuntimeCenterEvent,
  resetRuntimeCenterConnection,
  resourcesFromRestoredMessages,
  rollbackRuntimeResources,
} from '../src/renderer/bridge/runtimeCenterState';

function textMessage(role: string, text: string): MessageDto {
  return { role, blocks: [{ type: 'text', text }], images: [] };
}

test('runtime inspector reuses tabs and selects an adjacent fallback on close', () => {
  const task = { kind: 'task' as const, id: 'task-1' };
  const agent = { kind: 'agent' as const, id: 'agent:11111111-2222-4333-8444-555555555555' };
  let state = openRuntimeCenterItem(emptyRuntimeCenterState(), task);
  state = openRuntimeCenterItem(state, agent);
  state = openRuntimeCenterItem(state, task);
  assert.deepEqual(state.tabs, [task, agent]);
  assert.deepEqual(state.activeItem, task);

  state = closeRuntimeCenterItem(state, task);
  assert.deepEqual(state.tabs, [agent]);
  assert.deepEqual(state.activeItem, agent);
  assert.equal(state.inspectorOpen, true);
});

test('agent transcript snapshots retain a racing live tail and reject stale revisions', () => {
  const sessionId = 'session-a';
  const agentId = 'agent:11111111-2222-4333-8444-555555555555';
  const first = textMessage('user', 'prompt');
  const second = textMessage('assistant', 'answer');
  const tail = textMessage('assistant', 'live tail');
  let state = emptyRuntimeCenterState();
  state = reduceRuntimeCenterEvent(state, {
    type: 'session_agent_message', session_id: sessionId, agent_id: agentId,
    message_index: 2, message: tail,
  } satisfies ClientEvent, sessionId);
  state = reduceRuntimeCenterEvent(state, {
    type: 'session_agent_transcript', session_id: sessionId, agent_id: agentId,
    messages: [first, second], next_message_index: 2, revision: 4,
  } satisfies ClientEvent, sessionId);
  assert.deepEqual(state.transcripts[agentId]?.messages, [first, second, tail]);

  state = reduceRuntimeCenterEvent(state, {
    type: 'session_agent_transcript', session_id: sessionId, agent_id: agentId,
    messages: [first], next_message_index: 1, revision: 3,
  } satisfies ClientEvent, sessionId);
  assert.deepEqual(state.transcripts[agentId]?.messages, [first, second, tail]);

  const foreign = reduceRuntimeCenterEvent(state, {
    type: 'session_agent_message', session_id: 'session-b', agent_id: agentId,
    message_index: 3, message: textMessage('assistant', 'foreign'),
  } satisfies ClientEvent, sessionId);
  assert.equal(foreign, state);
});

test('optimistic resources rollback only the failed send and never expose base64 in ids', () => {
  const image = { media_type: 'image/png', base64: 'c2Vuc2l0aXZlLWJ5dGVz' };
  let state = emptyRuntimeCenterState();
  const first = promptRuntimeResources('session-a', 'send-1', [image], ['diagram.png'], ['src/app.ts']);
  assert.ok(first.every((resource) => !resource.id.includes(image.base64)));
  state = addRuntimeResources(state, first);
  state = addRuntimeResources(
    state,
    promptRuntimeResources('session-a', 'send-2', [], [], ['src/app.ts']),
  );

  state = rollbackRuntimeResources(state, 'send-1');
  assert.deepEqual(state.resources.map((resource) => resource.id), ['file:src/app.ts']);
  assert.deepEqual(state.resources[0]?.pendingSendTokens, ['send-2']);

  state = commitRuntimeResources(state, 'send-2');
  assert.equal(state.resources.length, 1);
  assert.equal(state.resources[0]?.pendingSendTokens, undefined);
  assert.equal(rollbackRuntimeResources(state, 'send-2'), state);
});

test('a successful overlapping send keeps the other owner pending without risking rollback', () => {
  let state = emptyRuntimeCenterState();
  state = addRuntimeResources(
    state,
    promptRuntimeResources('session-a', 'send-1', [], [], ['src/shared.ts']),
  );
  state = addRuntimeResources(
    state,
    promptRuntimeResources('session-a', 'send-2', [], [], ['src/shared.ts']),
  );
  state = commitRuntimeResources(state, 'send-1');
  assert.deepEqual(state.resources[0]?.pendingSendTokens, ['send-2']);
  assert.equal(state.resources[0]?.confirmed, true);

  state = rollbackRuntimeResources(state, 'send-2');
  assert.equal(state.resources.length, 1);
  assert.equal(state.resources[0]?.pendingSendTokens, undefined);
  assert.equal(state.resources[0]?.confirmed, true);
});

test('restored messages rebuild images and the serialized leading file mentions', () => {
  const messages: MessageDto[] = [{
    role: 'user',
    blocks: [{ type: 'text', text: '@src/app.ts @"My Files/read me.md"\n\nreview these' }],
    images: [{ media_type: 'image/png', url: 'data:image/png;base64,AAAA' }],
  }];
  const resources = resourcesFromRestoredMessages('session-a', messages);
  assert.deepEqual(resources.map((resource) => [resource.kind, resource.name]), [
    ['image', 'Attached image 1'],
    ['file', 'app.ts'],
    ['file', 'read me.md'],
  ]);
  assert.ok(resources.every((resource) => !resource.id.includes('AAAA')));
});

test('legacy plan ids survive unrelated insertion and explicit ids stay namespaced', () => {
  const task: PlanTaskDto = { subject: 'Implement inspector', state: 'in_progress' };
  const original = [task];
  const inserted: PlanTaskDto[] = [
    { subject: 'Audit bridge', state: 'completed' },
    task,
  ];
  assert.equal(planRuntimeItemId(task, 0, original), planRuntimeItemId(task, 1, inserted));
  assert.equal(
    planRuntimeItemId({ id: '42', subject: 'Ship', state: 'pending' }, 0, []),
    'id:42',
  );
});

// ---------------------------------------------------------------------------
// [Finding 22] `RuntimeCenterInspector`'s stage-poll effect
// (`startPollingWhileActive(() => taskInFlightRef.current,
// bridge.refreshTasks)`) reads `task` (`bridge.desktop.tasks[active.id]`)
// in its body to decide whether to run at all, but its dependency array
// named only `[active?.kind, active?.id, bridge.refreshTasks]` -- nothing
// derived from `task`. If the effect ever ran while the row was still
// absent (e.g. a task tab selected during the one-round-trip window a
// non-preserve `task_list` refresh empties `bridge.desktop.tasks`), it
// permanently no-ops: no interval starts, and nothing re-runs the effect
// when the row lands, so `task.stage` freezes for the rest of the run
// while the sibling output-poll effect right above it (which DOES carry
// `task?.status.type`) keeps ticking. This reads the component source
// directly rather than mounting React (this package ships neither jsdom
// nor react-test-renderer) -- the same technique
// `runtime-center-poll.test.ts`'s storm guard uses for the sibling effect.
// ---------------------------------------------------------------------------

const runtimeCenterSource = readFileSync(
  join(import.meta.dirname, '../src/renderer/components/RuntimeCenter.tsx'),
  'utf8',
);

function functionBody(source: string, name: string): string {
  const signature = source.indexOf(`function ${name}(`);
  assert.ok(signature !== -1, `function ${name} must exist in RuntimeCenter.tsx`);
  const parenOpen = source.indexOf('(', signature);
  assert.ok(parenOpen !== -1, `function ${name} must have a parameter list`);
  let parenDepth = 0;
  let parenClose = -1;
  for (let index = parenOpen; index < source.length; index += 1) {
    if (source[index] === '(') parenDepth += 1;
    else if (source[index] === ')') {
      parenDepth -= 1;
      if (parenDepth === 0) { parenClose = index; break; }
    }
  }
  assert.ok(parenClose !== -1, `function ${name}'s parameter list never closes`);
  const open = source.indexOf('{', parenClose);
  assert.ok(open !== -1, `function ${name} must have a body`);
  let depth = 0;
  for (let index = open; index < source.length; index += 1) {
    if (source[index] === '{') depth += 1;
    else if (source[index] === '}') {
      depth -= 1;
      if (depth === 0) return source.slice(open, index + 1);
    }
  }
  throw new Error(`function ${name}'s body never closes its braces`);
}

function effectEntries(source: string): { body: string; deps: string }[] {
  const entries: { body: string; deps: string }[] = [];
  for (let index = source.indexOf('useEffect('); index !== -1; index = source.indexOf('useEffect(', index + 1)) {
    const open = source.indexOf('{', index);
    if (open === -1) continue;
    let depth = 0;
    let bodyEnd = -1;
    for (let cursor = open; cursor < source.length; cursor += 1) {
      if (source[cursor] === '{') depth += 1;
      else if (source[cursor] === '}') {
        depth -= 1;
        if (depth === 0) { bodyEnd = cursor; break; }
      }
    }
    if (bodyEnd === -1) continue;
    const body = source.slice(open, bodyEnd + 1);
    const bracketOpen = source.indexOf('[', bodyEnd);
    if (bracketOpen === -1) continue;
    let bracketDepth = 0;
    let bracketClose = -1;
    for (let cursor = bracketOpen; cursor < source.length; cursor += 1) {
      if (source[cursor] === '[') bracketDepth += 1;
      else if (source[cursor] === ']') {
        bracketDepth -= 1;
        if (bracketDepth === 0) { bracketClose = cursor; break; }
      }
    }
    if (bracketClose === -1) continue;
    entries.push({ body, deps: source.slice(bracketOpen, bracketClose + 1) });
  }
  return entries;
}

test('the effect scanner can actually find the stage-poll effect before asserting on its deps', () => {
  const inspectorBody = functionBody(runtimeCenterSource, 'RuntimeCenterInspector');
  const stagePoll = effectEntries(inspectorBody).filter((entry) =>
    entry.body.includes('startPollingWhileActive(() => taskInFlightRef.current, bridge.refreshTasks)'));
  assert.equal(
    stagePoll.length, 1,
    'if the scanner cannot find the stage-poll effect, the assertion below proves nothing',
  );
});

test('RuntimeCenterInspector\'s stage-poll effect deps include task presence so a row that lands after mount is not missed forever', () => {
  const inspectorBody = functionBody(runtimeCenterSource, 'RuntimeCenterInspector');
  const stagePoll = effectEntries(inspectorBody).find((entry) =>
    entry.body.includes('startPollingWhileActive(() => taskInFlightRef.current, bridge.refreshTasks)'));
  assert.ok(stagePoll, 'RuntimeCenterInspector must contain the stage-poll effect');
  const hasTaskPresenceDep = /(^|[^\w.])!!task([^\w]|$)/.test(stagePoll!.deps)
    || /(^|[^\w.])task\?\.task_id([^\w]|$)/.test(stagePoll!.deps);
  assert.ok(
    hasTaskPresenceDep,
    `the stage-poll effect's dependency array (${stagePoll!.deps}) must include a task-presence `
    + 'dependency (e.g. `!!task`) -- the effect body reads `task` (bridge.desktop.tasks[active.id]) '
    + 'to decide whether to run, so if it first runs while the row is absent it can never recover '
    + 'when the row arrives, leaving task.stage frozen for the rest of the run',
  );
});

// ---------------------------------------------------------------------------
// [R5-14] The overview's task poll used to be gated on
// `hasInFlightTask(orderedTasks(bridge.desktop))` -- rows the client
// ALREADY has. Nothing pushes a new row (`emit_task_rows` answers only the
// `RefreshListings{Tasks}`/`TaskList` commands, and the single
// `TaskStatusChanged` push is dropped by the reducer for an unknown id), so
// with an empty or all-completed list the gate was permanently closed: open
// the Runtime Center first, then run `/fusion ...`, and the panel showed
// "No background tasks." for the entire run. The predicate now asks whether
// the PANEL is open, which is the only question a pull-only list can act
// on.
// ---------------------------------------------------------------------------

test('shouldPollOverviewTasks keeps polling while the overview is open even with no task in the map yet', () => {
  assert.equal(
    shouldPollOverviewTasks(true, []), true,
    'an open overview with an empty task map must still poll -- this is exactly the state a '
    + '/fusion task started after the panel opened lands in, and no push can fill it',
  );
  assert.equal(
    shouldPollOverviewTasks(true, [{ status: { type: 'completed' } }]), true,
    'an open overview whose only rows are terminal must still poll: the next task has not been '
    + 'pulled yet',
  );
  assert.equal(shouldPollOverviewTasks(true, [{ status: { type: 'running' } }]), true);
  assert.equal(
    shouldPollOverviewTasks(false, []), false,
    'a closed overview polls nothing',
  );
});

test('an open overview with an empty task map still issues task_list ticks (a /fusion task started after the panel opened is discovered)', () => {
  mock.timers.enable({ apis: ['setInterval'] });
  try {
    // Mirrors RuntimeCenterOverview: the ref is recomputed on every render
    // from the same predicate the component uses, and the interval reads it
    // fresh per tick.
    let tasks: { status: { type: string } }[] = [];
    const overviewOpen = true;
    const ref = { current: shouldPollOverviewTasks(overviewOpen, tasks) };
    let refreshes = 0;
    const refresh = async () => {
      refreshes += 1;
      // The engine spawned a `local_fusion` task after the panel opened; the
      // FIRST poll is what delivers its row.
      tasks = [{ status: { type: 'running' } }];
      ref.current = shouldPollOverviewTasks(overviewOpen, tasks);
    };
    const stop = startPollingWhileActive(() => ref.current, refresh);
    mock.timers.tick(1_500);
    assert.equal(
      refreshes, 1,
      'the first tick after opening the panel on an empty task map must issue a task_list; '
      + `got ${refreshes} -- the row of a task started after the panel opened can never arrive`,
    );
    mock.timers.tick(1_500);
    assert.equal(refreshes, 2, 'polling must continue once the row has landed');
    stop();
    mock.timers.tick(3_000);
    assert.equal(refreshes, 2, 'the interval must stop on cleanup');
  } finally {
    mock.timers.reset();
  }
});

test('RuntimeCenterOverview feeds its poll ref from shouldPollOverviewTasks, not from the task map alone', () => {
  const overviewBody = functionBody(runtimeCenterSource, 'RuntimeCenterOverview');
  assert.ok(
    overviewBody.includes('hasActiveTaskRef.current = shouldPollOverviewTasks(center.overviewOpen, tasks)'),
    'RuntimeCenterOverview must assign its poll ref from '
    + 'shouldPollOverviewTasks(center.overviewOpen, tasks) on every render; assigning it from '
    + 'hasInFlightTask(tasks)/hasActiveTask alone closes the poll for good whenever the task map '
    + 'is empty or all-terminal, which is precisely when a newly started task needs to be pulled',
  );
  assert.ok(
    !/hasActiveTaskRef\.current = hasActiveTask\b/.test(overviewBody),
    'the poll ref must not be fed the raw in-flight flag again',
  );
});

test('coordinator plan decisions update the displayed agent status without changing identity', () => {
  let state = emptyRuntimeCenterState();
  const worker = { agent_id: 'reviewer', name: 'reviewer', agent_type: 'general-purpose', status: 'awaiting approval' };
  state = reduceRuntimeCenterEvent(state, { type: 'coordinator_worker', worker }, 'session-a');
  assert.equal(state.agents.reviewer.status, 'awaiting approval');
  state = reduceRuntimeCenterEvent(state, { type: 'session_agent_updated', session_id: 'session-a', agent: { ...worker, status: 'idle' } }, 'session-a');
  assert.equal(state.agents.reviewer.status, 'awaiting approval', 'idle transcript notification is not a plan decision');
  state = reduceRuntimeCenterEvent(state, { type: 'session_agent_list', session_id: 'session-a', agents: [{ ...worker, status: 'idle' }] }, 'session-a');
  assert.equal(state.agents.reviewer.status, 'awaiting approval', 'roster refresh is not a plan decision');
  for (const decision of ['approved', 'rejected']) {
    state = reduceRuntimeCenterEvent(state, { type: 'coordinator_worker', worker: { ...worker, status: 'idle' } }, 'session-a');
    assert.equal(state.agents.reviewer.status, 'idle', decision);
    assert.equal(state.agents.reviewer.name, 'reviewer', decision);
  }
});

test('live coordinator workers survive transcript refresh without resurrecting terminal rows', () => {
  let state = emptyRuntimeCenterState();
  const worker = { agent_id: 'pane', name: 'pane', agent_type: 'general-purpose', status: 'working' };
  state = reduceRuntimeCenterEvent(state, { type: 'coordinator_worker', worker }, 'session-a');
  assert.equal(state.agents.pane.status, 'running');
  state = reduceRuntimeCenterEvent(state, { type: 'session_agent_list', session_id: 'session-a', agents: [] }, 'session-a');
  assert.equal(state.agents.pane.status, 'running', 'pane without parent transcript stays visible');
  state = reduceRuntimeCenterEvent(state, { type: 'session_agent_list', session_id: 'session-a', agents: [{ ...worker, status: 'idle', model: 'test-model' }] }, 'session-a');
  assert.equal(state.agents.pane.status, 'running');
  assert.equal(state.agents.pane.model, 'test-model');
  state = reduceRuntimeCenterEvent(state, { type: 'coordinator_worker', worker: { ...worker, status: 'killed' } }, 'session-a');
  state = reduceRuntimeCenterEvent(state, { type: 'session_agent_updated', session_id: 'session-a', agent: { ...worker, status: 'running' } }, 'session-a');
  assert.equal(state.agents.pane.status, 'killed', 'late transcript cannot revive a stopped worker');
  state = reduceRuntimeCenterEvent(state, { type: 'session_agent_list', session_id: 'session-a', agents: [] }, 'session-a');
  assert.equal(state.agents.pane, undefined);
  state = reduceRuntimeCenterEvent(state, { type: 'session_ended' }, 'session-a');
  assert.deepEqual(state.coordinatorWorkers, {});
  state = reduceRuntimeCenterEvent(state, { type: 'session_agent_list', session_id: 'session-b', agents: [] }, 'session-b');
  assert.deepEqual(state.agents, {});
});

for (const reset of ['connection', 'resume'] as const) {
  test(`${reset} discards coordinator liveness without closing historical transcripts`, () => {
    const sessionId = 'session-a';
    const worker = { agent_id: 'pane', name: 'pane', agent_type: 'general-purpose', status: 'working' };
    let state = openRuntimeCenterItem(emptyRuntimeCenterState(), { kind: 'agent', id: worker.agent_id });
    state = reduceRuntimeCenterEvent(state, {
      type: 'session_agent_transcript', session_id: sessionId, agent_id: worker.agent_id,
      messages: [textMessage('assistant', 'saved answer')], next_message_index: 1, revision: 1,
    }, sessionId);
    state = reduceRuntimeCenterEvent(state, { type: 'coordinator_worker', worker }, sessionId);
    const { tabs, activeItem, transcripts } = state;
    const resume = { type: 'session_resumed', session_id: sessionId, mode: 'code', messages: [] } satisfies ClientEvent;
    assert.equal(reduceRuntimeCenterEvent(state, { ...resume, session_id: 'other' }, sessionId), state);
    state = reset === 'connection'
      ? resetRuntimeCenterConnection(state)
      : reduceRuntimeCenterEvent(state, resume, sessionId);
    assert.deepEqual(state.coordinatorWorkers, {});
    assert.deepEqual(state.agents, {});
    assert.equal(state.tabs, tabs);
    assert.equal(state.activeItem, activeItem);
    assert.equal(state.transcripts, transcripts);
    state = reduceRuntimeCenterEvent(state, {
      type: 'session_agent_list', session_id: sessionId, agents: [],
    }, sessionId);
    assert.deepEqual(state.agents, {}, 'empty restarted snapshot must not resurrect the old live pane');
    state = reduceRuntimeCenterEvent(state, {
      type: 'session_agent_list', session_id: sessionId, agents: [{ ...worker, status: 'completed' }],
    }, sessionId);
    assert.equal(state.agents.pane.status, 'completed', 'restored history must not inherit a pre-restart running status');
  });
}
