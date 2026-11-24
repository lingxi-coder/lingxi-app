import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, mkdirSync, readFileSync, readdirSync, symlinkSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

import {
  BotConflictError, BotNotFoundError, BotStore, DEFAULT_WORKSPACE_ID, parseAutoReply, parseRecord, parseSandbox, parseScope, upgradeRecord, workspaceIdOf,
} from '../src/main/bots/store.js';
import { projectDirName } from '../src/main/bots/projectDirName.js';
import { BOT_CONFLICT_MESSAGE } from '../src/shared/bots.js';
import { botFixture } from './bot-helpers.js';

const ID = 'Bot0000000000000000001';
const MISSING = 'Bot0000000000000000999';

function recordText(store: BotStore, botId: string): string {
  return readFileSync(store.recordPath(botId), 'utf8');
}

test('a Bot is a folder with its record, a home seeded with the persona, skills and a workspace', (t) => {
  const f = botFixture(t, { ids: true });
  const bot = f.store.create({ name: '  Scribe ', description: 'Writes things down', persona: 'You are Scribe.', model: 'anthropic/claude-test' });
  assert.match(bot.bot_id, /^[A-Za-z0-9_-]{22}$/, 'sixteen random bytes in base64url');
  assert.equal(f.store.botDir(bot.bot_id), join(f.userData, 'bots', bot.bot_id));
  assert.equal(readFileSync(join(f.store.homeDir(bot.bot_id), 'LINGXI.md'), 'utf8'), 'You are Scribe.');
  assert.ok(existsSync(join(f.store.homeDir(bot.bot_id), 'skills')));
  assert.ok(existsSync(f.store.workspaceDir(bot.bot_id)));
  assert.deepEqual(JSON.parse(recordText(f.store, bot.bot_id)), {
    v: 2, bot_id: bot.bot_id, name: 'Scribe', description: 'Writes things down',
    created_at: bot.created_at, updated_at: bot.created_at, revision: 1, model: 'anthropic/claude-test',
    workspaces: [],
    sandbox: { enabled: true, allowedDomains: [] },
    scope: { localApps: [], datasets: [] },
    autoReply: { policy: '', keywords: [], cooldownSec: 0, thresholds: { decline: 0.3, run: 0.75 }, approval: 'never' },
    audiences: [{ id: 'owner', kind: 'owner' }],
  });
  assert.deepEqual(f.store.get(bot.bot_id), bot);
});

test('the Bots are kept outside the default configuration home', (t) => {
  const f = botFixture(t);
  const bot = f.store.create({ name: 'Elsewhere' });
  assert.ok(!f.store.botDir(bot.bot_id).startsWith(f.defaultConfigHome));
  assert.equal(f.store.root, join(f.userData, 'bots'));
  assert.deepEqual(readdirSync(f.defaultConfigHome).sort(), ['settings.json', 'skills']);
});

test('without a persona the home starts with a title and the description', (t) => {
  const f = botFixture(t);
  const plain = f.store.create({ name: 'Plain' });
  const described = f.store.create({ name: 'Described', description: 'Helps.' });
  assert.equal(readFileSync(join(f.store.homeDir(plain.bot_id), 'LINGXI.md'), 'utf8'), '# Plain\n');
  assert.equal(readFileSync(join(f.store.homeDir(described.bot_id), 'LINGXI.md'), 'utf8'), '# Described\n\nHelps.\n');
});

test('the Bot ids that real randomness gives are distinct', (t) => {
  const f = botFixture(t, { ids: true });
  const ids = new Set(Array.from({ length: 8 }, () => f.store.create({ name: 'Same' }).bot_id));
  assert.equal(ids.size, 8);
});

test('an id that is already taken is not reused', (t) => {
  const f = botFixture(t);
  const first = f.store.create({ name: 'First' });
  mkdirSync(join(f.store.root, 'Bot0000000000000000002'));
  const second = f.store.create({ name: 'Second' });
  assert.notEqual(second.bot_id, first.bot_id);
  assert.equal(second.bot_id, 'Bot0000000000000000003');
});

test('the memory folder is where the engine keeps the memory of the Bot workspace', (t) => {
  const f = botFixture(t);
  const bot = f.store.create({ name: 'Remembers' });
  assert.equal(f.store.memoryDir(bot.bot_id), join(f.store.homeDir(bot.bot_id), 'projects', projectDirName(f.store.workspaceDir(bot.bot_id)), 'memdir'));
});

test('list is oldest first, skips a Bot that was never finished, and names a damaged one', (t) => {
  let now = 1000;
  const f = botFixture(t);
  const store = new BotStore({ userData: f.userData, home: f.home, defaultConfigHome: f.defaultConfigHome, defaultWorkspaceRoot: f.defaultWorkspaceRoot, now: () => now, newId: (() => { let n = 100; return () => `Bot${String(++n).padStart(19, '0')}`; })() });
  now = 3000; const later = store.create({ name: 'Later' });
  now = 2000; const earlier = store.create({ name: 'Earlier' });
  mkdirSync(join(store.root, 'Bot0000000000000000999', 'home'), { recursive: true });
  assert.deepEqual(store.list().map((bot) => bot.name), ['Earlier', 'Later']);
  writeFileSync(store.recordPath(later.bot_id), '{broken');
  assert.throws(() => store.list(), new RegExp(`Cannot load Bot ${later.bot_id}`));
  assert.equal(store.get(earlier.bot_id).name, 'Earlier');
  assert.deepEqual(store.entries().map((entry) => [entry.botId, entry.record?.name, entry.error?.message]).filter((entry) => entry[0] !== earlier.bot_id),
    [[later.bot_id, undefined, 'bot.json is not valid JSON']]);
});

test('a Bot that does not exist is not found, and an id that is not an id is refused', (t) => {
  const f = botFixture(t);
  assert.throws(() => f.store.get(MISSING), BotNotFoundError);
  for (const bad of ['', 'short', '../etc', 'a'.repeat(23), 'Bot000000000000000000/', '..'.padEnd(22, '.')]) {
    assert.throws(() => f.store.get(bad), /invalid Bot id/, bad);
  }
});

