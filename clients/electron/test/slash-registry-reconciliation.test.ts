import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { runtimePath } from '../../shared/test/runtimeSource.js';

import {
  ALL_DESKTOP_COMMANDS,
  DESKTOP_COMMANDS,
  DESKTOP_ENGINE_BUILTIN_COMMANDS,
  DESKTOP_UNAVAILABLE_BUILTIN_COMMANDS,
} from '../src/renderer/bridge/desktopCommands';

const REGISTER_RS = runtimePath('crates/commands/core/src/register.rs');
const NAMES_RS = runtimePath('crates/command-api/src/builtin_support/names.rs');

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

test('the extraction actually reads the engine registry', () => {
  // interactiveOnlyNames() carries its own red-proof assertions now (length
  // + anchors), so calling it here still catches a broken extraction; this
  // test additionally proves the builtin-table reader works.
  interactiveOnlyNames();
  const builtins = builtinCommandNames();
  assert.equal(builtins.length, 87, `expected the 87-name builtin table, got ${builtins.length}`);
});

test('every interactive-only engine command has a Desktop execution disposition', () => {
  const handled = new Set(ALL_DESKTOP_COMMANDS.map((command) => command.name));
  const engine = new Set<string>(DESKTOP_ENGINE_BUILTIN_COMMANDS);
  const unaccounted = interactiveOnlyNames()
    .filter((name) => !handled.has(name) && !engine.has(name));

  assert.deepEqual(
    unaccounted,
    [],
    `these engine commands still fall through to the TUI-only handler: ${unaccounted.join(', ')}.`,
  );
});

test('every locked builtin is exactly engine-backed, Desktop-backed, or unavailable', () => {
  const local = new Set(DESKTOP_COMMANDS.map((command) => command.name));
  const engine = new Set<string>(DESKTOP_ENGINE_BUILTIN_COMMANDS);
  const unavailable = new Set(Object.keys(DESKTOP_UNAVAILABLE_BUILTIN_COMMANDS));

  for (const name of builtinCommandNames()) {
    const count = Number(local.has(name)) + Number(engine.has(name)) + Number(unavailable.has(name));
    assert.equal(count, 1, `/${name} must have exactly one Desktop disposition, got ${count}`);
  }
});

test('unavailable compatibility handlers are directly resolvable but never advertised', () => {
  const byName = new Map(ALL_DESKTOP_COMMANDS.map((command) => [command.name, command]));
  for (const name of Object.keys(DESKTOP_UNAVAILABLE_BUILTIN_COMMANDS)) {
    assert.equal(byName.get(name)?.advertised, false, `/${name} must stay out of Desktop menus`);
  }
});

test('no desktop command targets a name the engine does not have', () => {
  const builtins = new Set(builtinCommandNames());
  const unknown = DESKTOP_COMMANDS.map((c) => c.name).filter((name) => !builtins.has(name));

  assert.deepEqual(unknown, [], `desktop commands with no engine counterpart (typo?): ${unknown.join(', ')}`);
});
