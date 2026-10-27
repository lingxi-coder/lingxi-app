import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdirSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

import { MANAGED_FILE, applyManagedSettings, pathRule, writeAllBotSettings, writeBotSettings } from '../src/main/bots/botSettings.js';
import { projectDirName } from '../src/main/bots/projectDirName.js';
import { routerToolRules } from '../src/main/bots/routerTools.js';
import type { BotCreateInput, BotRecord } from '../src/main/bots/store.js';
import { botFixture } from './bot-helpers.js';

type Fixture = ReturnType<typeof botFixture>;

/** A Bot made the way the service makes one: its settings are written before its record. */
function makeBot(f: Fixture, input: BotCreateInput): BotRecord {
  return f.store.create(input, (record) => { writeBotSettings(f.store, record); });
}

function settingsPath(f: Fixture, bot: BotRecord): string {
  return join(f.store.homeDir(bot.bot_id), 'settings.json');
}

function settingsOf(f: Fixture, bot: BotRecord): any {
  return JSON.parse(readFileSync(settingsPath(f, bot), 'utf8'));
}

const read = (path: string) => pathRule('Read', path);
const edit = (path: string, directory = true) => pathRule('Edit', path, directory);

/** What the engine does to the text between the parentheses before it reads a pattern (rule.rs `unescape_rule_content_units`). */
function unescapeRuleContent(content: string): string {
  let units = content;
  for (const escaped of ['(', ')', '\\']) units = units.split(`\\${escaped}`).join(escaped);
  return units;
}

test('a path becomes a rule that roots it at / and takes in everything below it', () => {
  assert.equal(pathRule('Read', '/Users/me/Library/Application Support/LingXi/bots/abc'), 'Read(//Users/me/Library/Application Support/LingXi/bots/abc/**)');
  assert.equal(pathRule('Edit', '/Users/me/.lingxi.json', false), 'Edit(//Users/me/.lingxi.json)');
  assert.throws(() => pathRule('Read', 'relative/path'), /absolute POSIX path/);
});

test('characters that mean something to a pattern or to the rule syntax are escaped for both', () => {
  const path = '/tmp/a[1]/b (x)*?{c}/back\\slash';
  const rule = pathRule('Edit', path);
  assert.ok(rule.startsWith('Edit(') && rule.endsWith(')'));
  // What the engine hands the glob matcher after it has taken the rule apart.
  const pattern = unescapeRuleContent(rule.slice('Edit('.length, -1));
  assert.equal(pattern, String.raw`//tmp/a\[1\]/b (x)\*\?\{c\}/back\\slash/**`);
  // The parentheses of the rule syntax stay balanced: the first unescaped ( and the last unescaped ) are the outer ones.
  assert.equal(rule, String.raw`Edit(//tmp/a\\[1\\]/b \(x\)\\*\\?\\{c\\}/back\\\\slash/**)`);
  assert.equal(pathRule('Read', '/tmp/ends with space ').endsWith(String.raw`space\\ /**)`), true, 'a space that would end the pattern is protected');
});

