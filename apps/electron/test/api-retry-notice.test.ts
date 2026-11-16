import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import type { ClientEvent } from '@lingxi/bridge-client';
import { emptyDesktopState, reduceDesktopEvent } from '../src/renderer/bridge/desktopState';
import { Stage } from '../src/renderer/components/Stage';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
(globalThis as { React?: typeof React }).React = React;

const retry = (attempt = 2): ClientEvent =>
  ({ type: 'api_retry', message: 'rate limited', attempt, max_retries: 5, delay_ms: 30_000 }) as ClientEvent;
const reduce = (...events: readonly ClientEvent[]) =>
  events.reduce(reduceDesktopEvent, emptyDesktopState());

test('a retry becomes current state and is also kept for diagnostics', () => {
  const s = reduce(retry());
  assert.equal(s.apiRetry?.attempt, 2);
  assert.equal(s.lastApiRetry?.attempt, 2, 'the Diagnostics record still exists');
});

test('the next thing the turn does ends the wait', () => {
  // Each of these means the backoff is over. `turn_ended` alone is not enough:
  // a retry that succeeds streams for the rest of the turn.
  for (const done of [
    { type: 'text_delta', text: 'hi' },
    { type: 'thinking_delta', text: 'hm' },
    { type: 'tool_use_started', id: 't', tool: 'Read', input_json: '{}',
      header: { verb: 'read', label: 'Read', title: 'Read' } },
    { type: 'turn_started' },
    { type: 'error', kind: 'transport', message: 'x' },
    { type: 'turn_ended', cost: null },
  ] as unknown as ClientEvent[]) {
    const s = reduce(retry(), done);
    assert.equal(s.apiRetry, null, `${done.type} must clear the waiting notice`);
    assert.ok(s.lastApiRetry, `${done.type} must not erase the Diagnostics record`);
  }
});

test('the notice never survives into another session', () => {
  for (const boundary of [
    { type: 'session_started', session_id: 's', mode: 'code' },
    { type: 'session_resumed', session_id: 's', mode: 'code', messages: [] },
    { type: 'session_ended' },
  ] as unknown as ClientEvent[]) {
    assert.equal(reduce(retry(), boundary).apiRetry, null, boundary.type);
  }
});

const stage = (apiRetry: unknown) => renderToStaticMarkup(React.createElement(
  Theme.Provider, { value: tokens(false) },
  React.createElement(Stage, { liveItems: [], running: true, sessionKey: 'k', apiRetry } as never),
));

test('the reconnect notice is neutral and exposes expanded error details', () => {
  const html = stage(reduce(retry()).apiRetry);
  assert.match(html, /--retry-text:/);
  assert.doesNotMatch(html, /--sweep-base:/);
  assert.match(html, /aria-expanded="true"/);
  assert.match(html, /aria-controls="disclosure-body-/);
  assert.match(html, /transcript-api-retry-icon/);
  assert.match(html, /class="transcript-api-retry-message">rate limited<\/p>/);
});

test('the retry replaces the bare Thinking indicator the user was stuck on', () => {
  const waiting = stage(null);
  assert.match(waiting, /Thinking/);
  const retrying = stage(reduce(retry()).apiRetry);
  assert.doesNotMatch(retrying, /Thinking/, 'a multi-minute backoff must not read as "Thinking…"');
  assert.match(retrying, /Reconnecting 2\/5/);
  assert.match(retrying, /rate limited/, 'the reason is what tells the user to stop waiting');
});
