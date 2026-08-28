import { test } from 'node:test';
import assert from 'node:assert/strict';

import { beginLocalSlashCommand, beginSlashCommand, emptyConversation, reduceEvent } from '../src/renderer/bridge/conversation';
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

const COST = {
  total_usd: 0,
  input_tokens: 0,
  output_tokens: 0,
  api_calls: 0,
  session_duration_secs: 0,
  formatted: '0s',
} as const;

test('a display-only slash command locks the composer and its own result releases it', () => {
  const started = beginSlashCommand(emptyConversation(), '/status');
  // The composer must lock the instant the command crosses the bridge --
  // before any engine response, exactly like an ordinary prompt.
  assert.equal(started.running, true);

  const state = reduceEvent(started, { type: 'slash_command_result', display: 'Model: opus', is_error: false });
  assert.equal(state.running, false);
});

test('a slash command that expands into a turn keeps the composer locked through its own display-only result', () => {
  const started = beginSlashCommand(emptyConversation(), '/security-review');
  assert.equal(started.running, true);

  // turn_started proves the command became a real turn: it takes ownership
  // of `running` and clears the pre-claim so a later result can't touch it.
  const turnStarted = reduceEvent(started, { type: 'turn_started' });
  assert.equal(turnStarted.running, true);
  assert.equal(turnStarted.pendingSlashName, null);

  // router.rs:938 can still emit a display-only result as a fallback for a
  // command that already became a turn; releasing on it would unlock the
  // composer in the middle of a live turn.
  const afterResult = reduceEvent(turnStarted, { type: 'slash_command_result', display: 'started', is_error: false });
  assert.equal(afterResult.running, true);
});

test('turn_ended still releases the composer for a command that became a turn', () => {
  const started = beginSlashCommand(emptyConversation(), '/security-review');
  const turnStarted = reduceEvent(started, { type: 'turn_started' });
  const ended = reduceEvent(turnStarted, { type: 'turn_ended', outcome: { type: 'end_turn' }, cost: COST });

  assert.equal(ended.running, false);
});

test('a slash command that never gets a slash_command_result releases the composer on error', () => {
  const started = beginSlashCommand(emptyConversation(), '/security-review');
  assert.equal(started.running, true);

  // A transport failure, or the engine's own no-dispatcher fallback
  // (bridge-server/src/router.rs:953), never sends slash_command_result or
  // turn_started/turn_ended -- this `error` is the claim's only terminal
  // event, so it must release the composer itself.
  const state = reduceEvent(started, { type: 'error', message: 'boom' });
  assert.equal(state.running, false);
  assert.equal(state.pendingSlashName, null);
});

test('an error during an ordinary turn does not release the composer early', () => {
  // No slash claim outstanding: turn_started already cleared pendingSlashName
  // (or this is a plain prompt turn that never set it). turn_ended remains
  // the only lifecycle event allowed to release the composer here.
  let s = reduceEvent(emptyConversation(), { type: 'turn_started' });
  assert.equal(s.pendingSlashName, null);
  s = reduceEvent(s, { type: 'error', message: 'a tool call failed' });

  assert.equal(s.running, true);
});

test('a locally-handled command never claims running, unlike an engine-forwarded one', () => {
  // beginSlashCommand (engine path) pre-claims the composer, released only by
  // a slash_command_result/error/turn_ended that a LOCAL command never gets.
  const forwarded = beginSlashCommand(emptyConversation(), '/security-review');
  assert.equal(forwarded.running, true);

  // beginLocalSlashCommand (desktop path) must not make that claim at all --
  // there is no engine event coming to release it, so claiming it here would
  // brick the composer after a bare /model, /permissions, /effort, /theme, or
  // /config.
  const local = beginLocalSlashCommand(emptyConversation(), '/model');
  assert.equal(local.running, false);

  // A local command never receives a slash_command_result, error, or
  // turn_started/turn_ended -- so `running` simply stays false; nothing ever
  // arrives to change it, and there is no claim left to release.
  assert.equal(local.running, false);
});

test('command output folds only once it is genuinely long', () => {
  const short = { type: 'command', id: 'i1', name: '/status', output: 'one line', isError: false } as const;
  const tall = { ...short, output: Array.from({ length: 40 }, (_, i) => `line ${i}`).join('\n') };
  const wide = { ...short, output: 'x'.repeat(2000) };

  assert.equal(commandShouldCollapse(short), false);
  assert.equal(commandShouldCollapse(tall), true);
  assert.equal(commandShouldCollapse(wide), true);
});