test('a Bot may use its bound folders and its own memory, and is walled off from everything private', (t) => {
  const f = botFixture(t);
  const writable = f.folder('writable');
  const readonly = f.folder('readonly');
  const sibling = makeBot(f, { name: 'Sibling' });
  const bot = makeBot(f, { name: 'Scribe', model: 'anthropic/claude-test', workspaces: [{ path: writable, mode: 'write' }, { path: readonly, mode: 'read' }] });
  const settings = settingsOf(f, bot);
  const memory = join(f.store.homeDir(bot.bot_id), 'projects', projectDirName(f.store.workspaceDir(bot.bot_id)), 'memdir');
  assert.equal(settings.model, 'anthropic/claude-test');
  assert.deepEqual(settings.permissions.additionalDirectories, [writable, readonly]);
  assert.deepEqual(settings.permissions.allow, [read(memory), edit(memory), ...routerToolRules()]);
  assert.deepEqual(routerToolRules(), [
    'mcp__bots__list_bots', 'mcp__bots__delegate_task', 'mcp__bots__get_task', 'mcp__bots__message_bot', 'mcp__bots__cancel_task', 'mcp__bots__attach_artifact',
  ], 'the engine names a tool of an MCP server mcp__<server>__<tool>, and each rule names exactly one');

  const isolated = [
    f.store.botDir(sibling.bot_id),
    join(f.userData, 'bridge-runtime'), join(f.userData, 'logs'), join(f.userData, 'settings.v1.json'),
    f.defaultConfigHome, join(f.home, '.lingxi.json'),
  ].sort();
  assert.deepEqual(settings.sandbox.filesystem.denyRead, isolated);
  for (const path of isolated) {
    assert.ok(settings.permissions.deny.includes(read(path)), `no reading ${path}`);
    assert.ok(settings.permissions.deny.includes(edit(path)), `no changing ${path}`);
  }
  assert.ok(!settings.permissions.deny.includes(read(join(f.userData, 'bots'))), 'the Bots folder as a whole is not denied: the Bot lives in it');
  for (const own of [f.store.botDir(bot.bot_id), f.store.homeDir(bot.bot_id), f.store.workspaceDir(bot.bot_id), memory]) {
    assert.ok(!settings.permissions.deny.includes(read(own)), `its own ${own} stays readable`);
  }

  assert.ok(settings.permissions.deny.includes(edit(readonly)), 'no changing a folder bound read-only');
  assert.ok(!settings.permissions.deny.includes(edit(writable)));
  assert.ok(!settings.permissions.deny.includes(read(readonly)), 'but it can be read');

  for (const file of [
    join(f.store.botDir(bot.bot_id), 'bot.json'),
    join(f.store.homeDir(bot.bot_id), 'LINGXI.md'), join(f.store.homeDir(bot.bot_id), 'settings.json'),
    join(f.store.homeDir(bot.bot_id), 'settings.local.json'), join(f.store.homeDir(bot.bot_id), '.lingxi.json'), join(f.store.homeDir(bot.bot_id), MANAGED_FILE),
    join(f.store.workspaceDir(bot.bot_id), '.lingxi', 'settings.json'), join(f.store.workspaceDir(bot.bot_id), '.lingxi', 'settings.local.json'),
    join(f.store.workspaceDir(bot.bot_id), '.mcp.json'),
  ]) assert.ok(settings.permissions.deny.includes(edit(file, false)), `no changing ${file}`);
  assert.ok(settings.permissions.deny.includes(edit(f.store.tasksDir(bot.bot_id))), 'no changing the tasks other Bots handed it');

  assert.equal(settings.sandbox.enabled, true);
  assert.equal(settings.sandbox.failIfUnavailable, true);
});

test('a fresh file lists model, permissions and sandbox, and without a model or folders has neither', (t) => {
  const f = botFixture(t);
  const bare = makeBot(f, { name: 'Bare' });
  const settings = settingsOf(f, bare);
  assert.deepEqual(Object.keys(settings), ['permissions', 'sandbox']);
  assert.ok(!('additionalDirectories' in settings.permissions));
  assert.deepEqual(Object.keys(settings.permissions), ['allow', 'deny']);
  assert.deepEqual(Object.keys(settings.sandbox), ['enabled', 'failIfUnavailable', 'filesystem']);
  const full = makeBot(f, { name: 'Full', model: 'a/b', workspaces: [{ path: f.folder('one'), mode: 'write' }] });
  assert.deepEqual(Object.keys(settingsOf(f, full)), ['model', 'permissions', 'sandbox']);
});

test('the sibling Bots are walled off from each other, and a new or a removed Bot changes every other Bot', (t) => {
  const f = botFixture(t);
  const warnings: string[] = [];
  const first = makeBot(f, { name: 'First' });
  assert.ok(!settingsOf(f, first).permissions.deny.some((rule: string) => rule.includes('Bot0000000000000000002')));
  const second = makeBot(f, { name: 'Second' });
  writeAllBotSettings(f.store, (message) => warnings.push(message), second.bot_id);
  assert.ok(settingsOf(f, first).permissions.deny.includes(read(f.store.botDir(second.bot_id))), 'the first now keeps out of the second');
  assert.ok(settingsOf(f, second).permissions.deny.includes(read(f.store.botDir(first.bot_id))), 'and the second out of the first');
  assert.ok(!settingsOf(f, second).permissions.deny.includes(read(f.store.botDir(second.bot_id))));
  f.store.trash(second.bot_id);
  writeAllBotSettings(f.store, (message) => warnings.push(message));
  const after = settingsOf(f, first);
  assert.ok(!after.permissions.deny.some((rule: string) => rule.includes(second.bot_id)), 'nothing remains of the removed Bot');
  assert.ok(after.permissions.deny.includes(read(f.store.locations.trashRoot)), 'its trash folder is now one more private place');
  assert.deepEqual(warnings, []);
});

