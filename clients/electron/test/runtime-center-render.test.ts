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

import { RuntimeCenterOverview, RuntimeCenterInspector } from '../src/renderer/components/RuntimeCenter';
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

test('pinned summary is a nonmodal region with four ordered empty categories', () => {
  const bridge = runtimeBridge();
  bridge.desktop = emptyDesktopState();
  const html = render(React.createElement(RuntimeCenterOverview, { bridge }));
  assert.ok(html.includes('role="region"'));
  assert.ok(html.includes('aria-label="Pinned summary"'));
  assert.ok(!html.includes('role="dialog"'));
  const headings = [...html.matchAll(/<h2>(.*?)<\/h2>/g)].map((match) => match[1]);
  assert.deepEqual(headings, ['Subagents', 'Todos', 'Resources', 'Plan']);
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
  assert.ok(html.includes('local_fusion'));
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
