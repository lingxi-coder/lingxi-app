import assert from 'node:assert/strict';
import { test } from 'node:test';
import { composerGoalState, goalMessageObjective } from '../src/renderer/components/goalPresentation';
import type { RunItem } from '../src/renderer/model/runItem';
const user = (text: string, delivery?: 'pending' | 'failed'): RunItem => ({ type: 'narration', id: 'user', role: 'user', text, delivery });
const result = (output: string, isError = false): RunItem => ({ type: 'command', id: 'result', name: '/goal', output, isError });
test('goal presentation preserves multiline objectives and leaves control commands alone', () => {
  assert.equal(goalMessageObjective('/goal ship it\nwith tests'), 'ship it\nwith tests');
  for (const value of ['/goal', '/goal CLEAR', '/goal stop', '/review ship it']) assert.equal(goalMessageObjective(value), null);
});
test('goal state follows delivery, rejection, replacement, and clearing', () => {
  assert.equal(composerGoalState([user('/goal ship', 'pending')]).active, false);
  assert.equal(composerGoalState([user('/goal ship', 'failed')]).active, false);
  assert.deepEqual(composerGoalState([user('/goal first'), user('/goal second')]), { active: true, objective: 'second' });
  assert.equal(composerGoalState([user('/goal ship'), result('Only available in trusted workspaces', true)]).active, false);
  assert.equal(composerGoalState([user('/goal ship'), result('Goal cleared: ship')]).active, false);
  assert.equal(composerGoalState([result('Goal active: ship (2 turns)\nLast check: Still working')]).objective, 'ship');
});

test('goal toolbar orders delete, pause or resume, and expand; cancellation disables mutations', async () => {
  const React = await import('react');
  (globalThis as { React?: typeof React }).React = React;
  const { renderToStaticMarkup } = await import('react-dom/server');
  const { GoalStatus } = await import('../src/renderer/components/GoalStatus');
  const { Theme } = await import('../src/renderer/theme/ThemeContext');
  const { tokens } = await import('../src/renderer/theme/tokens');
  const render = (running: boolean, cancelling = false) => renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(false) }, React.createElement(GoalStatus, {
    objective: 'Ship the desktop', disabled: false, running, cancelling,
    onClear: async () => {}, onPause: async () => {}, onResume: async () => {},
  })));
  const active = render(true);
  assert.match(active, /Clear goal[\s\S]*Pause goal[\s\S]*Expand goal/);
  assert.match(active, />Pursuing goal<\/span>/);
  assert.match(render(false), /Resume goal/);
  const cancelling = render(true, true);
  assert.match(cancelling, /Pausing goal/);
  assert.equal((cancelling.match(/disabled=""/g) ?? []).length, 2);
});