test('regenerating across Bots reports the ones it cannot do and does the rest', (t) => {
  const f = botFixture(t);
  const good = makeBot(f, { name: 'Good' });
  const bad = makeBot(f, { name: 'Bad' });
  writeFileSync(settingsPath(f, bad), '{not json');
  const damaged = makeBot(f, { name: 'Damaged' });
  writeFileSync(f.store.recordPath(damaged.bot_id), 'garbage');
  const warnings: string[] = [];
  mkdirSync(join(f.userData, 'Cookies'));
  writeAllBotSettings(f.store, (message) => warnings.push(message));
  assert.ok(settingsOf(f, good).permissions.deny.includes(read(join(f.userData, 'Cookies'))), 'the sound Bot is done');
  assert.equal(readFileSync(settingsPath(f, bad), 'utf8'), '{not json', 'the damaged file is left alone');
  assert.equal(warnings.length, 2);
  assert.match(warnings.join('\n'), new RegExp(`Bot ${bad.bot_id}: settings were not updated: .*not valid JSON`));
  assert.match(warnings.join('\n'), new RegExp(`Bot ${damaged.bot_id} has a damaged record`));
});

test('the private places are read again each time, so a new entry of the app data is walled off at the next launch', (t) => {
  const f = botFixture(t);
  const bot = makeBot(f, { name: 'Fresh' });
  assert.ok(!settingsOf(f, bot).sandbox.filesystem.denyRead.includes(join(f.userData, 'Session Storage')));
  mkdirSync(join(f.userData, 'Session Storage'));
  writeBotSettings(f.store, bot);
  assert.ok(settingsOf(f, bot).sandbox.filesystem.denyRead.includes(join(f.userData, 'Session Storage')));
  rmSync(join(f.userData, 'Session Storage'), { recursive: true });
  writeBotSettings(f.store, bot);
  assert.ok(!settingsOf(f, bot).sandbox.filesystem.denyRead.includes(join(f.userData, 'Session Storage')), 'one that is gone is not kept');
});

test('regenerating twice changes nothing, not even the file', (t) => {
  const f = botFixture(t);
  const bot = makeBot(f, { name: 'Idempotent', model: 'a/b', workspaces: [{ path: f.folder('one'), mode: 'read' }] });
  const files = [settingsPath(f, bot), join(f.store.homeDir(bot.bot_id), MANAGED_FILE)];
  const before = files.map((file) => [readFileSync(file, 'utf8'), statSync(file).ino]);
  writeBotSettings(f.store, bot);
  writeBotSettings(f.store, bot);
  assert.deepEqual(files.map((file) => [readFileSync(file, 'utf8'), statSync(file).ino]), before);
});

test('what was put in the file by someone else stays, in place, once, however often it is regenerated', (t) => {
  const f = botFixture(t);
  const one = f.folder('one');
  const bot = f.store.create({ name: 'Mixed', workspaces: [{ path: one, mode: 'read' }] });
  const foreign = {
    theme: 'dark',
    env: { FOO: '1' },
    hooks: { PreToolUse: [{ matcher: 'Bash', hooks: [{ type: 'command', command: 'true' }] }] },
    permissions: { defaultMode: 'plan', allow: ['Bash(ls:*)'], deny: ['Read(//somewhere/secret/**)'], additionalDirectories: ['/extra'] },
    sandbox: { network: { allowLocalBinding: true }, filesystem: { denyRead: ['/extra-secret'] } },
  };
  writeFileSync(settingsPath(f, bot), JSON.stringify(foreign, null, 2));
  for (let round = 0; round < 3; round += 1) writeBotSettings(f.store, bot);
  const settings = settingsOf(f, bot);
  assert.equal(settings.theme, 'dark');
  assert.deepEqual(settings.env, { FOO: '1' });
  assert.deepEqual(settings.hooks, foreign.hooks);
  assert.equal(settings.permissions.defaultMode, 'plan');
  assert.deepEqual(settings.sandbox.network, { allowLocalBinding: true });
  assert.deepEqual(settings.permissions.allow.slice(0, 1), ['Bash(ls:*)']);
  assert.deepEqual(settings.permissions.deny.slice(0, 1), ['Read(//somewhere/secret/**)']);
  assert.deepEqual(settings.permissions.additionalDirectories, ['/extra', one]);
  assert.deepEqual(settings.sandbox.filesystem.denyRead.slice(0, 1), ['/extra-secret']);
  for (const list of [settings.permissions.allow, settings.permissions.deny, settings.permissions.additionalDirectories, settings.sandbox.filesystem.denyRead]) {
    assert.equal(new Set(list).size, list.length, 'nothing is listed twice');
  }
  assert.deepEqual(Object.keys(settings).slice(0, 4), ['theme', 'env', 'hooks', 'permissions'], 'the file keeps its order');
});

