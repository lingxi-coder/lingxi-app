/**
 * Rendered-markup test for finding B7#11: `TaskRowDto` gained a `stage`
 * field (F005, `/fusion` progress) on the wire, but nothing in
 * `RuntimeCenter.tsx` read it — `TaskDetail` rendered only `status`,
 * `description` and the spool output, so a running `/fusion` task showed no
 * progress for the whole panel stage no matter what the engine sent.
 *
 * This asserts on what `TaskDetail` actually EMITS (via `react-dom/server`),
 * not on a helper it is merely supposed to call — a state-level assertion on
 * `runtimeCenterState.ts` could not have caught this, because the stage
 * value was already present on `bridge.desktop.tasks[id]`; it was the JSX
 * that never read it. Same shim as `transcript-render.test.ts`: the bare
 * `tsx` loader reads the root `tsconfig.json` (no `jsx` option) and emits
 * classic `React.createElement`, which resolves `React` as a global.
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as React from 'react';

(globalThis as { React?: typeof React }).React = React;

import { renderToStaticMarkup } from 'react-dom/server';

import type { TaskRowDto } from '@lingxi/bridge-client';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import { TaskDetail } from '../src/renderer/components/RuntimeCenter';
import type { UseBridge } from '../src/renderer/bridge/useBridge';

function render(node: React.ReactElement): string {
  return renderToStaticMarkup(
    React.createElement(Theme.Provider, { value: tokens(true) }, node),
  );
}

// `TaskDetail` only reads `bridge.desktop.taskOutput`; every other bridge
// member it touches is behind a defined `task`, which the tests below always
// supply.
const EMPTY_BRIDGE = { desktop: { taskOutput: {} } } as unknown as UseBridge;

const FUSION_TASK: TaskRowDto = {
  task_id: 'f00000001',
  task_type: 'local_fusion',
  status: { type: 'running' },
  description: '/fusion compare two approaches',
  stage: 'Running panels 2/3',
};

test('TaskDetail renders the /fusion progress stage for a running task', () => {
  const html = render(React.createElement(TaskDetail, { task: FUSION_TASK, bridge: EMPTY_BRIDGE }));
  assert.ok(
    html.includes('Running panels 2/3'),
    `expected the rendered task detail to include the stage label, got: ${html}`,
  );
});

test('TaskDetail renders nothing extra when a task has no stage', () => {
  const noStage: TaskRowDto = { ...FUSION_TASK, stage: undefined };
  const html = render(React.createElement(TaskDetail, { task: noStage, bridge: EMPTY_BRIDGE }));
  assert.ok(!html.includes('Running panels'), `expected no stage text, got: ${html}`);
});

import { RuntimeCenterOverview, RuntimeCenterInspector, usesSummaryOverlayLayout } from '../src/renderer/components/RuntimeCenter';
import { emptyRuntimeCenterState } from '../src/renderer/bridge/runtimeCenterState';
import { emptyDesktopState } from '../src/renderer/bridge/desktopState';

function runtimeBridge(): UseBridge {
  return {
    runtimeCenter: { ...emptyRuntimeCenterState(), overviewOpen: true },
    desktop: { ...emptyDesktopState(), tasks: { [FUSION_TASK.task_id]: FUSION_TASK } },
    conversation: { plan: [] },
  } as unknown as UseBridge;
}

test('overview exposes background task data and detail preserves the task stage', () => {
  const bridge = runtimeBridge();
  const html = render(React.createElement(RuntimeCenterOverview, { bridge }));
  assert.ok(html.includes('local_fusion') || html.includes('background task'));
  const active = { kind: 'task', id: FUSION_TASK.task_id } as const;
  bridge.runtimeCenter = { ...bridge.runtimeCenter, inspectorOpen: true, activeItem: active, tabs: [active] };
  assert.ok(render(React.createElement(RuntimeCenterInspector, { bridge })).includes('Running panels 2/3'));
});

test('closed overview and inspector do not render stale session content', () => {
  const bridge = runtimeBridge();
  bridge.runtimeCenter = { ...bridge.runtimeCenter, overviewOpen: false, inspectorOpen: false };
  assert.equal(render(React.createElement(RuntimeCenterOverview, { bridge })), '');
  assert.equal(render(React.createElement(RuntimeCenterInspector, { bridge })), '');
});

test('summary reserves a side rail only while the message region is wide enough', () => {
  assert.equal(usesSummaryOverlayLayout(1_039), true);
  assert.equal(usesSummaryOverlayLayout(1_040), false);

  const bridge = runtimeBridge();
  bridge.runtimeCenter = { ...bridge.runtimeCenter, inspectorOpen: true };
  assert.match(render(React.createElement(RuntimeCenterOverview, { bridge })), /data-inspector-open="true"/);
});

test('pinned summary is a nonmodal region with context actions and four ordered empty categories', () => {
  const bridge = runtimeBridge();
  bridge.desktop = emptyDesktopState();
  const html = render(React.createElement(RuntimeCenterOverview, { bridge }));
  assert.ok(html.includes('role="region"'));
  assert.ok(html.includes('aria-label="Pinned summary"'));
  assert.ok(!html.includes('role="dialog"'));
  const headings = [...html.matchAll(/<h2>(.*?)<\/h2>/g)].map((match) => match[1]);
  assert.deepEqual(headings, ['Context', 'Subagents', 'Todos', 'Resources', 'Plan']);
  for (const empty of ['No subagents or background tasks.', 'No todos yet.', 'No input resources in this session.', 'No submitted plan yet.']) assert.ok(html.includes(empty));
});

test('summary previews only three resources while the resources detail lists every resource', () => {
  const bridge = runtimeBridge();
  bridge.runtimeCenter = { ...bridge.runtimeCenter, resources: Array.from({ length: 5 }, (_, i) => ({ id: `resource-${i}`, kind: 'file', name: `example-${i}.ts` })) };
  const summary = render(React.createElement(RuntimeCenterOverview, { bridge }));
  assert.ok(summary.includes('example-2.ts'));
  assert.ok(!summary.includes('example-3.ts'));
  assert.ok(summary.includes('View all'));
  const active = { kind: 'section', id: 'resources' } as const;
  bridge.runtimeCenter = { ...bridge.runtimeCenter, inspectorOpen: true, activeItem: active, tabs: [active] };
  const details = render(React.createElement(RuntimeCenterInspector, { bridge }));
  assert.ok(details.includes('example-4.ts'));
});

test('Todos checklist and submitted Plan body render as distinct detail content', () => {
  const bridge = runtimeBridge();
  bridge.runtimeCenter = { ...bridge.runtimeCenter, plan: [{ subject: 'Implement integration', state: 'in_progress' }], submittedPlan: { id: 'exit-plan-1', content: '# Migration design\n\nPreserve **compatibility**.', status: 'approved' } };
  const summary = render(React.createElement(RuntimeCenterOverview, { bridge }));
  assert.ok(summary.includes('0 of 1 done'));
  assert.ok(summary.includes('Implement integration'));
  assert.ok(summary.includes('Migration design'));
  const planTab = { kind: 'plan-document', id: 'exit-plan-1' } as const;
  bridge.runtimeCenter = { ...bridge.runtimeCenter, inspectorOpen: true, activeItem: planTab, tabs: [planTab] };
  const document = render(React.createElement(RuntimeCenterInspector, { bridge }));
  assert.ok(document.includes('Approved'));
  assert.ok(document.includes('compatibility</span>'));
  assert.ok(!document.includes('**compatibility**'));
  assert.ok(!document.includes('Implement integration'));
  const todoTab = { kind: 'section', id: 'todos' } as const;
  bridge.runtimeCenter = { ...bridge.runtimeCenter, activeItem: todoTab, tabs: [todoTab] };
  const todos = render(React.createElement(RuntimeCenterInspector, { bridge }));
  assert.ok(todos.includes('Implement integration'));
  assert.ok(!todos.includes('Migration design'));
});

test('an open inspector with no tabs shows four content entry points', () => {
  const bridge = runtimeBridge();
  bridge.runtimeCenter = { ...bridge.runtimeCenter, inspectorOpen: true, activeItem: null, tabs: [] };
  const html = render(React.createElement(RuntimeCenterInspector, { bridge }));
  for (const label of ['Subagents', 'Todos', 'Resources', 'Plan']) assert.ok(html.includes(label));
  assert.ok(html.includes('Hide right panel'));
  assert.ok(html.includes('Open pinned summary'));
});

test('Subagents detail identifies background tasks and preserves their stage', () => {
  const bridge = runtimeBridge();
  const active = { kind: 'section', id: 'agents' } as const;
  bridge.runtimeCenter = { ...bridge.runtimeCenter, inspectorOpen: true, activeItem: active, tabs: [active] };
  const html = render(React.createElement(RuntimeCenterInspector, { bridge }));
  assert.ok(html.includes('Background task'));
  assert.ok(html.includes('/fusion compare two approaches'));
  assert.ok(html.includes('Running panels 2/3'));
});

test('each submitted plan tab keeps its own body when a newer plan arrives', () => {
  const bridge = runtimeBridge();
  const first = { id: 'first', content: '# First proposal\n\nOriginal scope.', status: 'rejected' as const };
  const latest = { id: 'latest', content: '# Revised proposal\n\nNew scope.', status: 'approved' as const };
  bridge.runtimeCenter = {
    ...bridge.runtimeCenter,
    submittedPlan: latest,
    submittedPlanState: { calls: [first, latest], resolutions: {} },
    inspectorOpen: true,
    tabs: [{ kind: 'plan-document', id: 'first' }, { kind: 'plan-document', id: 'latest' }],
    activeItem: { kind: 'plan-document', id: 'first' },
  };
  const html = render(React.createElement(RuntimeCenterInspector, { bridge }));
  assert.ok(html.includes('Original scope.'));
  assert.ok(html.includes('Rejected'));
  assert.ok(!html.includes('New scope.'));
  bridge.runtimeCenter = { ...bridge.runtimeCenter, activeItem: { kind: 'section', id: 'plan' } };
  const current = render(React.createElement(RuntimeCenterInspector, { bridge }));
  assert.ok(current.includes('New scope.'));
  assert.ok(!current.includes('Original scope.'));
});

test('teammate roster preserves idle, running, and terminal states in the inspector', () => {
  const bridge = runtimeBridge();
  bridge.desktop = emptyDesktopState();
  const active = { kind: 'section', id: 'agents' } as const;
  bridge.runtimeCenter = {
    ...bridge.runtimeCenter,
    inspectorOpen: true,
    activeItem: active,
    tabs: [active],
    agents: Object.fromEntries(['running', 'idle', 'completed', 'failed'].map((status) => [`agent-${status}`, {
      agent_id: `agent-${status}`,
      name: `reviewer-${status}`,
      agent_type: 'general-purpose',
      model: 'claude-sonnet-4-6',
      status,
      latest_activity: status === 'idle' ? 'Waiting for a message' : 'Review implementation',
    }])),
  };
  const html = render(React.createElement(RuntimeCenterInspector, { bridge }));
  // The word each status is SHOWN as, per claude-code's row renderer: only
  // `completed` is relabelled (`done`). `idle` stays `idle` — it is a real
  // teammate state there, arriving through `coordinator_worker`; what it is not
  // is a finished background agent's status, which is the borrow this pass
  // undid. This loop used to assert every status rendered verbatim, which is to
  // say it pinned the divergence.
  const shown: Record<string, string> = { running: 'running', idle: 'idle', completed: 'done', failed: 'failed' };
  for (const status of ['running', 'idle', 'completed', 'failed']) {
    assert.ok(html.includes(`reviewer-${status}`));
    assert.ok(html.includes(`>${shown[status]}</span>`), `status ${status} renders as ${shown[status]}: ${html}`);
  }
  assert.ok(html.includes('Waiting for a message'));
});

test('an idle teammate transcript does not show active thinking', () => {
  const bridge = runtimeBridge();
  const active = { kind: 'agent', id: 'reviewer-id' } as const;
  bridge.runtimeCenter = {
    ...bridge.runtimeCenter,
    inspectorOpen: true,
    activeItem: active,
    tabs: [active],
    agents: {
      'reviewer-id': { agent_id: 'reviewer-id', name: 'reviewer', agent_type: 'general-purpose', status: 'idle', latest_activity: 'Waiting for a message' },
    },
  };
  const html = render(React.createElement(RuntimeCenterInspector, { bridge }));
  assert.ok(html.includes('>idle</span>'));
  assert.ok(html.includes('Waiting for a message'));
  assert.ok(!html.includes('Thinking'));
});

test('plan approval state overrides running label and clears after either decision', () => {
  const task: TaskRowDto = { task_id: 'teammate-plan', task_type: 'in_process_teammate', status: { type: 'running' }, description: 'Review the API', awaiting_plan_approval: true };
  const pending = render(React.createElement(TaskDetail, { task, bridge: EMPTY_BRIDGE }));
  assert.ok(pending.includes('>awaiting approval</span>'));
  assert.ok(pending.includes('Review the API'));
  for (const decision of ['approved', 'rejected']) {
    const resolved = render(React.createElement(TaskDetail, { task: { ...task, awaiting_plan_approval: false }, bridge: EMPTY_BRIDGE }));
    assert.ok(!resolved.includes('awaiting approval'), decision);
    assert.ok(resolved.includes('>running</span>'), decision);
    assert.ok(resolved.includes('Review the API'), decision);
  }
  const bridge = runtimeBridge();
  bridge.desktop = { ...emptyDesktopState(), tasks: { [task.task_id]: task } };
  assert.ok(render(React.createElement(RuntimeCenterOverview, { bridge })).includes('1 awaiting approval'));
});

function renderAgentActivity(status: string, activity?: string, withTranscript = true): string {
  const bridge = runtimeBridge();
  const active = { kind: 'agent', id: 'reviewer-progress' } as const;
  bridge.runtimeCenter = {
    ...bridge.runtimeCenter, inspectorOpen: true, activeItem: active, tabs: [active],
    agents: {
      [active.id]: { agent_id: active.id, name: 'reviewer', agent_type: 'general-purpose', status, latest_activity: activity },
    },
    transcripts: withTranscript ? {
      [active.id]: {
        messages: [{ role: 'user', blocks: [{ type: 'text', text: 'Review these changes.' }], images: [] }],
        revision: 1, nextMessageIndex: 1, messageIndexes: {},
      },
    } : {},
  };
  return render(React.createElement(RuntimeCenterInspector, { bridge }));
}

test('subagent progress and retry activity replace generic thinking without ending the run', () => {
  for (const activity of ['3 tool uses · 1200 tokens', 'Retrying (attempt 2): model response stalled']) {
    const html = renderAgentActivity('running', activity);
    assert.ok(html.includes(activity));
    assert.ok(html.includes('>running</span>'));
    assert.ok(html.includes('Review these changes.'));
    assert.ok(!html.includes('Thinking'));
    assert.ok(html.includes('role="status"'));
  }
});

test('subagent with no meaningful activity retains the generic thinking fallback', () => {
  for (const activity of [undefined, '', '   ']) {
    assert.ok(renderAgentActivity('running', activity).includes('Thinking'));
  }
});

test('retry before the first transcript message does not display welcome or waiting copy', () => {
  const html = renderAgentActivity('running', 'Retrying (attempt 2): first response timeout', false);
  assert.ok(html.includes('Retrying (attempt 2): first response timeout'));
  assert.ok(!html.includes('Thinking'));
  assert.ok(!html.includes('Waiting for the agent'));
  assert.ok(!html.includes('Turn intent into working code'));
});

test('interrupted subagent shows its failure reason without an active thinking indicator', () => {
  const html = renderAgentActivity('failed', 'Interrupted when the engine stopped.');
  assert.ok(html.includes('>failed</span>'));
  assert.ok(html.includes('Interrupted when the engine stopped.'));
  assert.ok(!html.includes('Thinking'));
});

import { Stage } from '../src/renderer/components/Stage';

test('explicit agent activity suppresses only synthetic thinking, retaining live reasoning and tools', () => {
  const thinking = render(React.createElement(Stage, {
    running: true,
    pendingActivity: '3 tool uses · 1200 tokens',
    liveItems: [{ type: 'thinking', id: 'reasoning-1', text: 'private reasoning', streamed: true }],
  }));
  assert.ok(thinking.includes('Thinking'));
  assert.ok(!thinking.includes('private reasoning'));
  const tool = render(React.createElement(Stage, {
    running: true,
    pendingActivity: '3 tool uses · 1200 tokens',
    liveItems: [{ type: 'tool', id: 'tool-1', tool: 'Read', status: 'running', view: { verb: 'read', label: 'Read', primary: 'src/main.ts', title: 'Read(src/main.ts)' } }],
  }));
  assert.ok(tool.includes('src/main.ts'));
  assert.ok(tool.includes('running-sweep'));
  assert.ok(!tool.includes('Thinking'));
});

/**
 * Finished work reads as claude-code writes it.
 *
 * Its task row keeps the machine status (`completed`) and RENDERS a different
 * word: `done` for an agent, plus `, unread` while the completion has not been
 * surfaced to the model; a background shell says `error` / `stopped` instead of
 * `failed` / `cancelled`. The port showed the raw status, and — worse — the
 * engine invented a sixth status, `idle`, for a parked persistent agent, so ONE
 * finished agent appeared twice in this panel under two different words: `idle`
 * on its Subagents row and `completed` on its task row.
 *
 * Asserted on the rendered markup, because the bug was never in the state: the
 * status reached the client intact both times and the JSX printed it verbatim.
 */
