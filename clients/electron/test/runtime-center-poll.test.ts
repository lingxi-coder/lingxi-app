/**
 * [Finding 5]: the desktop client's `TaskRow` (and therefore
 * `task.stage`/`status`/`description`, read by `TaskDetail` and the
 * overview subtitle) is pull-only -- nothing re-delivers it while a task
 * runs unless something re-issues `task_list`. Before this fix,
 * `RuntimeCenter.tsx` wired exactly one interval (`bridge.taskOutput`
 * polling) and NOTHING re-fetched the row itself, so a running `/fusion`
 * task's stage line froze at whatever it read the instant the panel
 * opened, for the rest of the run.
 *
 * [Finding 5, rework round 2]: the round-1 fix wired `bridge.refreshTasks()`
 * into a `useEffect` keyed on `hasActiveTask`/`task?.status.type`, and
 * `refreshTasks()` wiped `tasks`/`taskOutput` before repopulating them
 * (`beginTaskRefresh`). That wipe flipped the dependency false for one
 * render on every poll, re-running the effect and issuing another
 * `task_list` -- a self-amplifying storm with zero interval ticks
 * required. The fix here is two-part: (a) `bridge.refreshTasks({ preserve:
 * true })` skips the wipe so a background poll merges rows instead of
 * blanking the map, and (b) the in-flight flag is read from a `ref` on
 * every poll tick (`startPollingWhileActive`) instead of sitting in the
 * effect's own dependency array, so a status flip can never retrigger the
 * effect that issued the poll. The tests below cover both halves, plus a
 * regression guard that scans the actual `useEffect` dependency arrays so
 * the storm shape cannot silently come back.
 *
 * This tests the exact functions the RuntimeCenter task-detail pane and
 * overview panel now wire into `bridge.refreshTasks()`, using node:test's
 * built-in timer mock (`node:test`'s `mock.timers`, stable in this Node
 * version) rather than mounting React -- this package ships neither jsdom
 * nor react-test-renderer, and `renderToStaticMarkup` (used by the sibling
 * `runtime-center-render.test.ts`) never runs `useEffect` at all, which is
 * exactly why the original /fusion stage-render fix's test suite stayed
 * green with this defect present.
 */

import { test, mock } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

import type { TaskRowDto } from '@lingxi/bridge-client';

import { hasInFlightTask, startPollingWhileActive } from '../src/renderer/components/RuntimeCenter';
import { beginTaskRefresh, emptyDesktopState, orderedTasks, type DesktopState } from '../src/renderer/bridge/desktopState';

test('hasInFlightTask is true only for pending/running/paused tasks', () => {
  assert.equal(hasInFlightTask([{ status: { type: 'running' } }]), true);
  assert.equal(hasInFlightTask([{ status: { type: 'pending' } }]), true);
  assert.equal(hasInFlightTask([{ status: { type: 'paused' } }]), true);
  assert.equal(hasInFlightTask([{ status: { type: 'completed' } }]), false);
  assert.equal(hasInFlightTask([{ status: { type: 'failed' } }]), false);
  assert.equal(hasInFlightTask([]), false);
  assert.equal(
    hasInFlightTask([{ status: { type: 'completed' } }, { status: { type: 'running' } }]),
    true,
    'any in-flight task among several must count',
  );
});

test('startPollingWhileActive does not call refresh on a tick while isActive() reads false', () => {
  mock.timers.enable({ apis: ['setInterval'] });
  try {
    let calls = 0;
    let active = false;
    const stop = startPollingWhileActive(() => active, async () => { calls += 1; });
    mock.timers.tick(10_000);
    assert.equal(calls, 0, 'no refresh should fire while isActive() reads false');
    stop();
    active = true; // isActive() flips only after the interval was stopped
    mock.timers.tick(10_000);
    assert.equal(calls, 0, 'a stopped interval must never call refresh no matter what isActive() reads later');
  } finally {
    mock.timers.reset();
  }
});

test('startPollingWhileActive polls with { preserve: true } on a 1.5s interval while isActive() reads true, reading it FRESH each tick, and stops on cleanup', () => {
  mock.timers.enable({ apis: ['setInterval'] });
  try {
    let calls = 0;
    let preserveFlags: Array<boolean | undefined> = [];
    let active = true;
    const stop = startPollingWhileActive(() => active, async (options) => { calls += 1; preserveFlags.push(options?.preserve); });
    assert.equal(calls, 0, 'must not call immediately, only on the interval tick');
    mock.timers.tick(1_500);
    assert.equal(calls, 1, 'the running task must trigger a row refresh every 1.5s');
    // [Finding 5, rework round 2] The whole point of the getter shape: the
    // SAME interval, created once, must react to isActive() flipping
    // without itself being torn down and recreated -- that recreation is
    // exactly what stormed under the old `useEffect([..., hasActiveTask])`
    // wiring.
    active = false;
    mock.timers.tick(1_500);
    assert.equal(calls, 1, 'must skip the tick once isActive() reads false, using the SAME interval (no restart)');
    active = true;
    mock.timers.tick(1_500 * 2);
    assert.equal(calls, 3, 'must resume polling once isActive() reads true again, still the same interval');
    assert.deepEqual(preserveFlags, [true, true, true], 'every background poll tick must pass { preserve: true } -- it must merge rows, never wipe the task map first');
    stop();
    mock.timers.tick(10_000);
    assert.equal(calls, 3, 'stopping must clear the interval -- no calls after cleanup');
  } finally {
    mock.timers.reset();
  }
});

