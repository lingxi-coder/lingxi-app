import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  desktopCommandIsShadowed,
  parseSlashLine,
  resolveDesktopCommand,
  type DesktopCommand,
} from '../src/renderer/bridge/slashDispatch';
import { ALL_DESKTOP_COMMANDS, DESKTOP_COMMANDS } from '../src/renderer/bridge/desktopCommands';
import type { SessionWorkspaceOperation } from '../src/shared/sessionWorkspace';

const noop = () => undefined;
const table: DesktopCommand[] = [
  { name: 'model', args: 'optional', run: noop },
  { name: 'usage', aliases: ['cost'], args: 'none', run: noop },
  { name: 'rename', args: 'required', run: noop },
];

test('a slash line splits into a name and a trimmed-tail argument string', () => {
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

test('a project command shadows a same-named Desktop builtin', () => {
  assert.equal(desktopCommandIsShadowed('/rewind checkpoint', [{
    name: 'rewind',
    description: 'Project rewind workflow',
    source: 'project',
  }]), true);
  assert.equal(desktopCommandIsShadowed('/rewind checkpoint', [{
    name: 'rewind',
    description: 'Builtin rewind',
    source: 'builtin',
  }]), false);
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
  let reasoningSelection: unknown;
  const ctx = {
    setModel: async (m: string) => { calls.push(`setModel:${m}`); },
    knownModel: (m: string) => m === 'opus',
    setPermissionMode: async (m: string) => { calls.push(`setPermissionMode:${m}`); },
    setReasoningLevel: async (l: string) => {
      reasoningSelection = { type: 'level', id: l };
      calls.push(`setReasoningLevel:${l}`);
    },
    setReasoningAutomatic: async () => {
      reasoningSelection = { type: 'automatic' };
      calls.push('setReasoningAutomatic');
    },
    setFastMode: async (e: boolean) => { calls.push(`setFastMode:${e}`); },
    fastMode: () => false,
    setTheme: (t: string) => { calls.push(`setTheme:${t}`); },
    openModelPicker: (s: string) => { calls.push(`openModelPicker:${s}`); },
    openPermissionPicker: () => { calls.push('openPermissionPicker'); },
    openSettings: () => { calls.push('openSettings'); },
    openSettingsPage: (page: string) => { calls.push(`openSettingsPage:${page}`); },
    updateSessionWorkspace: async (operation: SessionWorkspaceOperation) => {
      calls.push(`updateSessionWorkspace:${operation.action}:${operation.path ?? '<picker>'}`);
    },
    resumeSession: (query?: string) => { calls.push(`resumeSession:${query ?? ''}`); },
    clearSession: async (name?: string) => { calls.push(name ? `clearSession:${name}` : 'clearSession'); },
    forceCompact: async (instructions?: string) => { calls.push(instructions ? `forceCompact:${instructions}` : 'forceCompact'); },
    copyLastResponse: async () => { calls.push('copyLastResponse'); return true; },
    login: async () => { calls.push('login'); },
    logout: async () => { calls.push('logout'); },
    reloadPlugins: async () => { calls.push('reloadPlugins'); },
    openTasks: async () => { calls.push('openTasks'); },
    showHelp: () => { calls.push('showHelp'); },
    emit: (output: string, isError?: boolean) => { calls.push(`emit:${isError ? 'error' : 'ok'}:${output}`); },
  };
  return { ctx, calls, reasoningSelection: () => reasoningSelection };
}

async function dispatch(raw: string, ctx: ReturnType<typeof recordingContext>['ctx']) {
  const resolved = resolveDesktopCommand(raw, DESKTOP_COMMANDS);
  assert.ok(resolved, `${raw} should be handled locally`);
  await resolved.command.run(resolved.args, ctx as never);
}

async function run(raw: string) {
  const { ctx, calls } = recordingContext();
  await dispatch(raw, ctx);
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
  assert.deepEqual(await run('/effort unset'), ['setReasoningAutomatic']);
});

test('/effort auto and unset clear a previously selected level through the automatic selection', async () => {
  const { ctx, calls, reasoningSelection } = recordingContext();
  for (const reset of ['auto', 'unset']) {
    await dispatch('/effort high', ctx);
    assert.deepEqual(reasoningSelection(), { type: 'level', id: 'high' });

    await dispatch(`/effort ${reset}`, ctx);
    assert.deepEqual(reasoningSelection(), { type: 'automatic' });
  }
  assert.deepEqual(calls, [
    'setReasoningLevel:high', 'setReasoningAutomatic',
    'setReasoningLevel:high', 'setReasoningAutomatic',
  ]);
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

test('/effort off and removed aliases are rejected', async () => {
  for (const value of ['off', 'automatic', 'disabled']) {
    assert.deepEqual(
      await run(`/effort ${value}`),
      ['emit:error:/effort requires a model-supported effort level'],
    );
  }
});

test('/fast off turns fast mode off explicitly, not just toggling it', async () => {
  assert.deepEqual(await run('/fast off'), ['setFastMode:false']);
});

test('/theme with an unrecognized argument reports itself instead of silently doing nothing', async () => {
  assert.deepEqual(await run('/theme sepia'), ['emit:error:/theme takes dark or light, not: sepia']);
});

test('Desktop-owned slash commands call real GUI and IPC actions', async () => {
  assert.deepEqual(await run('/help'), ['showHelp']);
  assert.deepEqual(await run('/clear'), ['clearSession']);
  assert.deepEqual(await run('/clear old investigation'), ['clearSession:old investigation']);
  assert.deepEqual(await run('/reset archived context'), ['clearSession:archived context']);
  assert.deepEqual(await run('/compact'), ['forceCompact']);
  assert.deepEqual(await run('/compact focus on the API changes'), ['forceCompact:focus on the API changes']);
  assert.deepEqual(await run('/login'), ['login']);
  assert.deepEqual(await run('/logout'), ['logout']);
  assert.deepEqual(await run('/add-dir /tmp/work'), [
    'updateSessionWorkspace:add-dir:/tmp/work',
  ]);
  assert.deepEqual(await run('/add-dir'), ['updateSessionWorkspace:add-dir:<picker>']);
  assert.deepEqual(await run('/cd'), ['updateSessionWorkspace:cd:<picker>']);
  assert.deepEqual(await run('/cd /tmp/work'), ['updateSessionWorkspace:cd:/tmp/work']);
  assert.deepEqual(await run('/resume'), ['resumeSession:']);
  assert.deepEqual(await run('/resume parser investigation'), ['resumeSession:parser investigation']);
  assert.deepEqual(await run('/copy'), [
    'copyLastResponse',
    'emit:ok:Copied the last response to the clipboard.',
  ]);
  assert.deepEqual(await run('/tasks'), ['openTasks']);
  assert.deepEqual(await run('/plugin'), ['openSettingsPage:plugins']);
  assert.deepEqual(await run('/reload-plugins'), ['reloadPlugins']);
});

test('workspace failures reach the composer without an invented success result', async () => {
  for (const raw of ['/cd /missing', '/add-dir /missing']) {
    const { ctx, calls } = recordingContext();
    ctx.updateSessionWorkspace = async () => { throw new Error('Directory does not exist.'); };
    await assert.rejects(() => dispatch(raw, ctx), /Directory does not exist/);
    assert.deepEqual(calls, []);
  }
});

test('project and plugin resume commands keep precedence over the Desktop picker', () => {
  assert.ok(resolveDesktopCommand('/resume search query', DESKTOP_COMMANDS));
  assert.equal(desktopCommandIsShadowed('/resume search query', [{
    name: 'resume', description: 'Project resume workflow', source: 'project',
  }]), true);
  assert.equal(desktopCommandIsShadowed('/resume search query', [{
    name: 'resume', description: 'Builtin resume', source: 'builtin',
  }]), false);
});

test('a directly typed unsupported command reports Desktop unavailability locally', async () => {
  const { ctx, calls } = recordingContext();
  const resolved = resolveDesktopCommand('/rewind now', ALL_DESKTOP_COMMANDS);
  assert.ok(resolved);
  await resolved.command.run(resolved.args, ctx as never);
  assert.equal(calls.length, 1);
  assert.match(calls[0]!, /^emit:error:\/rewind is not available in Desktop:/);
});
