import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { contextWindowUsage, ContextWindow } from '../src/renderer/components/ContextWindow';
import { formatTokens } from '../src/renderer/formatTokens';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import { emptyConversation, reduceEvent } from '../src/renderer/bridge/conversation';
(globalThis as { React?: typeof React }).React = React;
const usage = { inputTokens: 9000, outputTokens: 1000, cacheReadTokens: 65000, cacheCreationTokens: 4000 };
test('context matches Claude input plus cache usage, excluding generated output', () => {
  assert.deepEqual(contextWindowUsage(usage, 475000), { used: 78000, capacity: 475000, percent: 16 });
});
test('unknown and invalid counters do not report an empty context', () => {
  for (const capacity of [undefined, 0, -1, NaN, Infinity]) assert.equal(contextWindowUsage(usage, capacity), null);
  assert.equal(contextWindowUsage(null, 475000), null);
  assert.equal(contextWindowUsage({ ...usage, inputTokens: -1 }, 475000), null);
  assert.equal(contextWindowUsage({ ...usage, cacheReadTokens: NaN }, 475000), null);
});
test('over capacity clamps percentage while retaining actual token count', () => {
  assert.deepEqual(contextWindowUsage(usage, 50000), { used: 78000, capacity: 50000, percent: 100 });
});
test('accessible trigger reports estimate and unknown state', () => {
  const render = (value: typeof usage | null) => renderToStaticMarkup(React.createElement(Theme.Provider,
    { value: tokens(true) }, React.createElement(ContextWindow, { usage: value, capacity: 475000 })));
  assert.match(render(usage), /16% used \(84% left\)/);
  assert.match(render(usage), /78k \/ 475k tokens used/);
  assert.doesNotMatch(render(usage), /≈/);
  assert.match(render(usage), /estimated/);
  assert.match(render(null), /Usage unavailable/);
  assert.doesNotMatch(render(null), /0% used/);
});
test('million-scale values read as M instead of a four-digit k count', () => {
  const render = (value: typeof usage | null, capacity: number) => renderToStaticMarkup(React.createElement(Theme.Provider,
    { value: tokens(true) }, React.createElement(ContextWindow, { usage: value, capacity })));
  const promptOnly = { inputTokens: 79_000, outputTokens: 0, cacheReadTokens: 0, cacheCreationTokens: 0 };
  assert.match(render(promptOnly, 1_000_000), /79k \/ 1M tokens used/);
  assert.match(render(null, 1_000_000), /1M token capacity/);
  assert.doesNotMatch(render(promptOnly, 999_999), /1000k/);
  assert.equal(formatTokens(1_000_000), '1M');
  assert.equal(formatTokens(1_048_576), '1M');
  assert.equal(formatTokens(1_310_720), '1.3M');
  assert.equal(formatTokens(475_000), '475k');
  assert.equal(formatTokens(999), '999');
});
test('output-only usage preserves prompt cache, next request replaces it, compaction clears it', () => {
  let state = reduceEvent(emptyConversation(), { type: 'usage_update', input_tokens: 9000, output_tokens: 0, cache_read_tokens: 65000, cache_creation_tokens: 4000 });
  state = reduceEvent(state, { type: 'usage_update', input_tokens: 0, output_tokens: 1000, cache_read_tokens: 0, cache_creation_tokens: 0 });
  assert.deepEqual(state.usage, usage);
  state = reduceEvent(state, { type: 'usage_update', input_tokens: 50, output_tokens: 0, cache_read_tokens: 0, cache_creation_tokens: 0 });
  assert.deepEqual(state.usage, { inputTokens: 50, outputTokens: 0, cacheReadTokens: 0, cacheCreationTokens: 0 });
  state = reduceEvent(state, { type: 'compaction_status', phase: 'complete' });
  assert.equal(state.usage, null);
});

test('OpenAI placeholders preserve the last real usage until replacement or compaction', () => {
  const placeholder = { type: 'usage_update' as const, input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_creation_tokens: 0 };
  const fresh = reduceEvent(emptyConversation(), placeholder);
  assert.equal(fresh.usage, null);
  let state = reduceEvent(emptyConversation(), { ...placeholder, input_tokens: 79000 });
  assert.equal(contextWindowUsage(state.usage, 475000)?.percent, 17);
  state = reduceEvent(state, placeholder);
  assert.equal(contextWindowUsage(state.usage, 475000)?.used, 79000);
  state = reduceEvent(state, { ...placeholder, input_tokens: 80000, output_tokens: 1000 });
  assert.equal(contextWindowUsage(state.usage, 475000)?.used, 80000);
});

test('generated output does not change the context percentage', () => {
  assert.deepEqual(contextWindowUsage({ ...usage, outputTokens: 100000 }, 475000), contextWindowUsage(usage, 475000));
});

test('restored snapshots replace every bucket, including valid zeros', () => {
  const snapshot = { type: 'usage_update' as const, is_snapshot: true, input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_creation_tokens: 0 };
  let state = reduceEvent(emptyConversation(), { ...snapshot, input_tokens: 100, cache_read_tokens: 200 });
  state = reduceEvent(state, { ...snapshot, output_tokens: 10 });
  assert.deepEqual(state.usage, { inputTokens: 0, outputTokens: 10, cacheReadTokens: 0, cacheCreationTokens: 0 });
  state = reduceEvent(state, snapshot);
  assert.deepEqual(contextWindowUsage(state.usage, 475000), { used: 0, capacity: 475000, percent: 0 });
  const html = renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) },
    React.createElement(ContextWindow, { usage: state.usage, capacity: 475000 })));
  assert.match(html, /0% used/);
  assert.doesNotMatch(html, /Usage unavailable/);
});