// ---------------------------------------------------------------------------
// [Finding 5, rework round 1] The two tests above only exercise
// `hasInFlightTask`/`startActiveRefresh` as bare functions -- they prove
// the helpers work, not that `RuntimeCenter.tsx` actually WIRES them into
// the two `useEffect`s that call `bridge.refreshTasks()`. Nothing else in
// this suite mounts React (`runtime-center-render.test.ts` uses
// `renderToStaticMarkup`, which never runs effects), so deleting either
// wiring effect entirely was a silent, unguarded regression.
//
// This package has neither jsdom nor react-test-renderer, so -- following
// the same idiom as `settings-deadcode-guard.test.ts`'s button scanner --
// this reads the component source directly and asserts the two `useEffect`
// bodies actually call `startActiveRefresh(...)` with the right arguments,
// with a self-validation test proving the scanner can find a known-present
// effect first (so a regex that silently matches nothing can't make the
// whole guard vacuous).
// ---------------------------------------------------------------------------

const runtimeCenterSource = readFileSync(
  join(import.meta.dirname, '../src/renderer/components/RuntimeCenter.tsx'),
  'utf8',
);

/** Extracts the full source of `function <name>(...) { ... }` (or `export
 * function`) by brace-counting from the first `{` after the signature, so
 * it survives edits elsewhere in the file rather than pinning line ranges. */