test('the record is read strictly: one version, exact fields, nothing more', (t) => {
  const f = botFixture(t);
  const bot = f.store.create({ name: 'Strict' });
  const good = JSON.parse(recordText(f.store, bot.bot_id));
  assert.deepEqual(parseRecord(JSON.stringify(good), bot.bot_id), bot);
  const reject = (change: (record: Record<string, any>) => void, message: RegExp) => {
    const record = structuredClone(good);
    change(record);
    assert.throws(() => parseRecord(JSON.stringify(record), bot.bot_id), message);
  };
  reject((record) => { record.v = 1; }, /unsupported version/);
  reject((record) => { record.v = 3; }, /unsupported version/);
  reject((record) => { delete record.v; }, /unsupported version/);
  reject((record) => { record.extra = true; }, /unsupported fields/);
  reject((record) => { delete record.audiences; }, /unsupported fields/);
  for (const key of ['sandbox', 'scope', 'autoReply']) reject((record) => { delete record[key]; }, /unsupported fields/);
  reject((record) => { record.bot_id = 'Bot0000000000000000002'; }, /different Bot/);
  reject((record) => { record.name = ' padded '; }, /invalid name/);
  reject((record) => { record.name = ''; }, /invalid name/);
  reject((record) => { record.description = 'x'.repeat(501); }, /invalid description/);
  reject((record) => { record.revision = 0; }, /invalid revision/);
  reject((record) => { record.revision = 1.5; }, /invalid revision/);
  reject((record) => { record.created_at = -1; }, /invalid created_at/);
  reject((record) => { record.model = ''; }, /invalid model/);
  reject((record) => { record.audiences = []; }, /invalid audiences/);
  reject((record) => { record.audiences = [{ id: 'owner', kind: 'team' }]; }, /invalid audiences/);
  reject((record) => { record.audiences = [{ id: 'owner', kind: 'owner' }, { id: 'x', kind: 'owner' }]; }, /invalid audiences/);
  const folder = { id: 'wAbc', path: '/a', mode: 'read', addedAt: 5 };
  reject((record) => { record.workspaces = [{ ...folder, path: 'relative' }]; }, /invalid workspace path/);
  reject((record) => { record.workspaces = [{ ...folder, mode: 'admin' }]; }, /invalid workspace mode/);
  reject((record) => { record.workspaces = [{ ...folder, extra: 1 }]; }, /unsupported fields/);
  reject((record) => { record.workspaces = [{ path: '/a', mode: 'read' }]; }, /unsupported fields/);
  reject((record) => { record.workspaces = [{ ...folder, id: 'default' }]; }, /invalid workspace id/);
  reject((record) => { record.workspaces = [{ ...folder, id: 'has space' }]; }, /invalid workspace id/);
  reject((record) => { record.workspaces = [{ ...folder, addedAt: -1 }]; }, /invalid workspace time/);
  reject((record) => { record.workspaces = [{ ...folder, label: ' padded ' }]; }, /invalid workspace label/);
  reject((record) => { record.workspaces = [{ ...folder, label: '' }]; }, /invalid workspace label/);
  reject((record) => { record.workspaces = [folder, { ...folder, id: 'wOther' }]; }, /invalid workspaces/);
  reject((record) => { record.workspaces = [folder, { ...folder, path: '/b' }]; }, /invalid workspaces/);
  reject((record) => { record.sandbox = { enabled: 'yes', allowedDomains: [] }; }, /invalid sandbox/);
  reject((record) => { record.sandbox = { enabled: true, allowedDomains: ['not a domain'] }; }, /invalid sandbox domain/);
  reject((record) => { record.sandbox = { enabled: true }; }, /sandbox has unsupported fields/);
  reject((record) => { record.scope = { localApps: ['a b'], datasets: [] }; }, /invalid scope apps/);
  reject((record) => { record.scope = { localApps: [], datasets: [{ source: 's', dataset: 'd', write: 'no', reportReview: false }] }; }, /invalid scope dataset/);
  reject((record) => { record.autoReply.approval = 'sometimes'; }, /invalid reply approval/);
  reject((record) => { record.autoReply.thresholds = { decline: 0.9, run: 0.2 }; }, /invalid reply thresholds/);
  reject((record) => { record.autoReply.cooldownSec = -1; }, /invalid reply cooldown/);
  reject((record) => { record.autoReply.extra = 1; }, /autoReply has unsupported fields/);
  assert.throws(() => parseRecord('[]', bot.bot_id), /invalid bot\.json/);
  assert.throws(() => parseRecord('not json', bot.bot_id), /not valid JSON/);
  writeFileSync(f.store.recordPath(bot.bot_id), ' '.repeat(70_000));
  assert.throws(() => f.store.get(bot.bot_id), /too large/);
});

test('create checks its input before anything is made', (t) => {
  const f = botFixture(t);
  for (const input of [
    { name: '' }, { name: '   ' }, { name: 'x'.repeat(81) }, { name: 'two\nlines' }, { name: 5 as any },
    { name: 'ok', description: 'x'.repeat(501) }, { name: 'ok', description: 3 as any },
    { name: 'ok', persona: 'x'.repeat(64 * 1024 + 1) }, { name: 'ok', persona: 'nul\0' },
    { name: 'ok', model: '' }, { name: 'ok', model: 'x'.repeat(257) },
    { name: 'ok', workspaces: 'nope' as any }, { name: 'ok', workspaces: [{ path: f.folder('a'), mode: 'read', extra: 1 } as any] },
  ]) {
    assert.throws(() => f.store.create(input), /invalid|unsupported/, JSON.stringify(input).slice(0, 80));
  }
  assert.deepEqual(readdirSync(f.store.root), [], 'a refused Bot leaves no folder behind');
});

test('a failure while a Bot is made leaves nothing of it', (t) => {
  const f = botFixture(t);
  assert.throws(() => f.store.create({ name: 'Doomed' }, () => { throw new Error('settings failed'); }), /settings failed/);
  assert.deepEqual(readdirSync(f.store.root), []);
  assert.deepEqual(f.store.list(), []);
});

test('an update takes the revision it was based on and a stale one is a conflict', (t) => {
  let now = 5000;
  const f = botFixture(t);
  const store = new BotStore({ userData: f.userData, home: f.home, defaultConfigHome: f.defaultConfigHome, defaultWorkspaceRoot: f.defaultWorkspaceRoot, now: () => now, newId: () => ID });
  const bot = store.create({ name: 'Original', description: 'one', model: 'a/b' });
  now = 6000;
  const updated = store.update(bot.bot_id, 1, { name: 'Renamed', description: 'two' });
  assert.equal(updated.revision, 2);
  assert.equal(updated.updated_at, 6000);
  assert.equal(updated.created_at, 5000);
  assert.equal(updated.model, 'a/b', 'what the patch does not mention stays');
  assert.deepEqual(store.get(bot.bot_id), updated);
  const before = recordText(store, bot.bot_id);
  assert.throws(() => store.update(bot.bot_id, 1, { name: 'Stale' }), BotConflictError);
  assert.throws(() => store.update(bot.bot_id, 3, { name: 'Ahead' }), BotConflictError);
  assert.equal(recordText(store, bot.bot_id), before, 'a conflict changes nothing');
  assert.equal(store.update(bot.bot_id, 2, { model: null }).model, undefined);
  assert.equal(store.get(bot.bot_id).revision, 3);
  assert.ok(!('model' in JSON.parse(recordText(store, bot.bot_id))));
});

test('an update that changes nothing keeps the revision', (t) => {
  const f = botFixture(t);
  const bot = f.store.create({ name: 'Steady', workspaces: [{ path: f.folder('repo'), mode: 'read' }] });
  const before = recordText(f.store, bot.bot_id);
  assert.equal(f.store.update(bot.bot_id, 1, {}).revision, 1);
  assert.equal(f.store.update(bot.bot_id, 1, { name: 'Steady', workspaces: [{ path: f.folder('repo'), mode: 'read' }] }).revision, 1);
  assert.equal(recordText(f.store, bot.bot_id), before);
});

test('an update is checked like a creation, and a refused one changes nothing', (t) => {
  const f = botFixture(t);
  const bot = f.store.create({ name: 'Checked' });
  const before = recordText(f.store, bot.bot_id);
  for (const patch of [{ name: '' }, { description: 'x'.repeat(501) }, { model: '' }, { workspaces: [{ path: '/nonexistent/elsewhere', mode: 'read' as const }] }]) {
    assert.throws(() => f.store.update(bot.bot_id, 1, patch), /invalid|does not exist/);
  }
  for (const revision of [0, -1, 1.5, NaN, '1' as any]) assert.throws(() => f.store.update(bot.bot_id, revision, { name: 'x' }), /invalid revision/);
  assert.throws(() => f.store.update(MISSING, 1, { name: 'Nobody' }), BotNotFoundError);
  assert.equal(recordText(f.store, bot.bot_id), before);
});

