import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  activeSlashCommand,
  filterSlashCommands,
  moveSlashSelectionIndex,
  reconcileSlashSelectionIndex,
  renderDesktopSlashHelp,
  slashCommandText,
  slashMenuLabel,
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

const dtoCommands = [
  { name: 'model', description: 'Switch the active model', source: 'builtin' },
  { name: 'usage', description: 'Show usage', source: 'builtin', aliases: ['cost', 'stats'] },
  { name: 'secret', description: 'Hidden helper', source: 'builtin', hidden: true },
  { name: 'compact', description: 'Compact the conversation', source: 'builtin', menu_description: 'Compact', argument_hint: '[instructions]' },
];

test('hidden commands stay out of the bare menu but resolve on an exact name', () => {
  assert.equal(filterSlashCommands(dtoCommands, '').some((c) => c.name === 'secret'), false);
  assert.equal(filterSlashCommands(dtoCommands, 'secret').some((c) => c.name === 'secret'), true);
  // A prefix is not an exact name — still hidden.
  assert.equal(filterSlashCommands(dtoCommands, 'sec').some((c) => c.name === 'secret'), false);
});

test('an alias matches its command', () => {
  assert.deepEqual(filterSlashCommands(dtoCommands, 'cost').map((c) => c.name), ['usage']);
});

test('the menu label prefers menu_description', () => {
  assert.equal(slashMenuLabel(dtoCommands[3]!), 'Compact');
  assert.equal(slashMenuLabel(dtoCommands[0]!), 'Switch the active model');
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

test('Desktop hides unsupported builtins without hiding a project command of the same name', () => {
  const catalog = [
    { name: 'rename', description: 'Unavailable builtin', source: 'builtin' },
    { name: 'rename-project', description: 'Project rename workflow', source: 'project' },
    { name: 'help', description: 'Show help', source: 'builtin' },
  ];
  assert.deepEqual(filterSlashCommands(catalog, '').map((command) => command.name), ['help', 'rename-project']);
});

test('Desktop help is rendered from the same filtered live catalog as completion', () => {
  const help = renderDesktopSlashHelp([
    { name: 'help', description: 'Show help', source: 'builtin' },
    { name: 'tasks', description: 'Open tasks', source: 'builtin' },
    { name: 'tui', description: 'Terminal renderer', source: 'builtin' },
  ]);
  assert.match(help, /^Commands:/);
  assert.match(help, /\/help\s+Show help/);
  assert.match(help, /\/tasks\s+Open tasks/);
  assert.doesNotMatch(help, /\/tui/);
});
