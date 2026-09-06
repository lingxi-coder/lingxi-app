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
