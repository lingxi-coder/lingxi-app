import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, mkdirSync, readFileSync, readdirSync, symlinkSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

import { BotConflictError, BotNotFoundError, BotStore, parseRecord } from '../src/main/bots/store.js';
import { projectDirName } from '../src/main/bots/projectDirName.js';
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
    v: 1, bot_id: bot.bot_id, name: 'Scribe', description: 'Writes things down',
    created_at: bot.created_at, updated_at: bot.created_at, revision: 1, model: 'anthropic/claude-test',
    workspaces: [], audiences: [{ id: 'owner', kind: 'owner' }],
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
  const store = new BotStore({ userData: f.userData, home: f.home, defaultConfigHome: f.defaultConfigHome, now: () => now, newId: (() => { let n = 100; return () => `Bot${String(++n).padStart(19, '0')}`; })() });
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
  reject((record) => { record.v = 2; }, /unsupported version/);
  reject((record) => { delete record.v; }, /unsupported version/);
  reject((record) => { record.extra = true; }, /unsupported fields/);
  reject((record) => { delete record.audiences; }, /unsupported fields/);
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
  reject((record) => { record.workspaces = [{ path: 'relative', mode: 'read' }]; }, /invalid workspace path/);
  reject((record) => { record.workspaces = [{ path: '/a', mode: 'admin' }]; }, /invalid workspace mode/);
  reject((record) => { record.workspaces = [{ path: '/a', mode: 'read', extra: 1 }]; }, /unsupported fields/);
  reject((record) => { record.workspaces = [{ path: '/a', mode: 'read' }, { path: '/a', mode: 'write' }]; }, /invalid workspaces/);
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
  const store = new BotStore({ userData: f.userData, home: f.home, defaultConfigHome: f.defaultConfigHome, now: () => now, newId: () => ID });
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
  assert.deepEqual(bot.workspaces, [{ path: real, mode: 'write' }, { path: f.folder('other'), mode: 'read' }]);
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
  // Through a link, too.
  mkdirSync(join(f.root, 'work'), { recursive: true });
  symlinkSync(f.store.root, join(f.root, 'work', 'to-bots'));
  symlinkSync(f.defaultConfigHome, join(f.root, 'work', 'to-config'));
  assert.throws(() => f.store.workspace(join(f.root, 'work', 'to-bots'), 'read'), /app's own data folder/);
  assert.throws(() => f.store.workspace(join(f.root, 'work', 'to-config'), 'read'), /configuration folder/);
});

test('a configuration home outside the home is protected from both sides', (t) => {
  const f = botFixture(t);
  const configHome = join(f.root, 'elsewhere', 'config');
  mkdirSync(configHome, { recursive: true });
  const store = new BotStore({ userData: f.userData, home: f.home, defaultConfigHome: configHome });
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