test('a failure while a Bot is updated keeps the record as it was', (t) => {
  const f = botFixture(t);
  const bot = f.store.create({ name: 'Kept' });
  const before = recordText(f.store, bot.bot_id);
  assert.throws(() => f.store.update(bot.bot_id, 1, { name: 'Lost' }, () => { throw new Error('settings failed'); }), /settings failed/);
  assert.equal(recordText(f.store, bot.bot_id), before);
});

test('a deleted Bot goes to the trash folder beside the Bots, whole, and is not purged', (t) => {
  const f = botFixture(t);
  const first = f.store.create({ name: 'Gone' });
  const keep = f.store.create({ name: 'Stays' });
  writeFileSync(join(f.store.homeDir(first.bot_id), 'note.txt'), 'kept');
  const trashed = f.store.trash(first.bot_id);
  assert.equal(join(f.userData, 'bots-trash'), f.store.locations.trashRoot);
  assert.ok(trashed.startsWith(join(f.userData, 'bots-trash', first.bot_id)));
  assert.equal(readFileSync(join(trashed, 'home', 'note.txt'), 'utf8'), 'kept');
  assert.ok(existsSync(join(trashed, 'bot.json')));
  assert.ok(!existsSync(f.store.botDir(first.bot_id)));
  assert.deepEqual(f.store.list().map((bot) => bot.bot_id), [keep.bot_id]);
  assert.throws(() => f.store.get(first.bot_id), BotNotFoundError);
  assert.throws(() => f.store.trash(first.bot_id), BotNotFoundError);
});

test('a damaged Bot can still be deleted, and trashing twice in one millisecond keeps both', (t) => {
  const f = botFixture(t);
  const bot = f.store.create({ name: 'Damaged' });
  writeFileSync(f.store.recordPath(bot.bot_id), 'garbage');
  const first = f.store.trash(bot.bot_id);
  mkdirSync(f.store.botDir(bot.bot_id));
  const second = f.store.trash(bot.bot_id);
  assert.notEqual(first, second);
  assert.equal(readdirSync(f.store.locations.trashRoot).length, 2);
});

test('only the workspace of a Bot that exists belongs to a Bot', (t) => {
  const f = botFixture(t);
  const bot = f.store.create({ name: 'Owner of a workspace' });
  const workspace = f.store.workspaceDir(bot.bot_id);
  assert.equal(f.store.botIdForWorkspace(workspace), bot.bot_id);
  assert.equal(f.store.botIdForWorkspace(`${workspace}/`), bot.bot_id);
  assert.equal(f.store.botIdForWorkspace(f.store.homeDir(bot.bot_id)), undefined);
  assert.equal(f.store.botIdForWorkspace(join(workspace, 'sub')), undefined);
  assert.equal(f.store.botIdForWorkspace(f.store.botDir(bot.bot_id)), undefined);
  assert.equal(f.store.botIdForWorkspace(join(f.store.root, MISSING, 'workspace')), undefined, 'no such Bot');
  mkdirSync(join(f.store.root, MISSING, 'workspace'), { recursive: true });
  assert.equal(f.store.botIdForWorkspace(join(f.store.root, MISSING, 'workspace')), undefined, 'a folder without a record is not a Bot');
  assert.equal(f.store.botIdForWorkspace(join(f.folder('project'))), undefined);
  assert.equal(f.store.botIdForWorkspace('relative/workspace'), undefined);
  f.store.trash(bot.bot_id);
  assert.equal(f.store.botIdForWorkspace(workspace), undefined, 'a deleted Bot no longer has one');
});

test('every other entry of the Bots folder is out of bounds for a Bot', (t) => {
  const f = botFixture(t);
  const one = f.store.create({ name: 'One' });
  const two = f.store.create({ name: 'Two' });
  writeFileSync(join(f.store.root, '.DS_Store'), '');
  assert.deepEqual(f.store.otherEntries(one.bot_id), [join(f.store.root, '.DS_Store'), f.store.botDir(two.bot_id)]);
});

// Workspace bindings

test('a bound folder is kept as its real path', (t) => {
  const f = botFixture(t);
  const real = f.folder('real');
  symlinkSync(real, join(f.root, 'work', 'alias'));
  const bot = f.store.create({ name: 'Bound', workspaces: [{ path: join(f.root, 'work', 'alias'), mode: 'write' }, { path: f.folder('other'), mode: 'read' }] });
  assert.deepEqual(bot.workspaces.map(({ path, mode }) => ({ path, mode })), [{ path: real, mode: 'write' }, { path: f.folder('other'), mode: 'read' }]);
  assert.deepEqual(bot.workspaces.map((workspace) => workspace.id), [workspaceIdOf(real), workspaceIdOf(f.folder('other'))], 'a folder is named by what it is');
  assert.deepEqual(f.store.get(bot.bot_id).workspaces, bot.workspaces);
});

test('a folder is refused unless it is an absolute path to an existing directory', (t) => {
  const f = botFixture(t);
  const file = join(f.root, 'work', 'file.txt');
  mkdirSync(join(f.root, 'work'), { recursive: true });
  writeFileSync(file, 'x');
  for (const path of ['relative/folder', './here', '', join(f.root, 'work', 'missing'), file, `${f.folder('a')}\0`]) {
    assert.throws(() => f.store.workspace(path, 'read'), /invalid workspace path|does not exist or is not a directory/, JSON.stringify(path));
  }
  assert.throws(() => f.store.workspace(f.folder('a'), 'rw'), /invalid workspace mode/);
  assert.throws(() => f.store.workspace(42, 'read'), /invalid workspace path/);
  assert.deepEqual(f.store.workspace(f.folder('a'), 'read'), { path: f.folder('a'), mode: 'read' });
});

test('the filesystem root, the home folder and what contains it are refused', (t) => {
  const f = botFixture(t);
  assert.throws(() => f.store.workspace('/', 'read'), /filesystem root/);
  assert.throws(() => f.store.workspace(f.home, 'write'), /home folder/);
  assert.throws(() => f.store.workspace(f.root, 'read'), /home folder or in a folder that contains it/);
  assert.throws(() => f.store.workspace(join(f.home, '..'), 'read'), /home folder or in a folder that contains it/);
  mkdirSync(join(f.home, 'projects', 'app'), { recursive: true });
  assert.equal(f.store.workspace(join(f.home, 'projects', 'app'), 'write').path, join(f.home, 'projects', 'app'), 'a folder inside the home is fine');
});

