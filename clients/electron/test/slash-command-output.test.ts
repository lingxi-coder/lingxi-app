import { test } from 'node:test';
import assert from 'node:assert/strict';

import { beginSlashCommand, emptyConversation, reduceEvent } from '../src/renderer/bridge/conversation';
import { commandShouldCollapse } from '../src/renderer/model/runItem';

test('a slash command result becomes its own transcript row, not an assistant line', () => {
  const started = beginSlashCommand(emptyConversation(), '/status');
  const state = reduceEvent(started, { type: 'slash_command_result', display: 'Model: opus', is_error: false });

  const last = state.items.at(-1);
  assert.equal(last?.type, 'command');
  assert.equal(last.name, '/status');
  assert.equal(last.output, 'Model: opus');
  assert.equal(last.isError, false);
  // The typed line is still shown above it, as the CLI prints.
  assert.equal(state.items.at(-2)?.type, 'narration');
  assert.equal(state.items.at(-2)?.role, 'user');
  // The pending name is consumed, so a second result cannot inherit it.
  assert.equal(state.pendingSlashName, null);
});

test('an error result is marked, and keeps the error visible to the chrome', () => {
  const started = beginSlashCommand(emptyConversation(), '/nope');
  const state = reduceEvent(started, { type: 'slash_command_result', display: 'Unknown command', is_error: true });

  assert.equal(state.items.at(-1)?.isError, true);
  assert.equal(state.lastError, 'Unknown command');
});

test('a result with no pending command still renders its output', () => {
  const state = reduceEvent(emptyConversation(), { type: 'slash_command_result', display: 'orphan', is_error: false });

  assert.equal(state.items.at(-1)?.type, 'command');
  assert.equal(state.items.at(-1)?.name, '');
  assert.equal(state.items.at(-1)?.output, 'orphan');
});

test('command output folds only once it is genuinely long', () => {
  const short = { type: 'command', id: 'i1', name: '/status', output: 'one line', isError: false } as const;
  const tall = { ...short, output: Array.from({ length: 40 }, (_, i) => `line ${i}`).join('\n') };
  const wide = { ...short, output: 'x'.repeat(2000) };

  assert.equal(commandShouldCollapse(short), false);
  assert.equal(commandShouldCollapse(tall), true);
  assert.equal(commandShouldCollapse(wide), true);
});
