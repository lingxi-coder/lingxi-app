import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

import { DESKTOP_COMMANDS } from '../src/renderer/bridge/desktopCommands';

const REGISTER_RS = new URL('../../../lingxi-code/commands/core/src/register.rs', import.meta.url);
const NAMES_RS = new URL('../../../lingxi-code/command-api/src/builtin_support/names.rs', import.meta.url);

/**
 * Names the ENGINE answers with "available in interactive TUI mode only"
 * (`register_interactive_only_commands`). A desktop is an interactive client,
 * so each one is either handled here or explicitly deferred.
 */
function interactiveOnlyNames(): string[] {
  const source = readFileSync(REGISTER_RS, 'utf8');
  const fn = source.indexOf('pub fn register_interactive_only_commands');
  assert.notEqual(fn, -1, 'register_interactive_only_commands is gone — this gate is reading the wrong file');
  const open = source.indexOf('for name in [', fn);
  assert.notEqual(open, -1, 'the interactive-only name array moved — update this extraction');
  const close = source.indexOf('] {', open);
  assert.notEqual(close, -1, 'the interactive-only name array is unterminated');
  const names = source
    .slice(open, close)
    .split('\n')
    .filter((line) => !line.trim().startsWith('//'))
    .flatMap((line) => [...line.matchAll(/"([a-z0-9-]+)"/g)].map((match) => match[1]!));
  // A regex that silently matches nothing would make every caller's
  // assertions vacuously true. These red-proof checks travel with the
  // extractor itself (not a sibling test) so that running any single test
  // alone still cannot pass on an empty/broken extraction.
  assert.ok(names.length >= 15, `extracted only ${names.length} interactive-only names — the extraction is broken, not the registry`);
  for (const anchor of ['theme', 'rewind', 'tasks']) {
    assert.ok(names.includes(anchor), `anchor "${anchor}" missing — the extraction is reading the wrong block`);
  }
  return names;
}

function builtinCommandNames(): string[] {
  const source = readFileSync(NAMES_RS, 'utf8');
  const decl = source.indexOf('pub const BUILTIN_COMMAND_NAMES');
  assert.notEqual(decl, -1, 'BUILTIN_COMMAND_NAMES is gone — this gate is reading the wrong file');
  const open = source.indexOf('&[', source.indexOf('=', decl));
  const close = source.indexOf('];', open);
  assert.notEqual(close, -1, 'BUILTIN_COMMAND_NAMES is unterminated');
  return source
    .slice(open, close)
    .split('\n')
    .filter((line) => !line.trim().startsWith('//'))
    .flatMap((line) => [...line.matchAll(/"([a-z0-9-]+)"/g)].map((match) => match[1]!));
}

/**
 * Interactive-only names this sub-project deliberately does NOT handle, each
 * with where it goes. Deleting an entry without adding a desktop command turns
 * the gate red, which is the point.
 */
const DEFERRED: Record<string, string> = {
  background: 'not applicable to a GUI client (terminal detach)',
  branch: 'sub-project 4',
  'add-dir': 'sub-project 4',
  cd: 'sub-project 4',
  color: 'not applicable to a GUI client (terminal palette)',
  copy: 'sub-project 4',
  diff: 'sub-project 3',
  focus: 'not applicable to a GUI client (terminal renderer)',
  plan: 'sub-project 2',
  plugin: 'sub-project 4',
  'privacy-settings': 'sub-project 3',
  rename: 'sub-project 2',
  rewind: 'sub-project 2',
  tasks: 'sub-project 3',
  'terminal-setup': 'not applicable to a GUI client (terminal setup)',
  tui: 'not applicable to a GUI client (terminal renderer)',
  usage: 'sub-project 3',
  'usage-credits': 'sub-project 3',
};

test('the extraction actually reads the engine registry', () => {
  // interactiveOnlyNames() carries its own red-proof assertions now (length
  // + anchors), so calling it here still catches a broken extraction; this
  // test additionally proves the builtin-table reader works.
  interactiveOnlyNames();
  const builtins = builtinCommandNames();
  assert.equal(builtins.length, 86, `expected the 86-name builtin table, got ${builtins.length}`);
});

test('every interactive-only engine command is handled by the desktop or explicitly deferred', () => {
  const handled = new Set(DESKTOP_COMMANDS.map((command) => command.name));
  const unaccounted = interactiveOnlyNames()
    .filter((name) => !handled.has(name) && !(name in DEFERRED));

  assert.deepEqual(
    unaccounted,
    [],
    `these engine commands answer "interactive TUI mode only" on a desktop that could handle them: ${unaccounted.join(', ')}. Add a desktop command or an entry in DEFERRED.`,
  );
});

test('no desktop command targets a name the engine does not have', () => {
  const builtins = new Set(builtinCommandNames());
  const unknown = DESKTOP_COMMANDS.map((c) => c.name).filter((name) => !builtins.has(name));

  assert.deepEqual(unknown, [], `desktop commands with no engine counterpart (typo?): ${unknown.join(', ')}`);
});
