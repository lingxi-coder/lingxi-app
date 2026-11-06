import fs from 'node:fs';
import { syncBuiltinESMExports } from 'node:module';
import assert from 'node:assert/strict';
import { chmodSync, existsSync, linkSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, renameSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test, { type TestContext } from 'node:test';
import { applyRetiredTeamsData, inspectRetiredTeamsData, type RetiredTeamsCleanupPlan } from '../src/main/teams/retiredData';
const id = 'a'.repeat(22), otherId = 'b'.repeat(22);
function fixture(context: TestContext) {
  const parent = realpathSync.native(mkdtempSync(join(tmpdir(), 'teams-retired-test-'))), root = join(parent, 'userData'); mkdirSync(root, { mode: 0o700 });
  context.after(() => rmSync(parent, { recursive: true, force: true }));
  const put = (path: string, contents: string | object) => { const file = join(root, path); mkdirSync(join(file, '..'), { recursive: true, mode: 0o700 }); writeFileSync(file, typeof contents === 'string' ? contents : `${JSON.stringify(contents)}\n`, { mode: 0o600 }); return file; };
  const record = { v: 1, bot_id: id, name: 'Personal travel', revision: 8, model: 'private:model', marketOrigin: { releaseId: 'private-release' }, workspaces: [{ path: '/private/work', mode: 'read' }], custom: { teams: 'business value' }, teams: [{ teamId: 'retired-team', ownerUid: 'old-owner' }] };
  const bot = put(`bots/${id}/bot.json`, record);
  put('team-bot-host/old-owner/runtime/state.json', { pending: 'retired-run' }); put('team-bot-host/old-owner/outbox/reply.json', { text: 'retired output' });
  const preserved = ['credentials/account.json', 'chats/history.json', 'firestore/project-uid/cache', 'shared-bot-host-v2/current/snapshot', `bots/${id}/home/memory.md`, `bots/${id}/workspace/business.json`, `bots/${id}/tasks/personal.json`, 'bots/not-a-22-id/bot.json', `bots/${otherId}/bot.json`];
  for (const path of preserved) put(path, path.endsWith('bot.json') ? { v: 1, bot_id: otherId, name: 'Keep me', teams: path.includes('not-a-22-id') ? 'unrelated-directory' : undefined } : `protected:${path}`);
  const sentinel = new Map(preserved.map(path => [path, readFileSync(join(root, path))]));
  return { parent, root, put, bot, record, assertPreserved() { for (const [path, bytes] of sentinel) assert.deepEqual(readFileSync(join(root, path)), bytes, path); } };
}
function apply(root: string, plan = inspectRetiredTeamsData(root)) { return applyRetiredTeamsData(root, plan, { reviewedDigest: plan.digest, writersStopped: true }); }

test('dry-run reports exact retired root and field without changing data', context => {
  const f = fixture(context), before = readFileSync(f.bot), plan = inspectRetiredTeamsData(f.root);
  assert.deepEqual(plan.actions.map(a => [a.kind, a.relativePath]), [['delete_retired_host', 'team-bot-host'], ['remove_teams_field', `bots/${id}/bot.json`]]);
  assert.equal(plan.actions[0]!.files, 2); assert.equal(plan.digest.length, 64); assert.equal(existsSync(join(f.root, '.teams-retired-cleanup.lock')), false); assert.deepEqual(readFileSync(f.bot), before); f.assertPreserved();
});
test('apply preserves all personal fields/files and completes idempotently', context => {
  const f = fixture(context), plan = inspectRetiredTeamsData(f.root), mode = lstatSync(f.bot).mode, result = apply(f.root, plan);
  assert.equal(result.removedFiles, 2); assert.equal(result.rewrittenBots, 1); assert.equal(existsSync(join(f.root, 'team-bot-host')), false);
  const expected: Record<string, unknown> = { ...f.record }; delete expected.teams; assert.deepEqual(JSON.parse(readFileSync(f.bot, 'utf8')), expected); assert.equal(lstatSync(f.bot).mode, mode); f.assertPreserved();
  const clean = inspectRetiredTeamsData(f.root); assert.deepEqual(clean.actions, []); assert.deepEqual(apply(f.root, clean), { reviewedDigest: clean.digest, removedFiles: 0, removedDirectories: 0, rewrittenBots: 0 }); assert.equal(existsSync(join(f.root, '.teams-retired-cleanup.lock')), false); f.assertPreserved();
});
test('apply requires reviewed digest and stopped writers', context => {
  const f = fixture(context), plan = inspectRetiredTeamsData(f.root);
  assert.throws(() => applyRetiredTeamsData(f.root, plan, { reviewedDigest: '0'.repeat(64), writersStopped: true }), /REVIEW_REQUIRED/);
  assert.throws(() => applyRetiredTeamsData(f.root, plan, { reviewedDigest: plan.digest, writersStopped: false } as never), /REVIEW_REQUIRED/); assert.equal(existsSync(join(f.root, 'team-bot-host')), true); assert.deepEqual(JSON.parse(readFileSync(f.bot, 'utf8')), f.record);
});
test('Bot CAS rejects post-review personal edits before any deletion', context => {
  const f = fixture(context), plan = inspectRetiredTeamsData(f.root); writeFileSync(f.bot, JSON.stringify({ ...f.record, name: 'Changed elsewhere' }), { mode: 0o600 }); assert.throws(() => apply(f.root, plan), /CHANGED/); assert.equal(JSON.parse(readFileSync(f.bot, 'utf8')).name, 'Changed elsewhere'); assert.equal(existsSync(join(f.root, 'team-bot-host')), true); f.assertPreserved();
});
test('post-review additions to retired tree invalidate candidate before any mutation', context => {
  const f = fixture(context), plan = inspectRetiredTeamsData(f.root); f.put('team-bot-host/unreviewed.json', 'new content'); assert.throws(() => apply(f.root, plan), /CHANGED/); assert.deepEqual(JSON.parse(readFileSync(f.bot, 'utf8')), f.record); assert.equal(readFileSync(join(f.root, 'team-bot-host/unreviewed.json'), 'utf8'), 'new content'); f.assertPreserved();
});
test('tampered plan cannot expand the cleanup allowlist', context => {
  const f = fixture(context), plan = inspectRetiredTeamsData(f.root), forged = { ...plan, actions: [...plan.actions, { kind: 'delete_retired_host', relativePath: 'credentials', files: 1, bytes: 1 }] } as RetiredTeamsCleanupPlan; assert.throws(() => apply(f.root, forged), /CHANGED/); f.assertPreserved(); assert.deepEqual(JSON.parse(readFileSync(f.bot, 'utf8')), f.record);
});
test('retired root/descendant symlinks never follow or remove their targets', context => {
  for (const location of ['root', 'descendant']) { const f = fixture(context), outside = join(f.parent, `outside-${location}`); mkdirSync(outside, { mode: 0o700 }); writeFileSync(join(outside, 'protected'), 'safe', { mode: 0o600 }); if (location === 'root') { rmSync(join(f.root, 'team-bot-host'), { recursive: true }); symlinkSync(outside, join(f.root, 'team-bot-host')); } else symlinkSync(outside, join(f.root, 'team-bot-host/linked')); assert.throws(() => inspectRetiredTeamsData(f.root), /UNSAFE_PATH/); assert.equal(readFileSync(join(outside, 'protected'), 'utf8'), 'safe'); f.assertPreserved(); }
});
test('hardlinked retired files and Bot records are rejected with aliases intact', context => {
  for (const target of ['host', 'bot']) { const f = fixture(context), file = target === 'host' ? join(f.root, 'team-bot-host/old-owner/outbox/reply.json') : f.bot, alias = join(f.parent, `${target}-alias`); linkSync(file, alias); const bytes = readFileSync(alias); assert.throws(() => inspectRetiredTeamsData(f.root), /UNSAFE_PATH/); assert.deepEqual(readFileSync(alias), bytes); f.assertPreserved(); }
});
test('symlinked personal Bot directory/record is rejected', context => {
  for (const target of ['directory', 'record']) { const f = fixture(context), source = target === 'directory' ? join(f.root, 'bots', id) : f.bot, destination = join(f.parent, `moved-${target}`); renameSync(source, destination); symlinkSync(destination, source); assert.throws(() => inspectRetiredTeamsData(f.root), /UNSAFE_PATH/); assert.equal(existsSync(destination), true); }
});
test('root aliases, writable boundaries and changed Bot parent inode fail closed', context => {
  const f = fixture(context), alias = join(f.parent, 'alias'); symlinkSync(f.root, alias); assert.throws(() => inspectRetiredTeamsData(alias), /UNSAFE_PATH/); chmodSync(f.root, 0o777); assert.throws(() => inspectRetiredTeamsData(f.root), /UNSAFE_PATH/); chmodSync(f.root, 0o700); chmodSync(f.parent, 0o777); assert.throws(() => inspectRetiredTeamsData(f.root), /UNSAFE_PATH/); chmodSync(f.parent, 0o700);
  const plan = inspectRetiredTeamsData(f.root), botDir = join(f.root, 'bots', id), moved = join(f.parent, 'moved-bot'); renameSync(botDir, moved); mkdirSync(botDir, { mode: 0o700 }); renameSync(join(moved, 'bot.json'), f.bot); assert.throws(() => apply(f.root, plan), /CHANGED/); assert.deepEqual(JSON.parse(readFileSync(f.bot, 'utf8')), f.record); assert.equal(existsSync(join(f.root, 'team-bot-host')), true);
});
test('existing maintenance lock is preserved and prevents concurrent apply', context => {
  const f = fixture(context), plan = inspectRetiredTeamsData(f.root), lock = f.put('.teams-retired-cleanup.lock', 'another maintainer'); assert.throws(() => apply(f.root, plan), /EEXIST/); assert.equal(readFileSync(lock, 'utf8'), 'another maintainer'); f.assertPreserved();
});
test('malformed JSON remains untouched; new inspection finishes a partial tree cleanup', context => {
  const f = fixture(context), before = readFileSync(f.bot); writeFileSync(f.bot, '{broken', { mode: 0o600 }); assert.throws(() => inspectRetiredTeamsData(f.root), /INVALID_RECORD/); assert.equal(readFileSync(f.bot, 'utf8'), '{broken'); writeFileSync(f.bot, before); rmSync(join(f.root, 'team-bot-host/old-owner/outbox/reply.json')); const result = apply(f.root); assert.equal(result.removedFiles, 1); assert.equal(result.rewrittenBots, 1); f.assertPreserved();
});

test('owner mismatch rejects the candidate without modifying real ownership', context => {
  const f = fixture(context), uid = process.getuid!(); context.mock.method(process, 'getuid', () => uid + 1);
  assert.throws(() => inspectRetiredTeamsData(f.root), /UNSAFE_PATH/); assert.deepEqual(JSON.parse(readFileSync(f.bot, 'utf8')), f.record); f.assertPreserved();
});
test('failed staged-file durability leaves the original Bot byte-identical and retired tree intact', context => {
  const f = fixture(context), plan = inspectRetiredTeamsData(f.root), before = readFileSync(f.bot);
  const expected: Record<string, unknown> = { ...f.record }; delete expected.teams;
  const replacementSize = Buffer.byteLength(`${JSON.stringify(expected)}\n`), originalSync = fs.fsyncSync; let failed = false;
  const mocked = context.mock.method(fs, 'fsyncSync', (fd: number) => { const s = fs.fstatSync(fd); if (!failed && s.isFile() && s.size === replacementSize) { failed = true; throw new Error('injected durability failure'); } return originalSync(fd); });
  syncBuiltinESMExports();
  try { assert.throws(() => apply(f.root, plan), /injected durability failure/); } finally { mocked.mock.restore(); syncBuiltinESMExports(); }
  assert.equal(failed, true); assert.deepEqual(readFileSync(f.bot), before); assert.equal(existsSync(join(f.root, 'team-bot-host/old-owner/outbox/reply.json')), true);
  assert.equal(readdirSync(join(f.root, 'bots', id)).some(name => name.startsWith('.retired-teams-')), false); f.assertPreserved();
  assert.equal(apply(f.root, plan).rewrittenBots, 1, 'the unchanged reviewed candidate remains retryable after staging failure');
});

test('field removal preserves raw large integers, number notation, nested business teams and escaped text', context => {
  const f = fixture(context);
  const source = '{"teams":{"old":1},"large":9007199254740993,"nested":{"teams":"business"},"teams":null,"escaped":"comma,brace}","number":1e400}\n';
  const expected = '{"large":9007199254740993,"nested":{"teams":"business"},"escaped":"comma,brace}","number":1e400}\n';
  writeFileSync(f.bot, source, { mode: 0o600 }); apply(f.root); assert.equal(readFileSync(f.bot, 'utf8'), expected); f.assertPreserved();
});
