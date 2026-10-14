import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import type { SlashCommandDto } from '@lingxi/bridge-client';
import { slashArgumentHint } from '../src/renderer/bridge/slashArgumentHint';
import { parseSlashArguments } from '../src/renderer/bridge/nativeSlashArgumentParser.js';

const reviewHint = '[low|medium|high|xhigh|max|ultra] [--fix] [--comment] [<pr#>|<branch>|<path>] [--max-findings <n>|all]';
const commands = [
  { name: 'code-review', description: 'Review code', source: 'builtin', argument_hint: reviewHint, aliases: ['review'] },
  { name: 'deploy', description: 'Deploy', source: 'project', argument_hint: '[environment] [version] [region]', argument_names: ['environment', 'version', 'region'], aliases: ['ship'] },
  { name: 'raw', description: 'Raw hint', source: 'project', argument_hint: '  <文件>\t[选项]  ' },
];
const hint = (text: string, cursor = text.length) => slashArgumentHint(text, cursor, commands);

test('native static argument hint requires exactly the first ASCII separator space', () => {
  assert.equal(hint('/code-review '), reviewHint);
  assert.equal(hint('/review '), reviewHint);
  assert.equal(hint('/raw '), '  <文件>\t[选项]  ');
  for (const text of ['/code-review', '/code-review  ', '/code-review high ', '/CODE-REVIEW ', ' /code-review ', '/code-review\t', '/code-review\n ', '/unknown ', 'hello /code-review ']) {
    assert.equal(hint(text), undefined, JSON.stringify(text));
  }
  assert.equal(hint('/code-review ', 1), reviewHint, 'hint stays at the draft end while editing its middle');
  assert.equal(hint('/code-review ', 0), undefined);
});

test('exact command names win over aliases and hidden commands still resolve', () => {
  assert.equal(slashArgumentHint('/review ', 8, [...commands, { name: 'review', description: '', source: 'project', hidden: true, argument_hint: '<path>' }]), '<path>');
});

test('named prompt arguments advance using the native shell parser', () => {
  assert.equal(hint('/deploy '), '[environment] [version] [region]');
  assert.equal(hint('/deploy  '), '[environment] [version] [region]');
  assert.equal(hint('/ship staging '), '[version] [region]');
  assert.equal(hint('/deploy "staging west" v1 '), '[region]');
  assert.equal(hint('/deploy "" '), '[version] [region]');
  assert.equal(hint('/deploy staging v1 us '), undefined);
  assert.equal(hint('/deploy staging v1'), undefined);
  assert.equal(hint('/deploy staging\nv1 '), '[version] [region]');
  assert.equal(hint('/deploy staging $(echo hi) v1 '), '[version] [region]');
});

test('native parser preserves shell recovery, substitution boundaries and first-command behavior', () => {
  const cases: Array<[string, string[]]> = [
    ['', []], ['a ', ['a']], ['"a b" ', ['a b']], ['"" ', ['']], ["'' ", ['']],
    ['a\\ b ', ['a b']], ['"unterminated ', ['"unterminated ']],
    ['a; b ', ['a']], ['a | b ', ['a']], ['a && b ', ['a']], ['a\nb ', ['a']],
    ['A=b foo ', ['foo']], ['a # comment ', ['a']], ['a $(echo hi) b ', ['a']],
  ];
  for (const [input, expected] of cases) assert.deepEqual(parseSlashArguments(input), expected, JSON.stringify(input));
});

test('Desktop matches the installed 2.1.299 oracle for every prompt selector and shell argument vector', () => {
  const oracle = JSON.parse(readFileSync(new URL('../../../packages/bridge-client/test/fixtures/slash-argument-hints-2.1.299.json', import.meta.url), 'utf8')) as {
    commands: Array<{ name: string; aliases?: string[]; displayName?: string; argumentHint?: string; argNames?: string[]; type: string; isHidden?: boolean }>;
    cases: Array<{ input: string; cursor: number; mode: string; expected: string | null }>;
    argumentCases: Array<{ input: string; parsed: string[] }>;
  };
  const catalog: SlashCommandDto[] = oracle.commands.map((command) => ({
    name: command.name, description: '', source: command.type === 'prompt' ? 'project' : 'builtin',
    aliases: [...command.aliases ?? [], ...command.displayName ? [command.displayName] : []],
    argument_hint: command.argumentHint,
    argument_names: command.type === 'prompt' ? command.argNames : undefined,
    hidden: command.isHidden,
  }));
  // Desktop's composer is always in prompt mode; the native bash-mode row is
  // a mode gate outside the pure hint selector.
  for (const row of oracle.cases.filter((entry) => entry.mode === 'prompt')) {
    assert.equal(slashArgumentHint(row.input, row.cursor, catalog) ?? null, row.expected, JSON.stringify(row));
  }
  for (const row of oracle.argumentCases) assert.deepEqual(parseSlashArguments(row.input), row.parsed, JSON.stringify(row.input));
});