test('finished agents and tasks render claude-code labels, and the summary still counts them', () => {
  const agents = {
    'agent:parked': {
      agent_id: 'agent:parked',
      name: 'audit-cli',
      agent_type: 'Explore',
      status: 'completed',
      latest_activity: 'Bash',
    },
  };
  const tasks = {
    a0000001: { task_id: 'a0000001', task_type: 'local_agent', status: { type: 'completed' }, description: 'Audit CLI parity', unread: true },
    a0000002: { task_id: 'a0000002', task_type: 'local_agent', status: { type: 'completed' }, description: 'Audit mcp parity' },
    b0000003: { task_id: 'b0000003', task_type: 'local_bash', status: { type: 'failed' }, description: 'grep flag' },
    b0000004: { task_id: 'b0000004', task_type: 'local_bash', status: { type: 'cancelled' }, description: 'stopped sweep' },
  } as unknown as Record<string, TaskRowDto>;
  const active = { kind: 'section', id: 'agents' } as const;
  const bridge = {
    runtimeCenter: { ...emptyRuntimeCenterState(), agents, inspectorOpen: true, activeItem: active, tabs: [active] },
    desktop: { ...emptyDesktopState(), tasks },
    conversation: { plan: [] },
  } as unknown as UseBridge;

  const html = render(React.createElement(RuntimeCenterInspector, { bridge }));
  assert.ok(html.includes('>done<'), `a finished agent reads "done": ${html}`);
  assert.ok(html.includes('done, unread'), `an unsurfaced completion keeps claude-code's ", unread": ${html}`);
  assert.ok(html.includes('>error<'), `a failed background shell reads "error": ${html}`);
  assert.ok(html.includes('>stopped<'), `a cancelled background shell reads "stopped": ${html}`);
  assert.ok(!html.includes('>idle<'), `"idle" is the footer group's word, never a row's: ${html}`);
  assert.ok(!html.includes('>completed<'), `the machine status is not what the user reads: ${html}`);

  // The counters must keep seeing the MACHINE status. Feed them the rendered
  // labels instead and the overview reports "0 done" for a panel of finished
  // work — a subset comparison that reports all clear.
  const overview = render(React.createElement(RuntimeCenterOverview, {
    bridge: { ...bridge, runtimeCenter: { ...bridge.runtimeCenter, overviewOpen: true } } as unknown as UseBridge,
  }));
  assert.ok(overview.includes('2 done · 2 failed'), `background-task summary counts raw statuses: ${overview}`);
  assert.ok(overview.includes('1 done'), `subagent summary counts raw statuses: ${overview}`);
});

