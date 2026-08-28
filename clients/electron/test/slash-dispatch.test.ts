import { test } from 'node:test';
import assert from 'node:assert/strict';

import { parseSlashLine, resolveDesktopCommand, type DesktopCommand } from '../src/renderer/bridge/slashDispatch';
import { DESKTOP_COMMANDS } from '../src/renderer/bridge/desktopCommands';

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

function recordingContext() {
  const calls: string[] = [];
  const ctx = {
    setModel: async (m: string) => { calls.push(`setModel:${m}`); },
    knownModel: (m: string) => m === 'opus',
    setPermissionMode: async (m: string) => { calls.push(`setPermissionMode:${m}`); },
    setReasoningLevel: async (l: string) => { calls.push(`setReasoningLevel:${l}`); },
    setReasoningAutomatic: async () => { calls.push('setReasoningAutomatic'); },
    setReasoningDisabled: async () => { calls.push('setReasoningDisabled'); },
    setFastMode: async (e: boolean) => { calls.push(`setFastMode:${e}`); },
    fastMode: () => false,
    setTheme: (t: string) => { calls.push(`setTheme:${t}`); },
    openModelPicker: (s: string) => { calls.push(`openModelPicker:${s}`); },
    openPermissionPicker: () => { calls.push('openPermissionPicker'); },
    openSettings: () => { calls.push('openSettings'); },
    emit: (output: string, isError?: boolean) => { calls.push(`emit:${isError ? 'error' : 'ok'}:${output}`); },
  };
  return { ctx, calls };
}

async function run(raw: string) {
  const { ctx, calls } = recordingContext();
  const resolved = resolveDesktopCommand(raw, DESKTOP_COMMANDS);
  assert.ok(resolved, `${raw} should be handled locally`);
  await resolved.command.run(resolved.args, ctx as never);
  return calls;
}

test('bare selector commands open the surface that actually renders', async () => {
  assert.deepEqual(await run('/model'), ['openModelPicker:model']);
  assert.deepEqual(await run('/effort'), ['openModelPicker:effort']);
  assert.deepEqual(await run('/permissions'), ['openPermissionPicker']);
  assert.deepEqual(await run('/config'), ['openSettings']);
  assert.deepEqual(await run('/theme'), ['openSettings']);
});

test('arguments apply directly', async () => {
  assert.deepEqual(await run('/model opus'), ['setModel:opus']);
  assert.deepEqual(await run('/permissions plan'), ['setPermissionMode:plan']);
  assert.deepEqual(await run('/theme dark'), ['setTheme:dark']);
  assert.deepEqual(await run('/fast on'), ['setFastMode:true']);
  assert.deepEqual(await run('/effort high'), ['setReasoningLevel:high']);
  assert.deepEqual(await run('/effort auto'), ['setReasoningAutomatic']);
});

test('a bad argument reports itself instead of silently doing nothing', async () => {
  assert.deepEqual(await run('/model nope'), ['emit:error:Unknown model: nope']);
  const perms = await run('/permissions nope');
  assert.equal(perms.length, 1);
  assert.match(perms[0]!, /^emit:error:/);
  assert.match(perms[0]!, /bypassPermissions/);
  assert.deepEqual(await run('/config extra'), ['emit:error:/config takes no arguments']);
});

test('bare /fast toggles from the live state', async () => {
  assert.deepEqual(await run('/fast'), ['setFastMode:true']);
});
