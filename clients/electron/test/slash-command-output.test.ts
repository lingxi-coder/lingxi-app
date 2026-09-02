import { test } from 'node:test';
import assert from 'node:assert/strict';

import { beginLocalSlashCommand, beginSlashCommand, emptyConversation, reduceEvent } from '../src/renderer/bridge/conversation';
import {
  commandDefaultOpen,
  commandDiagnosticTone,
  commandPresentation,
  commandShouldCollapse,
  parseCommandHelp,
  parseCommandMetrics,
} from '../src/renderer/model/runItem';

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

test('a display-only slash command claims running state and its own result releases it', () => {
  const started = beginSlashCommand(emptyConversation(), '/status');
  // Running state must be claimed the instant the command crosses the bridge --
  // before any engine response, exactly like an ordinary prompt.
  assert.equal(started.running, true);

  const state = reduceEvent(started, { type: 'slash_command_result', display: 'Model: opus', is_error: false });
  assert.equal(state.running, false);
});

test('a slash command that expands into a turn keeps running state through its own display-only result', () => {
  const started = beginSlashCommand(emptyConversation(), '/security-review');
  assert.equal(started.running, true);

  // turn_started proves the command became a real turn: it takes ownership
  // of `running` and clears the pre-claim so a later result can't touch it.
  const turnStarted = reduceEvent(started, { type: 'turn_started' });
  assert.equal(turnStarted.running, true);
  assert.equal(turnStarted.pendingSlashName, null);

  // router.rs:938 can still emit a display-only result as a fallback for a
  // command that already became a turn; releasing on it would unlock the
  // running state in the middle of a live turn.
  const afterResult = reduceEvent(turnStarted, { type: 'slash_command_result', display: 'started', is_error: false });
  assert.equal(afterResult.running, true);
});

test('turn_ended still releases running state for a command that became a turn', () => {
  const started = beginSlashCommand(emptyConversation(), '/security-review');
  const turnStarted = reduceEvent(started, { type: 'turn_started' });
  const ended = reduceEvent(turnStarted, { type: 'turn_ended', outcome: { type: 'end_turn' }, cost: COST });

  assert.equal(ended.running, false);
});

test('a slash command that never gets a slash_command_result releases running state on error', () => {
  const started = beginSlashCommand(emptyConversation(), '/security-review');
  assert.equal(started.running, true);

  // A transport failure, or the engine's own no-dispatcher fallback
  // (bridge-server/src/router.rs:953), never sends slash_command_result or
  // turn_started/turn_ended -- this `error` is the claim's only terminal
  // event, so it must release running state itself.
  const state = reduceEvent(started, { type: 'error', message: 'boom' });
  assert.equal(state.running, false);
  assert.equal(state.pendingSlashName, null);
});

test('an error during an ordinary turn does not release running state early', () => {
  // No slash claim outstanding: turn_started already cleared pendingSlashName
  // (or this is a plain prompt turn that never set it). turn_ended remains
  // the only lifecycle event allowed to release running state here.
  let s = reduceEvent(emptyConversation(), { type: 'turn_started' });
  assert.equal(s.pendingSlashName, null);
  s = reduceEvent(s, { type: 'error', message: 'a tool call failed' });

  assert.equal(s.running, true);
});

test('a locally-handled command never claims running, unlike an engine-forwarded one', () => {
  // beginSlashCommand (engine path) pre-claims running state, released only by
  // a slash_command_result/error/turn_ended that a LOCAL command never gets.
  const forwarded = beginSlashCommand(emptyConversation(), '/security-review');
  assert.equal(forwarded.running, true);

  // beginLocalSlashCommand (desktop path) must not make that claim at all --
  // there is no engine event coming to release it, so claiming it here would
  // leave stale running state after a bare /model, /permissions, /effort, /theme, or
  // /config.
  const local = beginLocalSlashCommand(emptyConversation(), '/model');
  assert.equal(local.running, false);

  // A local EMITTING command (e.g. `/model nope`, `/theme sepia`, `/config
  // extra`) does push a SYNTHETIC slash_command_result through this same
  // reducer -- `emitCommandOutput` (useBridge.ts) builds exactly this event.
  // That is where pendingSlashName's double duty (label + release guard)
  // actually fires for a local command, and it must not disturb `running`:
  // it was never claimed, so there is no claim to release, but the row still
  // has to land labelled with the command name and the pending name still
  // has to be consumed so a later result can't inherit it.
  const afterSyntheticResult = reduceEvent(local, { type: 'slash_command_result', display: 'Unknown model: nope', is_error: true });
  assert.equal(afterSyntheticResult.running, false);
  assert.equal(afterSyntheticResult.pendingSlashName, null);
  const row = afterSyntheticResult.items.at(-1);
  assert.equal(row?.type, 'command');
  assert.equal(row.name, '/model');
  assert.equal(row.output, 'Unknown model: nope');
  assert.equal(row.isError, true);
});

test('command output folds only once it is genuinely long', () => {
  const short = { type: 'command', id: 'i1', name: '/status', output: 'one line', isError: false } as const;
  const tall = { ...short, output: Array.from({ length: 40 }, (_, i) => `line ${i}`).join('\n') };
  const wide = { ...short, output: 'x'.repeat(2000) };

  assert.equal(commandShouldCollapse(short), false);
  assert.equal(commandShouldCollapse(tall), true);
  assert.equal(commandShouldCollapse(wide), true);
});

test('help output starts expanded so the command never looks inert', () => {
  const help = { type: 'command', id: 'i1', name: '/help', output: 'Commands:\n  /help  Show help', isError: false } as const;
  const status = { ...help, name: '/status', output: 'Status' };

  assert.equal(commandDefaultOpen(help), true);
  assert.equal(commandDefaultOpen(status), true);
});

test('slash results select distinct semantic presentations', () => {
  const item = (name: string, output = 'ok', isError = false) => ({
    type: 'command', id: name, name, output, isError,
  } as const);

  assert.equal(commandPresentation(item('/help')).kind, 'help');
  assert.equal(commandPresentation(item('/usage')).kind, 'metrics');
  assert.equal(commandPresentation(item('/doctor')).kind, 'diagnostics');
  assert.equal(commandPresentation(item('/mcp')).kind, 'catalog');
  assert.equal(commandPresentation(item('/mcp')).title, 'MCP servers');
  assert.equal(commandPresentation(item('/compact')).kind, 'action');
  assert.equal(commandPresentation(item('/custom')).kind, 'plain');
  assert.equal(commandPresentation(item('/status', 'failed', true)).kind, 'error');
  assert.equal(commandDefaultOpen(item('/doctor', 'x'.repeat(2000))), true);
});

test('diagnostic rows distinguish success, warning, and neutral detail', () => {
  assert.equal(commandDiagnosticTone('[OK] git: ok'), 'ok');
  assert.equal(commandDiagnosticTone('[!!] api-key: warning'), 'warning');
  assert.equal(commandDiagnosticTone('ANTHROPIC_API_KEY not set'), 'warning');
  assert.equal(commandDiagnosticTone('git version 2.44.0'), 'info');
});

test('help and metric parsers preserve content without depending on exact widths', () => {
  assert.deepEqual(parseCommandHelp('Commands:\n  /help     Show help\n  /status   Show status'), [
    { name: '/help', description: 'Show help' },
    { name: '/status', description: 'Show status' },
  ]);
  assert.deepEqual(parseCommandMetrics('Model: opus\nTokens: 1,024\nnoise'), [
    { label: 'Model', value: 'opus' },
    { label: 'Tokens', value: '1,024' },
  ]);
});