test('a change to the Bot takes out what it had put in and nothing else', (t) => {
  const f = botFixture(t);
  const keep = f.folder('keep');
  const drop = f.folder('drop');
  const bot = f.store.create({ name: 'Changing', model: 'old/model', workspaces: [{ path: keep, mode: 'write' }, { path: drop, mode: 'read' }] });
  writeFileSync(settingsPath(f, bot), JSON.stringify({ permissions: { deny: ['Read(//mine/**)'], additionalDirectories: ['/mine'] }, model: 'someone/elses' }));
  writeBotSettings(f.store, bot);
  assert.equal(settingsOf(f, bot).model, 'old/model', 'the Bot\'s model wins while it has one');
  const changed = f.store.update(bot.bot_id, 1, { workspaces: [{ path: keep, mode: 'read' }], model: null });
  writeBotSettings(f.store, changed);
  const settings = settingsOf(f, bot);
  assert.deepEqual(settings.permissions.additionalDirectories, ['/mine', keep]);
  assert.ok(settings.permissions.deny.includes('Read(//mine/**)'));
  assert.ok(settings.permissions.deny.includes(edit(keep)), 'now read-only');
  assert.ok(!settings.permissions.deny.includes(edit(drop)), 'and the one that was dropped is not mentioned');
  assert.ok(!('model' in settings), 'the model it set is removed');
  const unbound = f.store.update(bot.bot_id, 2, { workspaces: [] });
  writeBotSettings(f.store, unbound);
  assert.deepEqual(settingsOf(f, bot).permissions.additionalDirectories, ['/mine']);
  assert.ok(!settingsOf(f, bot).permissions.deny.includes(edit(keep)));
});

test('a model that someone else set stays when the Bot never had one', (t) => {
  const f = botFixture(t);
  const bot = f.store.create({ name: 'No model' });
  writeFileSync(settingsPath(f, bot), JSON.stringify({ model: 'someone/elses' }));
  writeBotSettings(f.store, bot);
  assert.equal(settingsOf(f, bot).model, 'someone/elses');
});

test('the sandbox is switched on and required, whatever the file said', (t) => {
  const f = botFixture(t);
  const bot = f.store.create({ name: 'Sandboxed' });
  writeFileSync(settingsPath(f, bot), JSON.stringify({ sandbox: { enabled: false, failIfUnavailable: false, excludedCommands: ['git'] } }));
  writeBotSettings(f.store, bot);
  const sandbox = settingsOf(f, bot).sandbox;
  assert.equal(sandbox.enabled, true);
  assert.equal(sandbox.failIfUnavailable, true);
  assert.deepEqual(sandbox.excludedCommands, ['git']);
});

test('a file that is not what the engine needs is reported and left as it was', (t) => {
  const f = botFixture(t);
  const bot = makeBot(f, { name: 'Unreadable' });
  const managed = join(f.store.homeDir(bot.bot_id), MANAGED_FILE);
  const managedBefore = readFileSync(managed, 'utf8');
  for (const [text, message] of [
    ['{not json', /not valid JSON/],
    ['[]', /must be an object/],
    ['"text"', /must be an object/],
    ['{"permissions":[]}', /permissions must be an object/],
    ['{"permissions":{"deny":"Read(//x)"}}', /permissions\.deny must be a list of strings/],
    ['{"permissions":{"allow":[1]}}', /permissions\.allow must be a list of strings/],
    ['{"sandbox":{"filesystem":{"denyRead":{}}}}', /sandbox\.filesystem\.denyRead must be a list of strings/],
  ] as const) {
    writeFileSync(settingsPath(f, bot), text);
    assert.throws(() => writeBotSettings(f.store, bot), message, text);
    assert.equal(readFileSync(settingsPath(f, bot), 'utf8'), text, 'the file is not touched');
    assert.equal(readFileSync(managed, 'utf8'), managedBefore, 'neither is the list of what was written');
  }
  writeFileSync(settingsPath(f, bot), '   \n');
  writeBotSettings(f.store, bot);
  assert.equal(settingsOf(f, bot).sandbox.enabled, true, 'an empty file is as good as none');
});