function functionBody(source: string, name: string): string {
  const signature = source.indexOf(`function ${name}(`);
  assert.ok(signature !== -1, `function ${name} must exist in RuntimeCenter.tsx`);
  const parenOpen = source.indexOf('(', signature);
  assert.ok(parenOpen !== -1, `function ${name} must have a parameter list`);
  // Balance the PARAMETER parens first -- the params destructure objects
  // (`{ bridge }: { bridge: UseBridge }`), whose own `{`/`}` would
  // otherwise be mistaken for the body opening if we just grabbed the
  // first `{` after the signature.
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

/** Every `useEffect(() => { ... }, [deps])` call in a function body, as its
 * own `{ body, deps }` source slice pair, found the same brace-counting way.
 * `deps` is the literal `[...]` dependency-array source (including the
 * brackets) that follows the effect body -- kept as its own field (not just
 * folded into `body`) so a test can assert on what the effect depends on,
 * separately from what it does, which is exactly the distinction the
 * round-2 storm bug turned on: the body called `bridge.refreshTasks()`
 * while the deps array ALSO named the value that call mutated. */
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

/** Just the effect bodies, for scans that don't care about dependencies. */
function effectBodies(source: string): string[] {
  return effectEntries(source).map((entry) => entry.body);
}

/** A bare identifier token match (not a substring inside a longer
 * identifier) -- `deps.includes('task')` would also match `activeTask` or
 * `task?.status.type`; this checks for the identifier as its own token. */
function depsListNames(deps: string, identifier: string): boolean {
  return new RegExp(`(^|[^\\w.])${identifier.replace(/[.?]/g, '\\$&')}([^\\w]|$)`).test(deps);
}

test('the effect scanner can actually find the pre-existing bridge.taskOutput polling effect', () => {
  const inspectorBody = functionBody(runtimeCenterSource, 'RuntimeCenterInspector');
  const withTaskOutputPoll = effectBodies(inspectorBody).filter((body) => body.includes('bridge.taskOutput('));
  assert.equal(
    withTaskOutputPoll.length, 1,
    'if the scanner cannot find the already-existing, never-touched taskOutput polling effect, '
    + 'every assertion below proves nothing',
  );
});

test('RuntimeCenterOverview wires a useEffect that fetches on open and polls while a task is active', () => {
  const overviewBody = functionBody(runtimeCenterSource, 'RuntimeCenterOverview');
  const wired = effectBodies(overviewBody).some((body) =>
    body.includes('center.overviewOpen')
    && body.includes('bridge.refreshTasks()')
    && body.includes('startPollingWhileActive(() => hasActiveTaskRef.current, bridge.refreshTasks)'));
  assert.ok(
    wired,
    'RuntimeCenterOverview must contain a useEffect that, while the overview is open, calls '
    + 'bridge.refreshTasks() once and returns '
    + 'startPollingWhileActive(() => hasActiveTaskRef.current, bridge.refreshTasks) '
    + '-- without this a running task\'s row (status/stage/description) never refreshes while the '
    + 'overview panel is open',
  );
});

test('RuntimeCenterInspector wires a useEffect that polls the row refresh while the selected task is in flight', () => {
  const inspectorBody = functionBody(runtimeCenterSource, 'RuntimeCenterInspector');
  const wired = effectBodies(inspectorBody).some((body) =>
    body.includes('startPollingWhileActive(() => taskInFlightRef.current, bridge.refreshTasks)'));
  assert.ok(
    wired,
    'RuntimeCenterInspector must contain a useEffect that returns '
    + 'startPollingWhileActive(() => taskInFlightRef.current, bridge.refreshTasks) -- without this a '
    + "running task's stage/status/description freeze at whatever TaskDetail read the instant the "
    + "inspector pane opened, for the rest of the task's run",
  );
});

// ---------------------------------------------------------------------------
// [Finding 5, rework round 2 -- blocking] The round-1 wiring above passed
// the source scan (it did call `bridge.refreshTasks()` from the right
// effects) but stormed in production: `startActiveRefresh` was gated by a
// plain `boolean` captured from `hasActiveTask`/`hasInFlightTask([task])`,
// which sat in the SAME effect's dependency array, and `refreshTasks()`
// cleared `tasks`/`taskOutput` before repopulating them -- so every single
// poll flipped that boolean false for one render, re-ran the effect, and
// issued another `task_list`. A source scan that only checks "does the body
// call refreshTasks()" cannot see this; it has to check the DEPENDENCY
// ARRAY too. These two tests are that check, plus a dynamic reproduction
// using the real production reducers.
// ---------------------------------------------------------------------------

test('no useEffect that calls bridge.refreshTasks() may list hasActiveTask or task?.status.type in its own dependency array (storm guard)', () => {
  const overviewEntries = effectEntries(functionBody(runtimeCenterSource, 'RuntimeCenterOverview'));
  const inspectorEntries = effectEntries(functionBody(runtimeCenterSource, 'RuntimeCenterInspector'));
  const refreshEffects = [...overviewEntries, ...inspectorEntries].filter((entry) => entry.body.includes('bridge.refreshTasks'));
  assert.ok(
    refreshEffects.length >= 2,
    `expected to find both refreshTasks-calling effects (overview + inspector); found ${refreshEffects.length} -- `
    + 'if the scanner cannot find them, the assertions below prove nothing',
  );
  for (const { deps } of refreshEffects) {
    assert.ok(
      !depsListNames(deps, 'hasActiveTask'),
      'an effect whose body calls bridge.refreshTasks() must not depend on hasActiveTask: refreshTasks used to '
      + `wipe the task map, flipping hasActiveTask and re-running this exact effect every poll -- a `
      + `self-amplifying task_list storm. Found deps: ${deps}`,
    );
    assert.ok(
      !depsListNames(deps, 'task?.status.type'),
      'an effect whose body calls bridge.refreshTasks() must not depend on task?.status.type, for the same reason. '
      + `Found deps: ${deps}`,
    );
  }
});

test('the row-refresh effects read the in-flight ref, not a value captured from the dependency array (storm guard)', () => {
  const overviewEntries = effectEntries(functionBody(runtimeCenterSource, 'RuntimeCenterOverview'));
  const inspectorEntries = effectEntries(functionBody(runtimeCenterSource, 'RuntimeCenterInspector'));
  const overviewRefresh = overviewEntries.find((entry) => entry.body.includes('startPollingWhileActive('));
  const inspectorRefresh = inspectorEntries.find((entry) => entry.body.includes('startPollingWhileActive('));
  assert.ok(overviewRefresh && inspectorRefresh, 'both startPollingWhileActive call sites must exist');
  assert.ok(
    overviewRefresh!.body.includes('startPollingWhileActive(() => hasActiveTaskRef.current, bridge.refreshTasks)'),
    'the overview effect must pass a live getter closing over hasActiveTaskRef, not the hasActiveTask value itself',
  );
  assert.ok(
    inspectorRefresh!.body.includes('startPollingWhileActive(() => taskInFlightRef.current, bridge.refreshTasks)'),
    'the inspector effect must pass a live getter closing over taskInFlightRef, not a value derived from task directly',
  );
});

// ---------------------------------------------------------------------------
// [Finding 5, rework round 2] Dynamic reproduction of the storm mechanism,
// using the REAL production reducers (`beginTaskRefresh`, `orderedTasks`,
// `hasInFlightTask`, `startPollingWhileActive`), with only the React
// render/effect scheduling itself modeled (mirroring the reviewer's own
// probe at c5r2-storm-probe.ts): mount the overview effect once, then drain
// several async `task_row` replies, re-rendering (recomputing the ref) after
// each one exactly like React would. Because the effect's own dependency
// array is `[center.overviewOpen, bridge.refreshTasks]` -- stable across
// every one of those renders -- the effect body must run exactly once no
// matter how many replies land; only a genuine 1.5s timer tick may issue
// the next `task_list`. Under the round-1 shape (`hasActiveTask` in the
// deps array, and an unconditional wipe on every call) this same drain
// pushed the command count to 12 for 5 replies with zero timer ticks.
// ---------------------------------------------------------------------------

test('draining task_row replies after the row-refresh effect mounts never triggers another task_list -- only a real timer tick does (storm reproduction)', () => {
  mock.timers.enable({ apis: ['setInterval'] });
  try {
    const RUNNING_TASK: TaskRowDto = {
      task_id: 'f1',
      task_type: 'local_fusion',
      status: { type: 'running' },
      description: '/fusion compare two approaches',
      stage: 'Running panels 1/3',
    } as TaskRowDto;

    let desktop: DesktopState = { ...emptyDesktopState(), tasks: { f1: RUNNING_TASK } };
    let taskListCommands = 0;
    let effectMounts = 0;
    const pendingReplies: Array<() => void> = [];

    // Mirrors the real requestTaskList (useBridge.ts): a non-preserve call
    // wipes tasks/taskOutput synchronously via beginTaskRefresh; the engine
    // then answers ASYNCHRONOUSLY with one task_row per in-flight task,
    // which desktopState.ts's `case 'task_row'` merges back in.
    const refreshTasks = (options?: { preserve?: boolean }): Promise<void> => {
      taskListCommands += 1;
      if (!options?.preserve) desktop = beginTaskRefresh(desktop);
      pendingReplies.push(() => { desktop = { ...desktop, tasks: { ...desktop.tasks, f1: RUNNING_TASK } }; });
      return Promise.resolve();
    };

    const hasActiveTaskRef = { current: hasInFlightTask(orderedTasks(desktop)) };
    let cleanup: (() => void) | undefined;

    // The effect body, verbatim (RuntimeCenter.tsx's overview useEffect),
    // run once because its dependency array never changes.
    function mountRowRefreshEffect() {
      effectMounts += 1;
      void refreshTasks();
      cleanup = startPollingWhileActive(() => hasActiveTaskRef.current, refreshTasks);
    }
    mountRowRefreshEffect();
    assert.equal(taskListCommands, 1, 'mounting the effect must issue exactly one task_list');

    // Drain the initial reply and re-render, exactly as the engine
    // answering the one-shot open-time fetch would. This alone must not
    // trigger a second task_list or a second effect mount.
    const initialReply = pendingReplies.shift();
    assert.ok(initialReply, 'expected the mount-time task_list to have queued exactly one pending reply');
    initialReply();
    // Re-render: recompute the ref the way a real render commit does.
    // React only re-runs an effect when ITS OWN dependency array changes
    // value -- [center.overviewOpen, bridge.refreshTasks] does not, on
    // this or any later render, so mountRowRefreshEffect must not run
    // again no matter what this ref now reads.
    hasActiveTaskRef.current = hasInFlightTask(orderedTasks(desktop));
    assert.equal(
      effectMounts, 1,
      'the row-refresh effect must mount exactly once -- a second mount from draining the initial reply is '
      + 'the storm (12 task_list commands for 5 replies with zero timer ticks, under the round-1 shape)',
    );
    assert.equal(taskListCommands, 1, 'draining the initial reply must not itself trigger another task_list');

    // Now advance through several real polling intervals. Each tick, and
    // ONLY each tick, may issue exactly one new task_list; draining that
    // poll's reply must likewise never trigger another one on its own.
    for (let tick = 0; tick < 5; tick += 1) {
      mock.timers.tick(1_500);
      assert.equal(taskListCommands, 2 + tick, `interval tick ${tick} must issue exactly one task_list`);
      const reply = pendingReplies.shift();
      assert.ok(reply, `expected a pending reply after interval tick ${tick}`);
      reply();
      hasActiveTaskRef.current = hasInFlightTask(orderedTasks(desktop));
      assert.equal(effectMounts, 1, `the row-refresh effect must still be mounted exactly once after tick ${tick}`);
      assert.equal(
        taskListCommands, 2 + tick,
        `draining the reply from tick ${tick} must not itself trigger another task_list`,
      );
    }

    cleanup?.();
  } finally {
    mock.timers.reset();
  }
});
