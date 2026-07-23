import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  activeSlashCommand,
  filterSlashCommands,
  moveSlashSelectionIndex,
  reconcileSlashSelectionIndex,
  slashCommandText,
  slashNavigationDirection,
} from '../src/renderer/bridge/slashCommands';

const commands = [
  { name: 'model', description: 'Switch the active model', source: 'builtin' },
  { name: 'mcp', description: 'Inspect MCP servers', source: 'builtin' },
  { name: 'review-pr', description: 'Review the current pull request', source: 'project' },
  { name: 'commit', description: 'Create a git commit', source: 'plugin' },
];

test('slash completion opens only for a line-leading command token', () => {
  assert.deepEqual(activeSlashCommand('/', 1), { start: 0, end: 1, query: '' });
  assert.deepEqual(activeSlashCommand('  /mo', 5), { start: 2, end: 5, query: 'mo' });
  assert.deepEqual(activeSlashCommand('hello\n/mcp', 10), { start: 6, end: 10, query: 'mcp' });
  assert.equal(activeSlashCommand('hello /mo', 9), null);
  assert.equal(activeSlashCommand('/model opus', 11), null);
});

test('slash completion ranks names before descriptions and deduplicates catalog rows', () => {
  assert.deepEqual(filterSlashCommands(commands, 'm').map((command) => command.name), ['mcp', 'model', 'commit']);
  assert.deepEqual(filterSlashCommands(commands, 'pull').map((command) => command.name), ['review-pr']);
  assert.deepEqual(
    filterSlashCommands([...commands, { ...commands[0]!, source: 'project' }], '').map((command) => command.name),
    ['commit', 'mcp', 'model', 'review-pr'],
  );
});

test('slash command insertion always has one leading slash', () => {
  assert.equal(slashCommandText('model'), '/model');
  assert.equal(slashCommandText('/model'), '/model');
});

test('the unfiltered palette keeps the complete engine catalog', () => {
  const catalog = Array.from({ length: 75 }, (_, index) => ({
    name: `command-${String(index).padStart(2, '0')}`,
    description: `Command ${index}`,
    source: 'builtin',
  }));
  assert.equal(filterSlashCommands(catalog, '').length, catalog.length);
});

test('arrow navigation survives keyup reconciliation and clamps at both ends', () => {
  assert.equal(moveSlashSelectionIndex(0, 'next', 4), 1);
  assert.equal(reconcileSlashSelectionIndex(1, 'mod', 'mod', 4), 1);
  assert.equal(moveSlashSelectionIndex(1, 'previous', 4), 0);
  assert.equal(moveSlashSelectionIndex(0, 'previous', 4), 0);
  assert.equal(moveSlashSelectionIndex(3, 'next', 4), 3);
  assert.equal(reconcileSlashSelectionIndex(3, 'mod', 'model', 1), 0);
  assert.equal(slashNavigationDirection('ArrowDown'), 'next');
  assert.equal(slashNavigationDirection('Down'), 'next');
  assert.equal(slashNavigationDirection('ArrowUp'), 'previous');
  assert.equal(slashNavigationDirection('Up'), 'previous');
});
