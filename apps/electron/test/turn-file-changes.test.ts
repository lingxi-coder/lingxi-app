import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { ClientEvent, StructuredDiffDto } from '@lingxi/bridge-client';
import { conversationFromMessages, emptyConversation, reduceEvent, reduceEvents } from '../src/renderer/bridge/conversation';
import type { MetaRunItem } from '../src/renderer/model/runItem';
const diff = (path = 'src/app.ts', additions = 2, removals = 1): StructuredDiffDto => ({
  file_path: path, additions, removals, gutter_width: 2, truncated_rows: 0,
  rows: [{ kind: 'add', line_no: 1, hunk: 0, segments: [{ text: 'updated', class: 'plain' }] }],
});
const end = (formatted = ''): ClientEvent => ({ type: 'turn_ended', outcome: { type: 'end_turn' },
  cost: { formatted, total_usd: 0, input_tokens: 0, output_tokens: 0, api_calls: 1, session_duration_secs: 1 },
});
const start = (id: string): ClientEvent => ({ type: 'tool_use_started', id, tool: 'Edit', input_json: '{}' });
const result = (id: string, value = diff(), is_error = false): ClientEvent => ({
  type: 'tool_use_result', id, tool: 'Edit', is_error, result_json: '{}', display: { body_lines: 0, diff: value },
});
const metas = (state: ReturnType<typeof emptyConversation>) => state.items.filter((item): item is MetaRunItem => item.type === 'meta');

test('appends completed-turn summary with cumulative same-file counts and all diffs', () => {
  let state = reduceEvents(emptyConversation(), [{ type: 'turn_started' }, start('a'), result('a'),
    start('b'), result('b', diff('src/app.ts', 3, 4)), start('c'), result('c', diff('other.ts', 1, 0))]);
  assert.equal(metas(state).length, 0);
  state = reduceEvent(state, end('1s'));
  const summary = metas(state)[0]!;
  assert.equal(state.items.at(-1), summary);
  assert.equal(summary.dur, '1s');
  assert.deepEqual(summary.files?.map(({ path, additions, removals, diffs }) => [path, additions, removals, diffs.length]),
    [['src/app.ts', 5, 5, 2], ['other.ts', 1, 0, 1]]);
});
test('excludes failures, unfinished calls, no-op edits, and missing paths', () => {
  const state = reduceEvents(emptyConversation(), [start('failed'), result('failed', diff(), true),
    start('running'), start('noop'), result('noop', diff('noop.ts', 0, 0)),
    start('missing'), result('missing', diff('')), end()]);
  assert.equal(metas(state).length, 0);
});
test('every terminal boundary isolates edits without formatted cost or another turn_started', () => {
  let state = reduceEvents(emptyConversation(), [start('first'), result('first'), end()]);
  const first = metas(state)[0];
  state = reduceEvents(state, [end(), start('failed'), result('failed', diff(), true), end(),
    start('next'), result('next', diff('next.ts')), end()]);
  assert.equal(metas(state).length, 2);
  assert.equal(metas(state)[0], first);
  assert.deepEqual(metas(state)[1]?.files?.map((file) => file.path), ['next.ts']);
});
test('summary snapshots survive payload changes and late prior-turn results', () => {
  const payload = diff();
  let state = reduceEvents(emptyConversation(), [start('old'), result('old', payload), end()]);
  payload.rows[0]!.segments[0]!.text = 'mutated';
  state = reduceEvents(state, [{ type: 'turn_started' }, result('old', diff('wrong.ts')), end()]);
  assert.equal(metas(state).length, 1);
  assert.equal(metas(state)[0]?.files?.[0]?.diffs[0]?.rows[0]?.segments[0]?.text, 'updated');
});
test('orphan confirmed result contributes once; duplicate result does not double count', () => {
  const state = reduceEvents(emptyConversation(), [result('orphan'), result('orphan'), end()]);
  assert.equal(metas(state)[0]?.files?.[0]?.additions, 2);
});
test('historical edits are not attributed to the next live turn', () => {
  const history = conversationFromMessages([{ role: 'assistant', blocks: [
    { type: 'tool_use', id: 'past', tool: 'Edit', input_json: '{}' },
  ] }, { role: 'user', blocks: [
    { type: 'tool_result', id: 'past', tool: 'Edit', is_error: false, result_json: '{}', display: { body_lines: 0, diff: diff() } },
  ] }]);
  const state = reduceEvents(history, [{ type: 'turn_started' }, end('1s')]);
  assert.equal(metas(state)[0]?.files, undefined);
});

test('cancelled turns retain confirmed edits but exclude interrupted calls', () => {
  const terminal = end();
  if (terminal.type !== 'turn_ended') throw new Error('expected terminal event');
  const state = reduceEvents(emptyConversation(), [start('saved'), result('saved'), start('interrupted'),
    { ...terminal, outcome: { type: 'cancelled' } }]);
  assert.equal(metas(state)[0]?.files?.length, 1);
  assert.equal(metas(state)[0]?.files?.[0]?.diffs.length, 1);
});
