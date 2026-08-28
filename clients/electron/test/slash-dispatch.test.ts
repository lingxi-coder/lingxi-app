import { test } from 'node:test';
import assert from 'node:assert/strict';

import { parseSlashLine, resolveDesktopCommand, type DesktopCommand } from '../src/renderer/bridge/slashDispatch';

const noop = () => undefined;
const table: DesktopCommand[] = [
  { name: 'model', args: 'optional', run: noop },
  { name: 'usage', aliases: ['cost'], args: 'none', run: noop },
  { name: 'rename', args: 'required', run: noop },
];

test('a slash line splits into a name and an untrimmed-tail argument string', () => {
  assert.deepEqual(parseSlashLine('/model'), { name: 'model', args: '' });
  assert.deepEqual(parseSlashLine('/model opus 4'), { name: 'model', args: 'opus 4' });
  assert.deepEqual(parseSlashLine('  /model  opus  '), { name: 'model', args: 'opus' });
  assert.equal(parseSlashLine('hello'), null);
  assert.equal(parseSlashLine('/'), null);
});

test('an alias resolves to its command', () => {
  assert.equal(resolveDesktopCommand('/cost', table)?.command.name, 'usage');
});

test('a required-argument command invoked bare falls through to the engine', () => {
  // ArgSpec::Required in tui/src/command.rs:31 — an empty tail is NOT a local
  // dispatch, so the engine gets its own say.
  assert.equal(resolveDesktopCommand('/rename', table), null);
  assert.equal(resolveDesktopCommand('/rename new title', table)?.command.name, 'rename');
});

test('a command outside the table is not intercepted', () => {
  assert.equal(resolveDesktopCommand('/status', table), null);
});

test('resolution is case-insensitive on the name only', () => {
  assert.equal(resolveDesktopCommand('/MODEL Opus', table)?.args, 'Opus');
});

test('a no-argument command with supplied arguments still resolves, so its run can report the misuse', () => {
  const result = resolveDesktopCommand('/usage extra', table);
  assert.equal(result?.command.name, 'usage');
  assert.equal(result?.args, 'extra');
});