test('summary owns context browsing and compaction controls', () => {
  const bridge = runtimeBridge();
  const html = render(React.createElement(RuntimeCenterOverview, { bridge }));
  assert.match(html, /aria-label="Open context summaries"/);
  assert.match(html, /aria-label="Compact conversation"/);
  assert.match(html, /Compact/);
});

test('background agents show task titles and avatars without duplicating linked agents', () => {
  const bridge = runtimeBridge();
  const active = { kind: 'section', id: 'agents' } as const;
  bridge.runtimeCenter = { ...bridge.runtimeCenter, inspectorOpen: true, activeItem: active, tabs: [active], agents: {
    linked: { agent_id: 'linked', name: 'Reviewer', agent_type: 'Explore', status: 'completed' },
    unrelated: { agent_id: 'unrelated', name: 'Other reviewer', agent_type: 'Explore', status: 'running' },
  } };
  bridge.desktop.tasks = {
    first: { task_id: 'first', task_type: 'local_agent', agent_id: 'linked', description: 'Audit build scripts', status: { type: 'completed' } },
    second: { task_id: 'second', task_type: 'local_agent', description: 'Review test coverage', status: { type: 'completed' } },
    empty: { task_id: 'empty', task_type: 'local_bash', description: '  ', status: { type: 'running' } },
  };
  const html = render(React.createElement(RuntimeCenterInspector, { bridge }));
  assert.ok(html.includes('>Audit build scripts</span>'));
  assert.ok(html.includes('>Review test coverage</span>'));
  assert.ok(html.includes('Background agent · Reviewer'));
  assert.ok(html.includes('Other reviewer'));
  assert.ok(!html.includes('>Reviewer</span>'));
  assert.ok(!html.includes('local_agent'));
  assert.equal((html.match(/data-agent-avatar=/g) || []).length, 3);
  assert.ok(html.includes('Background task empty'));
  const overview = render(React.createElement(RuntimeCenterOverview, { bridge }));
  assert.ok(overview.includes('1 running'));
  assert.ok(overview.includes('3 background tasks'));
});