test('the app data, the Bots and the configuration home of normal sessions are refused', (t) => {
  const f = botFixture(t);
  const bot = f.store.create({ name: 'Walled' });
  for (const path of [
    f.userData, join(f.userData, 'logs'), f.store.root, f.store.botDir(bot.bot_id), f.store.workspaceDir(bot.bot_id), f.store.homeDir(bot.bot_id),
    join(f.root, 'Application Support'),
  ]) assert.throws(() => f.store.workspace(path, 'write'), /app's own data folder/, path);
  for (const path of [f.defaultConfigHome, join(f.defaultConfigHome, 'skills')]) {
    assert.throws(() => f.store.workspace(path, 'read'), /configuration folder/, path);
  }
  for (const path of [f.defaultWorkspaceRoot, join(f.defaultWorkspaceRoot, 'chat')]) {
    assert.throws(() => f.store.workspace(path, 'write'), /workspace folder/, path);
  }
  // Through a link, too.
  mkdirSync(join(f.root, 'work'), { recursive: true });
  symlinkSync(f.store.root, join(f.root, 'work', 'to-bots'));
  symlinkSync(f.defaultConfigHome, join(f.root, 'work', 'to-config'));
  assert.throws(() => f.store.workspace(join(f.root, 'work', 'to-bots'), 'read'), /app's own data folder/);
  assert.throws(() => f.store.workspace(join(f.root, 'work', 'to-config'), 'read'), /configuration folder/);
  symlinkSync(join(f.defaultWorkspaceRoot, 'chat'), join(f.root, 'work', 'to-chat'));
  assert.throws(() => f.store.workspace(join(f.root, 'work', 'to-chat'), 'read'), /workspace folder/);
});

test('a configuration home outside the home is protected from both sides', (t) => {
  const f = botFixture(t);
  const configHome = join(f.root, 'elsewhere', 'config');
  mkdirSync(configHome, { recursive: true });
  const store = new BotStore({ userData: f.userData, home: f.home, defaultConfigHome: configHome, defaultWorkspaceRoot: f.defaultWorkspaceRoot });
  assert.throws(() => store.workspace(configHome, 'read'), /configuration folder/);
  assert.throws(() => store.workspace(join(f.root, 'elsewhere'), 'read'), /configuration folder/, 'a folder that contains it');
});

test('the same folder cannot be bound twice, not even through a link, and the list is bounded', (t) => {
  const f = botFixture(t);
  const real = f.folder('twice');
  symlinkSync(real, join(f.root, 'work', 'twice-alias'));
  assert.throws(() => f.store.create({ name: 'Twice', workspaces: [{ path: real, mode: 'read' }, { path: join(f.root, 'work', 'twice-alias'), mode: 'write' }] }), /bound twice/);
  const many = Array.from({ length: 17 }, (_, index) => ({ path: f.folder(`many-${index}`), mode: 'read' as const }));
  assert.throws(() => f.store.create({ name: 'Many', workspaces: many }), /invalid workspaces/);
  assert.equal(f.store.create({ name: 'Sixteen', workspaces: many.slice(0, 16) }).workspaces.length, 16);
});

// The permission mode a Bot's sessions start in

const MODES = ['default', 'acceptEdits', 'plan', 'auto', 'dontAsk', 'bypassPermissions'] as const;

test('a Bot can start in any of the permission modes the Desktop offers, and in no other', (t) => {
  const f = botFixture(t);
  for (const mode of MODES) {
    const bot = f.store.create({ name: `In ${mode}`, permissionMode: mode });
    assert.equal(bot.permission_mode, mode);
    assert.equal(JSON.parse(recordText(f.store, bot.bot_id)).permission_mode, mode);
    assert.equal(f.store.get(bot.bot_id).permission_mode, mode);
  }
  for (const bad of ['', 'Default', 'yolo', 'bypass', 5, null, {}, ['plan']]) {
    assert.throws(() => f.store.create({ name: 'Bad', permissionMode: bad as any }), /invalid permission mode/, JSON.stringify(bad));
  }
  assert.equal(f.store.list().length, MODES.length, 'a refused creation leaves no Bot behind');
  const plain = f.store.create({ name: 'Plain' });
  assert.ok(!('permission_mode' in plain) && !('permission_mode' in JSON.parse(recordText(f.store, plain.bot_id))), 'no mode means the key is absent');
});

test('the mode is changed and removed by an update, which is checked first', (t) => {
  const f = botFixture(t);
  const bot = f.store.create({ name: 'Moody' });
  const planned = f.store.update(bot.bot_id, 1, { permissionMode: 'plan' });
  assert.equal(planned.permission_mode, 'plan');
  assert.equal(planned.revision, 2);
  assert.equal(f.store.update(bot.bot_id, 2, { permissionMode: 'plan' }).revision, 2, 'the same mode again changes nothing');
  assert.equal(f.store.update(bot.bot_id, 2, { name: 'Moody too' }).permission_mode, 'plan', 'what the patch does not mention stays');
  const before = recordText(f.store, bot.bot_id);
  for (const bad of ['', 'yolo', 5, {}]) assert.throws(() => f.store.update(bot.bot_id, 3, { permissionMode: bad as any }), /invalid permission mode/);
  assert.equal(recordText(f.store, bot.bot_id), before, 'a refused mode changes nothing');
  const cleared = f.store.update(bot.bot_id, 3, { permissionMode: null });
  assert.equal(cleared.permission_mode, undefined);
  assert.ok(!('permission_mode' in JSON.parse(recordText(f.store, bot.bot_id))));
  assert.equal(f.store.update(bot.bot_id, 4, { permissionMode: null }).revision, 4, 'clearing a mode that is not set changes nothing');
});

test('the record is read strictly where the mode is concerned', (t) => {
  const f = botFixture(t);
  const bot = f.store.create({ name: 'Strict mode', permissionMode: 'acceptEdits' });
  const good = JSON.parse(recordText(f.store, bot.bot_id));
  assert.deepEqual(parseRecord(JSON.stringify(good), bot.bot_id), bot);
  for (const bad of ['', 'yolo', 5, null]) {
    assert.throws(() => parseRecord(JSON.stringify({ ...good, permission_mode: bad }), bot.bot_id), /invalid permission mode/, JSON.stringify(bad));
  }
});

test('a Bot is made with Remote Control off, and the switch is turned on and off by an update, which is checked first', (t) => {
  const f = botFixture(t);
  const bot = f.store.create({ name: 'Reachable' });
  assert.ok(!('remote' in bot) && !('remote' in JSON.parse(recordText(f.store, bot.bot_id))), 'a new Bot is off, and off is an absent key');
  const on = f.store.update(bot.bot_id, 1, { remote: true });
  assert.equal(on.remote, true);
  assert.equal(on.revision, 2);
  assert.equal(JSON.parse(recordText(f.store, bot.bot_id)).remote, true);
  assert.equal(f.store.get(bot.bot_id).remote, true);
  assert.equal(f.store.update(bot.bot_id, 2, { remote: true }).revision, 2, 'the same again changes nothing');
  assert.equal(f.store.update(bot.bot_id, 2, { name: 'Reachable too' }).remote, true, 'what the patch does not mention stays');
  const before = recordText(f.store, bot.bot_id);
  for (const bad of ['true', 'yes', 1, 0, null, {}, []]) {
    assert.throws(() => f.store.update(bot.bot_id, 3, { remote: bad as any }), /invalid remote/, JSON.stringify(bad));
  }
  assert.equal(recordText(f.store, bot.bot_id), before, 'a refused switch changes nothing');
  const off = f.store.update(bot.bot_id, 3, { remote: false });
  assert.equal(off.remote, undefined);
  assert.equal(off.revision, 4);
  assert.ok(!('remote' in JSON.parse(recordText(f.store, bot.bot_id))), 'switching off removes the key');
  assert.equal(f.store.update(bot.bot_id, 4, { remote: false }).revision, 4, 'off when it is off changes nothing');
});

test('the record is read strictly where the switch is concerned', (t) => {
  const f = botFixture(t);
  const bot = f.store.create({ name: 'Strict switch' });
  const good = JSON.parse(recordText(f.store, bot.bot_id));
  assert.equal(parseRecord(JSON.stringify({ ...good, remote: true }), bot.bot_id).remote, true);
  assert.deepEqual(parseRecord(JSON.stringify({ ...good, remote: false }), bot.bot_id), bot, 'false reads as an absent key');
  for (const bad of ['true', 'yes', 1, 0, null, {}, []]) {
    assert.throws(() => parseRecord(JSON.stringify({ ...good, remote: bad }), bot.bot_id), /invalid remote/, JSON.stringify(bad));
  }
  assert.throws(() => parseRecord(JSON.stringify({ ...good, remote: true, shared: true }), bot.bot_id), /unsupported fields/, 'an unknown key is still refused');
});

test('the Bot a transcript belongs to is found by the session id alone, and only a session id is looked for', (t) => {
  const f = botFixture(t);
  const first = f.store.create({ name: 'First' });
  const second = f.store.create({ name: 'Second' });
  const id = '12121212-3434-4567-8899-aaaaaaaaaaaa';
  assert.equal(f.store.botIdOfTranscript(id), undefined, 'no transcript, no Bot');
  mkdirSync(f.store.transcriptsDir(second.bot_id), { recursive: true });
  writeFileSync(join(f.store.transcriptsDir(second.bot_id), `${id}.jsonl`), '');
  assert.equal(f.store.botIdOfTranscript(id), second.bot_id);
  assert.equal(f.store.botIdOfTranscript('12121212-3434-4567-8899-bbbbbbbbbbbb'), undefined, 'another session');
  mkdirSync(join(f.store.root, 'not-a-bot', 'home'), { recursive: true });
  for (const bad of ['', '../x', `../${first.bot_id}/bot`, id.slice(1), `${id}.jsonl`, 'x'.repeat(2000)]) {
    assert.equal(f.store.botIdOfTranscript(bad), undefined, JSON.stringify(bad));
  }
});

test('a conflict says what the window knows it by', (t) => {
  const f = botFixture(t);
  const bot = f.store.create({ name: 'Raced' });
  f.store.update(bot.bot_id, 1, { name: 'Won' });
  assert.throws(() => f.store.update(bot.bot_id, 1, { name: 'Lost' }), (error: Error) => error instanceof BotConflictError && error.message === BOT_CONFLICT_MESSAGE);
});

test('the transcripts of a Bot are in the project folder of its workspace, and its memory is inside it', (t) => {
  const f = botFixture(t);
  const bot = f.store.create({ name: 'Transcribed' });
  assert.equal(f.store.transcriptsDir(bot.bot_id), join(f.store.homeDir(bot.bot_id), 'projects', projectDirName(f.store.workspaceDir(bot.bot_id))));
  assert.equal(f.store.memoryDir(bot.bot_id), join(f.store.transcriptsDir(bot.bot_id), 'memdir'));
});

// Version 2: folders with ids, sandbox, scope and automatic replies

/** What `bot.json` looked like in version 1, the way the app wrote it. */
function v1Record(botId: string, extra: Record<string, unknown> = {}): string {
  return `${JSON.stringify({
    v: 1, bot_id: botId, name: 'Old', description: 'From before', created_at: 1000, updated_at: 2500, revision: 4,
    workspaces: [], audiences: [{ id: 'owner', kind: 'owner' }], ...extra,
  }, null, 2)}\n`;
}

function putV1(f: ReturnType<typeof botFixture>, botId: string, extra: Record<string, unknown> = {}): void {
  mkdirSync(join(f.store.root, botId, 'home'), { recursive: true });
  mkdirSync(join(f.store.root, botId, 'workspace'), { recursive: true });
  writeFileSync(join(f.store.root, botId, 'bot.json'), v1Record(botId, extra));
}

function reopen(f: ReturnType<typeof botFixture>, warn?: (message: string) => void): BotStore {
  return new BotStore({ userData: f.userData, home: f.home, defaultConfigHome: f.defaultConfigHome, defaultWorkspaceRoot: f.defaultWorkspaceRoot, ...(warn ? { warn } : {}) });
}

test('a version 1 record becomes version 2 once, when the store opens, and keeps everything it had', (t) => {
  const f = botFixture(t);
  const repo = f.folder('repo');
  const docs = f.folder('docs');
  putV1(f, ID, { model: 'a/b', permission_mode: 'plan', remote: true, workspaces: [{ path: repo, mode: 'write' }, { path: docs, mode: 'read' }] });
  const store = reopen(f);
  const bot = store.get(ID);
  assert.equal(bot.v, 2);
  assert.deepEqual([bot.name, bot.description, bot.created_at, bot.updated_at, bot.revision, bot.model, bot.permission_mode, bot.remote], ['Old', 'From before', 1000, 2500, 4, 'a/b', 'plan', true],
    'nothing the Bot had is changed, not even its revision or time');
  assert.deepEqual(bot.workspaces, [
    { id: workspaceIdOf(repo), path: repo, mode: 'write', addedAt: 1000 },
    { id: workspaceIdOf(docs), path: docs, mode: 'read', addedAt: 1000 },
  ], 'the folders get their ids and the time the Bot was made');
  assert.deepEqual(bot.sandbox, { enabled: true, allowedDomains: [] });
  assert.deepEqual(bot.scope, { localApps: [], datasets: [] });
  assert.deepEqual(bot.autoReply, { policy: '', keywords: [], cooldownSec: 0, thresholds: { decline: 0.3, run: 0.75 }, approval: 'never' });
  assert.equal(JSON.parse(recordText(store, ID)).v, 2, 'it is on disk as version 2');
  assert.deepEqual(readdirSync(join(store.root, ID)).sort(), ['bot.json', 'home', 'workspace'], 'no temporary file or backup is left');
  const settled = recordText(store, ID);
  assert.deepEqual(reopen(f).get(ID), bot, 'a second opening reads what the first one wrote');
  assert.equal(recordText(store, ID), settled, 'and does not write it again');
});

test('every version 1 record is moved, one that cannot be is named and left alone, and the others are not touched', (t) => {
  const f = botFixture(t);
  const second = 'Bot0000000000000000002';
  const third = 'Bot0000000000000000003';
  const fourth = 'Bot0000000000000000004';
  putV1(f, ID);
  putV1(f, second, { workspaces: [{ path: '/not/absolute/enough', mode: 'admin' }] });
  putV1(f, third, { surprise: true });
  putV1(f, fourth);
  writeFileSync(join(f.store.root, fourth, 'bot.json'), '{broken');
  mkdirSync(join(f.store.root, 'not-a-bot'), { recursive: true });
  const warned: string[] = [];
  const store = reopen(f, (message) => warned.push(message));
  assert.equal(JSON.parse(recordText(store, ID)).v, 2);
  assert.equal(JSON.parse(recordText(store, second)).v, 1, 'a record that does not read stays as it is');
  assert.equal(JSON.parse(recordText(store, third)).v, 1);
  assert.equal(recordText(store, fourth), '{broken');
  assert.equal(warned.length, 2, 'two could not be moved; a damaged record is for list() to name');
  assert.match(warned.join('\n'), new RegExp(`Bot ${second}: bot.json could not be moved to version 2: invalid workspace mode|Bot ${second}: .*workspace`));
  assert.match(warned.join('\n'), new RegExp(`Bot ${third}: .*unsupported fields`));
  assert.throws(() => store.get(second), /unsupported version/, 'and it is not read as version 1 either');
});

test('version 1 is read by one function and nowhere else', (t) => {
  const f = botFixture(t);
  assert.throws(() => parseRecord(v1Record(ID), ID), /unsupported version/);
  assert.equal(upgradeRecord(v1Record(ID), ID)?.v, 2);
  const current = f.store.create({ name: 'Current' });
  assert.equal(upgradeRecord(recordText(f.store, current.bot_id), current.bot_id), undefined, 'a version 2 record is not its business');
  assert.equal(upgradeRecord('{broken', ID), undefined);
  assert.equal(upgradeRecord('[]', ID), undefined);
  assert.throws(() => upgradeRecord(v1Record(ID), 'Bot0000000000000000002'), /different Bot/);
  assert.throws(() => upgradeRecord(v1Record(ID, { sandbox: {} }), ID), /unsupported fields/, 'a version 1 record has no version 2 fields');
  assert.throws(() => upgradeRecord(v1Record(ID, { workspaces: [{ path: '/a', mode: 'read' }, { path: '/a', mode: 'write' }] }), ID), /invalid workspaces/);
});

test('the id of a folder is the same whenever it is bound, and differs between folders', (t) => {
  const f = botFixture(t);
  const one = f.folder('one');
  const two = f.folder('two');
  assert.match(workspaceIdOf(one), /^w[A-Za-z0-9_-]{10}$/);
  assert.equal(workspaceIdOf(one), workspaceIdOf(one));
  assert.notEqual(workspaceIdOf(one), workspaceIdOf(two));
  assert.notEqual(workspaceIdOf(one), DEFAULT_WORKSPACE_ID);
  const bot = f.store.create({ name: 'Rebinds', workspaces: [{ path: one, mode: 'read' }] });
  const without = f.store.update(bot.bot_id, 1, { workspaces: [] });
  const again = f.store.update(bot.bot_id, 2, { workspaces: [{ path: one, mode: 'write' }] });
  assert.equal(again.workspaces[0]!.id, bot.workspaces[0]!.id, 'unbound and bound again, it is the folder it was');
  assert.equal(without.workspaces.length, 0);
});

test('a patch that lists the folders again keeps the id and the time of the ones that were bound', (t) => {
  let now = 100;
  const f = botFixture(t);
  const store = new BotStore({ userData: f.userData, home: f.home, defaultConfigHome: f.defaultConfigHome, defaultWorkspaceRoot: f.defaultWorkspaceRoot, now: () => now, newId: () => ID });
  const one = f.folder('one');
  const two = f.folder('two');
  const bot = store.create({ name: 'Lists', workspaces: [{ path: one, mode: 'read', label: 'First' }] });
  now = 900;
  const next = store.update(bot.bot_id, 1, { workspaces: [{ path: one, mode: 'write', label: 'Primary' }, { path: two, mode: 'read' }] });
  assert.deepEqual(next.workspaces, [
    { id: workspaceIdOf(one), path: one, mode: 'write', label: 'Primary', addedAt: 100 },
    { id: workspaceIdOf(two), path: two, mode: 'read', addedAt: 900 },
  ]);
  assert.equal(store.update(bot.bot_id, 2, { workspaces: next.workspaces.map(({ path, mode, label }) => ({ path, mode, ...(label ? { label } : {}) })) }).revision, 2,
    'the same list again changes nothing');
  assert.throws(() => store.update(bot.bot_id, 2, { workspaces: [{ path: one, mode: 'read', label: 'bad\nlabel' }] }), /invalid workspace label/);
  assert.throws(() => store.update(bot.bot_id, 2, { workspaces: [{ path: one, mode: 'read', extra: 1 } as any] }), /unsupported fields/);
});

test('a folder is added, changed and removed one at a time, whatever revision the Bot is at, and the Bot own folder is not among them', (t) => {
  let now = 100;
  const f = botFixture(t);
  const store = new BotStore({ userData: f.userData, home: f.home, defaultConfigHome: f.defaultConfigHome, defaultWorkspaceRoot: f.defaultWorkspaceRoot, now: () => now, newId: () => ID });
  const one = f.folder('one');
  const two = f.folder('two');
  const bot = store.create({ name: 'Grows' });
  now = 200;
  const added = store.addWorkspace(bot.bot_id, { path: one, mode: 'read', label: 'Repo' });
  assert.deepEqual(added.workspaces, [{ id: workspaceIdOf(one), path: one, mode: 'read', label: 'Repo', addedAt: 200 }]);
  assert.equal(added.revision, 2);
  assert.equal(added.updated_at, 200);
  assert.deepEqual(store.get(bot.bot_id), added);
  assert.throws(() => store.addWorkspace(bot.bot_id, { path: one, mode: 'write' }), /already/, 'one folder, once');
  assert.throws(() => store.addWorkspace(bot.bot_id, { path: join(f.root, 'nowhere'), mode: 'read' }), /does not exist/);
  assert.throws(() => store.addWorkspace(bot.bot_id, { path: two, mode: 'rw' as any }), /invalid workspace mode/);
  assert.throws(() => store.addWorkspace(MISSING, { path: two, mode: 'read' }), BotNotFoundError);
  assert.equal(store.get(bot.bot_id).revision, 2, 'a refused folder changes nothing');
  now = 300;
  const changed = store.changeWorkspace(bot.bot_id, workspaceIdOf(one), { mode: 'write' });
  assert.deepEqual(changed.workspaces[0], { id: workspaceIdOf(one), path: one, mode: 'write', label: 'Repo', addedAt: 200 });
  assert.equal(changed.revision, 3);
  assert.equal(store.changeWorkspace(bot.bot_id, workspaceIdOf(one), { mode: 'write', label: 'Repo' }).revision, 3, 'the same again changes nothing');
  assert.ok(!('label' in store.changeWorkspace(bot.bot_id, workspaceIdOf(one), { label: null }).workspaces[0]!), 'null removes the label');
  assert.equal(store.changeWorkspace(bot.bot_id, workspaceIdOf(one), { label: '  Renamed ' }).workspaces[0]!.label, 'Renamed');
  assert.throws(() => store.changeWorkspace(bot.bot_id, workspaceIdOf(one), { mode: 'admin' as any }), /invalid workspace mode/);
  assert.throws(() => store.changeWorkspace(bot.bot_id, workspaceIdOf(one), { label: '' }), /invalid workspace label/);
  assert.throws(() => store.changeWorkspace(bot.bot_id, 'wNoSuchFold', { mode: 'read' }), /no such folder/);
  assert.throws(() => store.changeWorkspace(bot.bot_id, DEFAULT_WORKSPACE_ID, { mode: 'read' }), /own folder cannot be changed/);
  store.addWorkspace(bot.bot_id, { path: two, mode: 'read' });
  const removed = store.removeWorkspace(bot.bot_id, workspaceIdOf(one));
  assert.deepEqual(removed.workspaces.map((workspace) => workspace.path), [two]);
  assert.throws(() => store.removeWorkspace(bot.bot_id, workspaceIdOf(one)), /no such folder/);
  assert.throws(() => store.removeWorkspace(bot.bot_id, DEFAULT_WORKSPACE_ID), /own folder cannot be removed/);
  assert.deepEqual(store.get(bot.bot_id).workspaces.map((workspace) => workspace.path), [two]);
});

test('a folder change is undone when the settings it needs cannot be written', (t) => {
  const f = botFixture(t);
  const bot = f.store.create({ name: 'Atomic', workspaces: [{ path: f.folder('one'), mode: 'read' }] });
  const before = recordText(f.store, bot.bot_id);
  const fail = () => { throw new Error('settings failed'); };
  assert.throws(() => f.store.addWorkspace(bot.bot_id, { path: f.folder('two'), mode: 'read' }, fail), /settings failed/);
  assert.throws(() => f.store.changeWorkspace(bot.bot_id, bot.workspaces[0]!.id, { mode: 'write' }, fail), /settings failed/);
  assert.throws(() => f.store.removeWorkspace(bot.bot_id, bot.workspaces[0]!.id, fail), /settings failed/);
  assert.equal(recordText(f.store, bot.bot_id), before);
});

test('there is a limit to the folders a Bot is given one by one, too', (t) => {
  const f = botFixture(t);
  const bot = f.store.create({ name: 'Full', workspaces: Array.from({ length: 16 }, (_, index) => ({ path: f.folder(`full-${index}`), mode: 'read' as const })) });
  assert.throws(() => f.store.addWorkspace(bot.bot_id, { path: f.folder('one-more'), mode: 'read' }), /16 folders/);
});

test('the folders of a Bot are its own first and the owner ones after it, and a folder is found by its id', (t) => {
  const f = botFixture(t);
  const one = f.folder('one');
  const bot = f.store.create({ name: 'Several', workspaces: [{ path: one, mode: 'read', label: 'One' }] });
  assert.deepEqual(f.store.allWorkspaces(bot).map((workspace) => [workspace.id, workspace.path, workspace.mode]), [
    [DEFAULT_WORKSPACE_ID, f.store.workspaceDir(bot.bot_id), 'write'],
    [workspaceIdOf(one), one, 'read'],
  ]);
  assert.equal(f.store.workspaceById(bot, DEFAULT_WORKSPACE_ID)?.path, f.store.workspaceDir(bot.bot_id));
  assert.equal(f.store.workspaceById(bot, workspaceIdOf(one))?.label, 'One');
  assert.equal(f.store.workspaceById(bot, 'wNoSuchFold'), undefined);
});

test('the transcripts of every folder of a Bot are in its home, each in the project folder of that folder', (t) => {
  const f = botFixture(t);
  const one = f.folder('one');
  const bot = f.store.create({ name: 'Wide', workspaces: [{ path: one, mode: 'write' }] });
  assert.equal(f.store.transcriptsDirOf(bot.bot_id, one), join(f.store.homeDir(bot.bot_id), 'projects', projectDirName(one)));
  assert.equal(f.store.transcriptsDirOf(bot.bot_id, f.store.workspaceDir(bot.bot_id)), f.store.transcriptsDir(bot.bot_id));
  assert.equal(f.store.memoryDir(bot.bot_id), join(f.store.transcriptsDir(bot.bot_id), 'memdir'), 'the memory stays where it was, whatever folders there are');
});

test('a session of a bound folder is known as the Bot own by what the Bot recorded, and its folder by where its transcript is', (t) => {
  const f = botFixture(t);
  const one = f.folder('one');
  const two = f.folder('two');
  const first = f.store.create({ name: 'First', workspaces: [{ path: one, mode: 'write' }, { path: two, mode: 'read' }] });
  const second = f.store.create({ name: 'Second', workspaces: [{ path: one, mode: 'read' }] });
  const id = '12121212-3434-4567-8899-aaaaaaaaaaaa';
  assert.equal(f.store.botIdOfRecordedSession(id, one), undefined, 'nothing recorded, nothing known');
  const transcript = join(f.store.transcriptsDirOf(first.bot_id, one), `${id}.jsonl`);
  mkdirSync(f.store.transcriptsDirOf(first.bot_id, one), { recursive: true });
  writeFileSync(transcript, '');
  f.store.rememberSessionSource(first.bot_id, id, { path: transcript, mode: 'code' });
  assert.equal(f.store.botIdOfRecordedSession(id, one), first.bot_id);
  assert.equal(f.store.botIdOfRecordedSession(id, two), undefined, 'the folder it ran in is part of the answer');
  assert.equal(f.store.botIdOfRecordedSession(id, f.store.workspaceDir(first.bot_id)), undefined);
  assert.equal(f.store.botIdOfRecordedSession('12121212-3434-4567-8899-bbbbbbbbbbbb', one), undefined, 'another session');
  assert.equal(f.store.botIdOfRecordedSession('not-a-session', one), undefined);
  assert.equal(f.store.botIdOfRecordedSession(id, 'relative'), undefined);
  assert.equal(f.store.botIdOfTranscript(id), first.bot_id, 'the transcript is found in whichever folder it is');
  assert.equal(f.store.workspaceOfTranscript(first, id)?.path, one);
  assert.equal(f.store.workspaceOfTranscript(second, id), undefined, 'a Bot that never ran it has no folder for it');
  f.store.removeWorkspace(first.bot_id, workspaceIdOf(one));
  assert.equal(f.store.workspaceOfTranscript(f.store.get(first.bot_id), id), undefined, 'a folder that was removed is not the folder of a session any more');
  assert.equal(f.store.botIdOfTranscript(id), first.bot_id, 'but its transcript still is the Bot');
  assert.equal(f.store.botIdOfRecordedSession(id, one), first.bot_id, 'and the record of whose it was stays');
});

test('a session that ran in the Bot own folder needs no record to be found', (t) => {
  const f = botFixture(t);
  const bot = f.store.create({ name: 'Home' });
  const id = '12121212-3434-4567-8899-cccccccccccc';
  mkdirSync(f.store.transcriptsDir(bot.bot_id), { recursive: true });
  writeFileSync(join(f.store.transcriptsDir(bot.bot_id), `${id}.jsonl`), '');
  assert.equal(f.store.workspaceOfTranscript(bot, id)?.id, DEFAULT_WORKSPACE_ID);
});

test('the sandbox, scope and automatic replies are kept as given, checked, and changed whole by an update', (t) => {
  const f = botFixture(t);
  const autoReply = {
    policy: 'Answer questions about billing.', keywords: ['refund', 'invoice'], cooldownSec: 60,
    quietHours: { start: '22:00', end: '07:30', timeZone: 'Asia/Shanghai' }, thresholds: { decline: 0.2, run: 0.8 }, approval: 'uncertain' as const,
    decisionProviderId: 'provider-1',
  };
  const scope = { localApps: ['app.one', 'app:two'], datasets: [{ source: 'warehouse', dataset: 'orders', write: false, reportReview: true }] };
  const bot = f.store.create({ name: 'Configured', sandbox: { enabled: false, allowedDomains: ['Example.com', '*.docs.example.com'] }, scope, autoReply });
  assert.deepEqual(bot.sandbox, { enabled: false, allowedDomains: ['example.com', '*.docs.example.com'] }, 'domains are kept lower case');
  assert.deepEqual(bot.scope, scope);
  assert.deepEqual(bot.autoReply, autoReply);
  assert.deepEqual(f.store.get(bot.bot_id), bot, 'all of it comes back from the disk');
  assert.equal(f.store.update(bot.bot_id, 1, { sandbox: { enabled: false, allowedDomains: ['example.com', '*.docs.example.com'] }, scope, autoReply }).revision, 1, 'the same again changes nothing');
  const changed = f.store.update(bot.bot_id, 1, { sandbox: { enabled: true, allowedDomains: [] }, autoReply: { ...autoReply, approval: 'always', quietHours: undefined } as any });
  assert.deepEqual(changed.sandbox, { enabled: true, allowedDomains: [] });
  assert.equal(changed.autoReply.approval, 'always');
  assert.ok(!('quietHours' in changed.autoReply), 'a quiet hours that is left out is removed');
  assert.deepEqual(changed.scope, scope, 'what the patch does not mention stays');
  assert.equal(changed.revision, 2);
});

test('the sandbox, scope and automatic replies are checked before anything is made or changed', (t) => {
  const f = botFixture(t);
  const bot = f.store.create({ name: 'Checked' });
  const before = recordText(f.store, bot.bot_id);
  const reply = (change: Record<string, unknown>) => ({ ...parseAutoReply({ policy: '', keywords: [], cooldownSec: 0, thresholds: { decline: 0.3, run: 0.75 }, approval: 'never' }), ...change });
  const bad: Array<[Record<string, unknown>, RegExp]> = [
    [{ sandbox: { enabled: 'yes', allowedDomains: [] } }, /invalid sandbox/],
    [{ sandbox: { enabled: true, allowedDomains: ['a b'] } }, /invalid sandbox domain/],
    [{ sandbox: { enabled: true, allowedDomains: ['x.com', 'X.com'] } }, /invalid sandbox domains/],
    [{ sandbox: { enabled: true, allowedDomains: Array.from({ length: 65 }, (_, index) => `h${index}.example.com`) } }, /invalid sandbox domains/],
    [{ sandbox: { enabled: true, allowedDomains: [], extra: 1 } }, /unsupported fields/],
    [{ scope: { localApps: ['ok', 'ok'], datasets: [] } }, /invalid scope apps/],
    [{ scope: { localApps: [], datasets: [{ source: 's', dataset: 'd', write: false, reportReview: false }, { source: 's', dataset: 'd', write: true, reportReview: false }] } }, /invalid scope datasets/],
    [{ scope: { localApps: [], datasets: [{ source: '', dataset: 'd', write: false, reportReview: false }] } }, /invalid scope dataset source/],
    [{ autoReply: reply({ policy: 'x'.repeat(4001) }) }, /invalid reply policy/],
    [{ autoReply: reply({ keywords: ['a', 'A'] }) }, /invalid reply keywords/],
    [{ autoReply: reply({ keywords: Array.from({ length: 33 }, (_, index) => `k${index}`) }) }, /invalid reply keywords/],
    [{ autoReply: reply({ cooldownSec: 86401 }) }, /invalid reply cooldown/],
    [{ autoReply: reply({ cooldownSec: 1.5 }) }, /invalid reply cooldown/],
    [{ autoReply: reply({ thresholds: { decline: 0.5, run: 0.4 } }) }, /invalid reply thresholds/],
    [{ autoReply: reply({ thresholds: { decline: -0.1, run: 0.4 } }) }, /invalid reply thresholds/],
    [{ autoReply: reply({ thresholds: { decline: 0.1, run: 1.1 } }) }, /invalid reply thresholds/],
    [{ autoReply: reply({ approval: 'maybe' }) }, /invalid reply approval/],
    [{ autoReply: reply({ quietHours: { start: '25:00', end: '07:00', timeZone: 'Asia/Shanghai' } }) }, /invalid quiet hours/],
    [{ autoReply: reply({ quietHours: { start: '22:00', end: '22:00', timeZone: 'Asia/Shanghai' } }) }, /invalid quiet hours/],
    [{ autoReply: reply({ quietHours: { start: '22:00', end: '07:00', timeZone: 'Mars/Olympus' } }) }, /invalid quiet hours/],
  ];
  for (const [patch, message] of bad) {
    assert.throws(() => f.store.update(bot.bot_id, 1, patch as any), message, JSON.stringify(patch).slice(0, 90));
    assert.throws(() => f.store.create({ name: 'Never', ...patch } as any), message, JSON.stringify(patch).slice(0, 90));
  }
  assert.equal(recordText(f.store, bot.bot_id), before);
  assert.deepEqual(f.store.list().map((listed) => listed.bot_id), [bot.bot_id], 'a refused creation leaves no Bot behind');
  assert.deepEqual(parseSandbox({ enabled: true, allowedDomains: [] }), { enabled: true, allowedDomains: [] });
  assert.deepEqual(parseScope({ localApps: [], datasets: [] }), { localApps: [], datasets: [] });
});

test('the folder a routine runs in is kept beside the Bot, one id per routine, and what is not listed runs in the Bot own folder', (t) => {
  const f = botFixture(t);
  const bot = f.store.create({ name: 'Clockwork' });
  assert.deepEqual(f.store.routineFolders(bot.bot_id), {}, 'a Bot that has none');
  f.store.setRoutineFolder(bot.bot_id, 'task-1', 'wAbCdEfGhIj');
  f.store.setRoutineFolder(bot.bot_id, 'task-2', 'wKlMnOpQrSt');
  assert.deepEqual(f.store.routineFolders(bot.bot_id), { 'task-1': 'wAbCdEfGhIj', 'task-2': 'wKlMnOpQrSt' });
  assert.deepEqual(JSON.parse(readFileSync(join(f.store.botDir(bot.bot_id), 'routine-folders.json'), 'utf8')), {
    v: 1, folders: { 'task-1': 'wAbCdEfGhIj', 'task-2': 'wKlMnOpQrSt' },
  });
  f.store.setRoutineFolder(bot.bot_id, 'task-1', undefined);
  assert.deepEqual(f.store.routineFolders(bot.bot_id), { 'task-2': 'wKlMnOpQrSt' });
  f.store.setRoutineFolder(bot.bot_id, 'task-2', undefined);
  assert.equal(existsSync(join(f.store.botDir(bot.bot_id), 'routine-folders.json')), false, 'nothing is kept when nothing is chosen');
  assert.doesNotThrow(() => f.store.setRoutineFolder(bot.bot_id, 'task-9', undefined), 'forgetting what was never kept is fine');
  assert.equal(f.store.get(bot.bot_id).revision, bot.revision, 'the Bot record is not touched: a routine is not a setting of the Bot');
});

test('a routine cannot be given the Bot own folder, an id that is no id, or a Bot that is not there; a damaged file is refused, not guessed at', (t) => {
  const f = botFixture(t);
  const bot = f.store.create({ name: 'Clockwork' });
  assert.throws(() => f.store.setRoutineFolder(bot.bot_id, 'task-1', DEFAULT_WORKSPACE_ID), /invalid workspace id/, 'the own folder is what nothing is kept for');
  assert.throws(() => f.store.setRoutineFolder(bot.bot_id, 'task-1', '../escape'), /invalid workspace id/);
  assert.throws(() => f.store.setRoutineFolder(bot.bot_id, 'a/b', 'wAbCdEfGhIj'), /invalid routine id/);
  assert.throws(() => f.store.setRoutineFolder(bot.bot_id, '', 'wAbCdEfGhIj'), /invalid routine id/);
  assert.throws(() => f.store.routineFolders(MISSING), BotNotFoundError);
  const file = join(f.store.botDir(bot.bot_id), 'routine-folders.json');
  for (const text of ['not json', '[]', '{"v":2,"folders":{}}', '{"v":1,"folders":{"task-1":"default"}}', '{"v":1,"folders":{"task-1":1}}', '{"v":1,"folders":{},"extra":true}']) {
    writeFileSync(file, text);
    assert.throws(() => f.store.routineFolders(bot.bot_id), undefined, text);
  }
  writeFileSync(file, JSON.stringify({ v: 1, folders: Object.fromEntries(Array.from({ length: 257 }, (_, n) => [`t${n}`, 'wAbCdEfGhIj'])) }));
  assert.throws(() => f.store.routineFolders(bot.bot_id), /invalid routine folders/, 'more than the limit');
});