test('a list of what was written that cannot be read stops the regeneration instead of guessing', (t) => {
  const f = botFixture(t);
  const bot = makeBot(f, { name: 'Guessing' });
  const managed = join(f.store.homeDir(bot.bot_id), MANAGED_FILE);
  const before = readFileSync(settingsPath(f, bot), 'utf8');
  for (const text of ['{broken', '{"v":2}', JSON.stringify({ v: 1, additionalDirectories: [], allow: [], deny: [], denyRead: [], extra: 1 }), JSON.stringify({ v: 1, allow: [] })]) {
    writeFileSync(managed, text);
    assert.throws(() => writeBotSettings(f.store, bot), new RegExp(`${MANAGED_FILE.replace('.', '\\.')} is unreadable`));
    assert.equal(readFileSync(settingsPath(f, bot), 'utf8'), before);
  }
});

test('a folder that is gone, or has become a link, is no longer granted', (t) => {
  const f = botFixture(t);
  const gone = f.folder('gone');
  const swapped = f.folder('swapped');
  const fine = f.folder('fine');
  const bot = f.store.create({ name: 'Drifting', workspaces: [{ path: gone, mode: 'write' }, { path: swapped, mode: 'read' }, { path: fine, mode: 'write' }] });
  assert.deepEqual(writeBotSettings(f.store, bot).skipped, []);
  rmSync(gone, { recursive: true });
  rmSync(swapped, { recursive: true });
  symlinkSync(fine, swapped);
  const result = writeBotSettings(f.store, bot);
  assert.deepEqual(result.bound.map((workspace) => workspace.path), [fine]);
  assert.deepEqual(result.skipped.map((workspace) => workspace.path), [gone, swapped]);
  const settings = settingsOf(f, bot);
  assert.deepEqual(settings.permissions.additionalDirectories, [fine]);
  assert.ok(!settings.permissions.deny.includes(edit(swapped)));
});

test('a folder that has moved somewhere a Bot may not work is no longer granted either', (t) => {
  const f = botFixture(t);
  const folder = f.folder('moves');
  const bot = f.store.create({ name: 'Moved', workspaces: [{ path: folder, mode: 'write' }] });
  rmSync(folder, { recursive: true });
  symlinkSync(f.home, folder);
  assert.deepEqual(writeBotSettings(f.store, bot).bound, []);
});

test('merging only ever touches the copy it was given', () => {
  const existing = Object.freeze({ permissions: Object.freeze({ deny: Object.freeze(['x']) }) }) as Record<string, unknown>;
  const merged = applyManagedSettings(existing, undefined, { additionalDirectories: [], allow: [], deny: ['y'], denyRead: [] });
  assert.deepEqual((merged['permissions'] as any).deny, ['x', 'y']);
  assert.deepEqual((existing['permissions'] as any).deny, ['x']);
});

// The permission mode a Bot's sessions start in

test('the mode of a Bot is written where the engine reads the starting mode of the user settings', (t) => {
  const f = botFixture(t);
  const bot = makeBot(f, { name: 'Careful', permissionMode: 'plan' });
  assert.equal(settingsOf(f, bot).permissions.defaultMode, 'plan');
  assert.equal(JSON.parse(readFileSync(join(f.store.homeDir(bot.bot_id), MANAGED_FILE), 'utf8')).defaultMode, 'plan');
  for (const mode of ['default', 'acceptEdits', 'auto', 'dontAsk', 'bypassPermissions'] as const) {
    const other = makeBot(f, { name: `Mode ${mode}`, permissionMode: mode });
    assert.equal(settingsOf(f, other).permissions.defaultMode, mode);
  }
});

