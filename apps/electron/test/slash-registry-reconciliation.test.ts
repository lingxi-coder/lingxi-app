import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { runtimePath } from '../../../packages/bridge-client/test/runtimeSource.js';

import {
  ALL_DESKTOP_COMMANDS,
  DESKTOP_COMMANDS,
  DESKTOP_ENGINE_BUILTIN_COMMANDS,
  DESKTOP_ONLY_COMMANDS,
  DESKTOP_UNAVAILABLE_BUILTIN_COMMANDS,
} from '../src/renderer/bridge/desktopCommands';

const REGISTER_RS = runtimePath('crates/command-api/src/builtins/register.rs');
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

// Source-backed 2.1.295 builtin table, in the order exported by the engine.
// The five names that caused the old 87-row expectation are not builtin rows:
// `commit` and `doctor` are bundled prompt entries; `commit-push-pr`, `files`,
// and `init-verifiers` have no reachable default command-pool producer. See
// `crates/command-api/src/builtin_support/names.rs` and the fixed-source
// bindings in `acceptance/source-alignment-295-r31/native-command-source-order.md`.
const NATIVE_295_BUILTIN_COMMAND_NAMES = [
  'add-dir',
  'advisor',
  'agents',
  'auto-mode-setup',
  'autocompact',
  'autofix-pr',
  'background',
  'branch',
  'brief',
  'btw',
  'bug',
  'cd',
  'chrome',
  'clear',
  'color',
  'compact',
  'config',
  'context',
  'copy',
  'desktop',
  'diff',
  'effort',
  'exit',
  'export',
  'extra-usage',
  'fast',
  'feedback',
  'focus',
  'fork',
  'goal',
  'help',
  'hooks',
  'ide',
  'init',
  'insights',
  'install',
  'install-github-app',
  'install-slack-app',
  'keybindings',
  'login',
  'logout',
  'mcp',
  'memory',
  'mobile',
  'model',
  'output-style',
  'passes',
  'permissions',
  'plan',
  'plugin',
  'powerup',
  'privacy-settings',
  'rate-limit-options',
  'recap',
  'release-notes',
  'reload-plugins',
  'reload-skills',
  'remote-env',
  'rename',
  'resume',
  'rewind',
  'security-review',
  'session',
  'skill-doctor',
  'skills',
  'status',
  'statusline',
  'stickers',
  'stop',
  'subtask',
  'tasks',
  'team-onboarding',
  'teleport',
  'terminal-setup',
  'theme',
  'tui',
  'ultraplan',
  'upgrade',
  'usage',
  'usage-credits',
  'version',
  'voice',
  'workflows',
] as const;

test('the extraction actually reads the engine registry', () => {
  // interactiveOnlyNames() carries its own red-proof assertions now (length
  // + anchors), so calling it here still catches a broken extraction; this
  // test additionally proves the builtin-table reader works.
  interactiveOnlyNames();
  const builtins = builtinCommandNames();
  assert.deepEqual(
    builtins,
    NATIVE_295_BUILTIN_COMMAND_NAMES,
    'the builtin table must match the complete ordered 2.1.295 source-backed set',
  );
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

/** `COMMAND_ALIASES`: every alias the engine resolves, with the builtin it resolves to. */
function engineAliases(): Map<string, string> {
  const source = readFileSync(NAMES_RS, 'utf8');
  const decl = source.indexOf('pub const COMMAND_ALIASES');
  assert.notEqual(decl, -1, 'COMMAND_ALIASES is gone — this gate is reading the wrong file');
  const open = source.indexOf('&[(', decl);
  const close = source.indexOf('];', open);
  assert.notEqual(close, -1, 'COMMAND_ALIASES is unterminated');
  const aliases = new Map<string, string>();
  for (const entry of source.slice(open, close).matchAll(/\("([a-z0-9-]+)",\s*&\[([^\]]*)\]\)/g)) {
    for (const alias of entry[2]!.matchAll(/"([a-z0-9-]+)"/g)) aliases.set(alias[1]!, entry[1]!);
  }
  assert.equal(aliases.get('remote'), 'session', 'the /remote -> /session alias moved — update the Remote Control exception below');
  return aliases;
}

test('no desktop command targets a name the engine does not have, except the documented Desktop-only ones', () => {
  const builtins = new Set(builtinCommandNames());
  const unknown = DESKTOP_COMMANDS.map((c) => c.name)
    .filter((name) => !builtins.has(name) && !(name in DESKTOP_ONLY_COMMANDS));

  assert.deepEqual(unknown, [], `desktop commands with no engine counterpart (typo?): ${unknown.join(', ')}`);
});

test('every Desktop-only exception is real, documented, and collides with nothing in the engine', () => {
  const builtins = new Set(builtinCommandNames());
  const aliases = engineAliases();
  const local = new Map(DESKTOP_COMMANDS.map((command) => [command.name, command]));
  assert.deepEqual(Object.keys(DESKTOP_ONLY_COMMANDS).sort(), ['remote-control']);
  for (const [name, reason] of Object.entries(DESKTOP_ONLY_COMMANDS)) {
    const command = local.get(name);
    assert.ok(command, `/${name} is documented as Desktop-only but is not a Desktop command`);
    assert.ok(reason.length > 20, `/${name} needs a real reason`);
    for (const spelling of [name, ...(command.aliases ?? [])]) {
      assert.equal(builtins.has(spelling), false, `/${spelling} is an engine builtin; it cannot be a Desktop-only spelling`);
      assert.equal(aliases.has(spelling), false, `/${spelling} is an engine alias; it cannot be a Desktop-only spelling`);
    }
  }
});

test('/remote stays the engine alias of /session and is not shadowed by /remote-control or /rc', () => {
  const spellings = ALL_DESKTOP_COMMANDS.flatMap((command) => [command.name, ...(command.aliases ?? [])]);
  assert.equal(spellings.includes('remote'), false);
  assert.ok(spellings.includes('remote-control'));
  assert.ok(spellings.includes('rc'));
});
