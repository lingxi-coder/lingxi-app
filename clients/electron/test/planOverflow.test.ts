/**
 * The pinned plan strip's pure helpers.
 *
 * `planOverflowSummary` is a 1:1 port of Rust
 * `tui_core::tool_display::plan::overflow_summary`; the cases below mirror that
 * module's own tests so a drift in either spelling fails on both sides.
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import type { PlanTaskDto, PlanTaskStateDto } from '@lingxi/bridge-client';
import {
  MAX_VISIBLE_TASKS,
  PLAN_GLYPH,
  planGlyph,
  planOverflowSummary,
  planWindow,
} from '../src/renderer/components/planOverflow';

const task = (subject: string, state: PlanTaskStateDto): PlanTaskDto => ({ subject, state });

test('planOverflowSummary matches the terminal copy', () => {
  assert.equal(
    planOverflowSummary([task('a', 'in_progress'), task('b', 'pending')]),
    '+1 in progress, 1 pending',
  );
});

test('planOverflowSummary omits zero clauses and keeps the order', () => {
  assert.equal(
    planOverflowSummary([task('a', 'completed'), task('b', 'completed')]),
    '+2 completed',
  );
  assert.equal(
    planOverflowSummary([task('a', 'completed'), task('b', 'in_progress'), task('c', 'pending')]),
    // in progress → pending → completed, regardless of input order.
    '+1 in progress, 1 pending, 1 completed',
  );
  assert.equal(planOverflowSummary([]), null);
});

test('planOverflowSummary emits exactly one clause when only one state is hidden', () => {
  assert.equal(planOverflowSummary([task('a', 'pending')]), '+1 pending');
  assert.equal(planOverflowSummary([task('a', 'in_progress')]), '+1 in progress');
  // Never a trailing separator, never a bare "+".
  assert.doesNotMatch(planOverflowSummary([task('a', 'pending')]) ?? '', /,/);
});

test('the checklist glyphs match the terminal', () => {
  assert.equal(PLAN_GLYPH.pending, '◻');
  assert.equal(PLAN_GLYPH.in_progress, '◼');
  assert.equal(PLAN_GLYPH.completed, '✔');
  assert.equal(planGlyph('in_progress'), '◼');
});

test('a task state named after an Object.prototype member renders a glyph, not a function', () => {
  // `task.state` is a wire value and `Object.freeze({…})` keeps the prototype,
  // so `PLAN_GLYPH[state]` used to answer `constructor` with a FUNCTION — which
  // React renders as nothing at best.
  for (const state of ['constructor', 'toString', 'valueOf', '__proto__']) {
    const glyph = planGlyph(state as PlanTaskStateDto);
    assert.equal(typeof glyph, 'string', state);
    assert.equal(glyph, PLAN_GLYPH.pending, state);
  }
  assert.equal((PLAN_GLYPH as unknown as Record<string, unknown>)['constructor'], undefined);
});

test('a short plan shows every row and hides nothing', () => {
  const tasks = [task('a', 'completed'), task('b', 'in_progress'), task('c', 'pending')];
  const { visible, hidden } = planWindow(tasks);
  assert.equal(visible, tasks, 'a short plan should not be copied');
  assert.deepEqual(hidden, []);
  assert.equal(planOverflowSummary(hidden), null);
});

test('an overflowing plan keeps the FIRST rows, exactly like the terminal', () => {
  // Mirrors `input_status::task_lines_with_limit`: `take(max_visible)` then
  // `overflow_summary(&tasks[max_visible..])`. No cleverer windowing — that
  // would be a fresh divergence between surfaces.
  const tasks: PlanTaskDto[] = [
    task('1', 'completed'),
    task('2', 'completed'),
    task('3', 'completed'),
    task('4', 'completed'),
    task('5', 'in_progress'),
    task('6', 'pending'),
    task('7', 'pending'),
    task('8', 'completed'),
  ];
  const { visible, hidden } = planWindow(tasks);
  assert.equal(visible.length, MAX_VISIBLE_TASKS);
  assert.deepEqual(visible.map((t) => t.subject), ['1', '2', '3', '4', '5']);
  assert.deepEqual(hidden.map((t) => t.subject), ['6', '7', '8']);
  assert.equal(planOverflowSummary(hidden), '+2 pending, 1 completed');
});

test('the limit is caller-overridable and the boundary is exact', () => {
  const tasks = Array.from({ length: 5 }, (_, i) => task(String(i + 1), 'pending'));
  // Exactly at the cap ⇒ nothing hidden.
  assert.deepEqual(planWindow(tasks).hidden, []);
  assert.equal(planWindow(tasks, 3).visible.length, 3);
  assert.equal(planOverflowSummary(planWindow(tasks, 3).hidden), '+2 pending');
});