test('without a mode of its own a Bot starts in the engine default, not in the mode the owner last chose', (t) => {
  const f = botFixture(t);
  const bare = makeBot(f, { name: 'No mode' });
  assert.ok(!('defaultMode' in settingsOf(f, bare).permissions));
  assert.ok(!('defaultMode' in JSON.parse(readFileSync(join(f.store.homeDir(bare.bot_id), MANAGED_FILE), 'utf8'))));
  // The owner's own settings are never read into a Bot.
  writeFileSync(join(f.defaultConfigHome, 'settings.json'), JSON.stringify({ permissions: { defaultMode: 'bypassPermissions' } }));
  writeBotSettings(f.store, bare);
  assert.ok(!('defaultMode' in settingsOf(f, bare).permissions));
});

test('changing or clearing the mode removes what the Bot put in, and a mode someone else set stays', (t) => {
  const f = botFixture(t);
  const bot = makeBot(f, { name: 'Changing mode', permissionMode: 'acceptEdits' });
  const changed = f.store.update(bot.bot_id, 1, { permissionMode: 'dontAsk' });
  writeBotSettings(f.store, changed);
  assert.equal(settingsOf(f, bot).permissions.defaultMode, 'dontAsk');
  const cleared = f.store.update(bot.bot_id, 2, { permissionMode: null });
  writeBotSettings(f.store, cleared);
  assert.ok(!('defaultMode' in settingsOf(f, bot).permissions), 'the mode it set is removed');

  const foreign = makeBot(f, { name: 'Foreign mode' });
  const settings = settingsOf(f, foreign);
  settings.permissions.defaultMode = 'plan';
  writeFileSync(settingsPath(f, foreign), JSON.stringify(settings));
  writeBotSettings(f.store, foreign);
  assert.equal(settingsOf(f, foreign).permissions.defaultMode, 'plan', 'a mode the Bot never had is not the Bot\'s to remove');

  const owned = makeBot(f, { name: 'Owned mode', permissionMode: 'auto' });
  const edited = settingsOf(f, owned);
  edited.permissions.defaultMode = 'default';
  writeFileSync(settingsPath(f, owned), JSON.stringify(edited));
  writeBotSettings(f.store, owned);
  assert.equal(settingsOf(f, owned).permissions.defaultMode, 'auto', 'while the Bot has a mode it is the Bot\'s');
  const again = settingsOf(f, owned);
  again.permissions.defaultMode = 'plan';
  writeFileSync(settingsPath(f, owned), JSON.stringify(again));
  writeBotSettings(f.store, f.store.update(owned.bot_id, 1, { permissionMode: null }));
  assert.equal(settingsOf(f, owned).permissions.defaultMode, 'plan', 'and a mode changed by someone else after the Bot wrote its own stays when the Bot lets go');
});

test('regenerating with a mode twice changes nothing, and the list of what was written rejects a mode that is not one', (t) => {
  const f = botFixture(t);
  const bot = makeBot(f, { name: 'Idempotent mode', permissionMode: 'acceptEdits' });
  const before = readFileSync(settingsPath(f, bot), 'utf8');
  const managedPath = join(f.store.homeDir(bot.bot_id), MANAGED_FILE);
  const managedBefore = readFileSync(managedPath, 'utf8');
  writeBotSettings(f.store, bot);
  assert.equal(readFileSync(settingsPath(f, bot), 'utf8'), before);
  assert.equal(readFileSync(managedPath, 'utf8'), managedBefore);
  const managed = JSON.parse(managedBefore);
  for (const bad of ['yolo', 5, null]) {
    writeFileSync(managedPath, JSON.stringify({ ...managed, defaultMode: bad }));
    assert.throws(() => writeBotSettings(f.store, bot), /is unreadable/);
    assert.equal(readFileSync(settingsPath(f, bot), 'utf8'), before);
  }
});

test('a mode that is merged is only ever put into the copy it was given', () => {
  const existing = { permissions: { defaultMode: 'default', allow: [] } };
  const managed = { defaultMode: 'plan' as const, additionalDirectories: [], allow: [], deny: [], denyRead: [] };
  const merged = applyManagedSettings(existing, undefined, managed) as any;
  assert.equal(merged.permissions.defaultMode, 'plan');
  assert.equal(existing.permissions.defaultMode, 'default');
});
