import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { after, before, beforeEach, test } from 'node:test';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { Timestamp, getFirestore } from 'firebase-admin/firestore';
import { initializeTestEnvironment } from '@firebase/rules-unit-testing';
import { messagePreview } from '../src/team-message-preview.mjs';
import { TeamMessagesStore } from '../src/team-messages.mjs';
import { TeamsStore } from '../src/teams.mjs';
import { BATCH_SIZE, applyLastMessageRepairs, describePlan, isWholePointer, planLastMessageRepairs } from '../scripts/teams-migrate-last-message.mjs';

const script = fileURLToPath(new URL('../scripts/teams-migrate-last-message.mjs', import.meta.url));
const options = { skip: !process.env.FIRESTORE_EMULATOR_HOST, timeout: 60000 };
const projectId = 'demo-lingxi-teams-t0', teamId = 'migrate-team';
const botKey = `bot:owner:${'h'.repeat(22)}:${'b'.repeat(22)}`;
let app, db, environment;

const at = minute => Timestamp.fromDate(new Date(Date.UTC(2026, 10, 1, 12, minute)));
const alice = { kind: 'user', uid: 'alice' }, bot = { kind: 'bot', uid: 'owner', botKey };
const conversationRef = id => db.doc(`teams/${teamId}/conversations/${id}`);
const message = (seq, text, more = {}) => ({ seq, author: alice, kind: 'text', text, mentions: [], createdAt: at(seq), changeSeq: seq, changeId: `change-${seq}`, ...more });
/** `lastMessage` is left out of the document when it is `undefined`. */
async function seed(id, { lastMessage, lastSeq = 0, messages = {} }) {
  await conversationRef(id).set({ kind: 'channel', title: id, participants: ['user:alice'], memberUids: ['alice'], createdBy: 'alice', requestDigest: id,
    lastSeq, lastChangeSeq: lastSeq, lastChange: null, ...(lastMessage === undefined ? {} : { lastMessage }) });
  for (const [messageId, data] of Object.entries(messages)) await conversationRef(id).collection('messages').doc(messageId).set(data);
}
const plain = value => value instanceof Timestamp ? value.toMillis() : Array.isArray(value) ? value.map(plain)
  : value && typeof value === 'object' ? Object.fromEntries(Object.entries(value).map(([key, item]) => [key, plain(item)])) : value;
const pointer = async id => plain((await conversationRef(id).get()).get('lastMessage'));
const updateTimes = async () => Object.fromEntries((await db.collection(`teams/${teamId}/conversations`).get()).docs.map(doc => [doc.id, doc.updateTime.valueOf()]));
const long = `Line one\n\n   line two\t${'x'.repeat(200)}`;

/** Conversations in every shape the repair has to tell apart. */
async function seedAll() {
  await seed('whole', { lastSeq: 1, lastMessage: { id: 'm1', seq: 1, at: at(1), author: alice, preview: 'Hello' }, messages: { m1: message(1, 'Hello') } });
  await seed('empty', { lastSeq: 0, lastMessage: null });
  await seed('legacy', { lastSeq: 2, lastMessage: { id: 'm2', seq: 2, at: at(2) }, messages: { m1: message(1, 'First'), m2: message(2, long) } });
  await seed('recalled', { lastSeq: 1, lastMessage: { id: 'm1', seq: 1, at: at(1) }, messages: { m1: message(1, '', { recalled: true, editedAt: at(5) }) } });
  await seed('bot', { lastSeq: 2, lastMessage: { id: 'm2', seq: 2, at: at(2) }, messages: { m1: message(1, 'Ask'), m2: message(2, 'Answer from the Bot', { author: bot, kind: 'text' }) } });
  await seed('no-author', { lastSeq: 1, lastMessage: { id: 'm1', seq: 1, at: at(1), preview: 'Stale text' }, messages: { m1: message(1, 'Current text', { editedAt: at(3) }) } });
  await seed('no-preview', { lastSeq: 1, lastMessage: { id: 'm1', seq: 1, at: at(1), author: alice }, messages: { m1: message(1, 'Body') } });
  await seed('missing', { lastSeq: 2, lastMessage: { id: 'gone', seq: 2, at: at(9) }, messages: { m1: message(1, 'One'), m2: message(2, 'Two') } });
  await seed('nothing', { lastSeq: 1, lastMessage: { id: 'm1', seq: 1, at: at(1) } });
  await seed('absent', { lastSeq: 1, messages: { m1: message(1, 'Only') } });
  await seed('garbage', { lastSeq: 1, lastMessage: 'oops', messages: { m1: message(1, 'Only') } });
  await seed('bad-author', { lastSeq: 1, lastMessage: { id: 'm1', seq: 1, at: at(1), author: { kind: 'user' }, preview: 'x' }, messages: { m1: message(1, 'Only') } });
  await seed('no-time', { lastSeq: 1, lastMessage: { id: 'm1', seq: 1, at: null, author: alice, preview: 'Pending stamp' }, messages: { m1: message(1, 'Pending stamp') } });
  await seed('inconsistent', { lastSeq: 3, lastMessage: null });
  await seed('broken', { lastSeq: 1, lastMessage: { id: 'm1', seq: 1, at: at(1) }, messages: { m1: { seq: 1, kind: 'text', text: 'No author', createdAt: at(1) } } });
}
const repaired = {
  legacy: { id: 'm2', seq: 2, at: at(2).toMillis(), author: alice, preview: messagePreview(long) },
  recalled: { id: 'm1', seq: 1, at: at(1).toMillis(), author: alice, preview: '', recalled: true },
  bot: { id: 'm2', seq: 2, at: at(2).toMillis(), author: bot, preview: 'Answer from the Bot' },
  'no-author': { id: 'm1', seq: 1, at: at(1).toMillis(), author: alice, preview: 'Current text' },
  'no-preview': { id: 'm1', seq: 1, at: at(1).toMillis(), author: alice, preview: 'Body' },
  missing: { id: 'm2', seq: 2, at: at(2).toMillis(), author: alice, preview: 'Two' },
  nothing: null,
  absent: { id: 'm1', seq: 1, at: at(1).toMillis(), author: alice, preview: 'Only' },
  garbage: { id: 'm1', seq: 1, at: at(1).toMillis(), author: alice, preview: 'Only' },
  'bad-author': { id: 'm1', seq: 1, at: at(1).toMillis(), author: alice, preview: 'Only' },
};
const cli = (args, env = {}) => {
  const environment = { ...process.env, ...env };
  for (const [key, value] of Object.entries(env)) if (value === undefined) delete environment[key];
  const run = spawnSync(process.execPath, [script, ...args], { env: environment, encoding: 'utf8', timeout: 60000 });
  return { status: run.status, output: run.stdout + run.stderr };
};

before(async () => {
  if (!process.env.FIRESTORE_EMULATOR_HOST) return;
  const [host, port] = process.env.FIRESTORE_EMULATOR_HOST.split(':'); assert.ok(['127.0.0.1', 'localhost'].includes(host));
  environment = await initializeTestEnvironment({ projectId, firestore: { host, port: Number(port), rules: readFileSync(new URL('../../../infra/firebase/firestore.default.rules', import.meta.url), 'utf8') } });
  app = initializeApp({ projectId }, 'teams-migrate-last-message-tests'); db = getFirestore(app);
});
beforeEach(async () => { await environment?.clearFirestore(); });
after(async () => { await environment?.cleanup(); await db?.terminate(); if (app) await deleteApp(app); });

test('migrate lastMessage: what is whole is told from what is not, the way the clients tell them', () => {
  const whole = { id: 'm1', seq: 1, at: at(1), author: alice, preview: '' };
  assert.equal(isWholePointer(whole), true);
  assert.equal(isWholePointer({ ...whole, at: null, author: bot, preview: 'x', recalled: true }), true, 'a write the server has not stamped yet, a Bot author, a recall');
  for (const broken of [undefined, 'x', [], { id: 'm1', seq: 1, at: at(1) }, { ...whole, author: undefined }, { ...whole, author: { kind: 'user' } }, { ...whole, author: { kind: 'bot', uid: 'owner' } },
    { ...whole, author: { kind: 'robot', uid: 'x' } }, { ...whole, preview: undefined }, { ...whole, preview: 7 }, { ...whole, recalled: false }, { ...whole, id: '' }, { ...whole, seq: 1.5 }, { ...whole, at: 'now' }]) {
    assert.equal(isWholePointer(broken), false, JSON.stringify(broken));
  }
});

test('migrate lastMessage: repairs go to the database in batches of at most 400', async () => {
  const batches = [];
  const fake = { batch() { const updates = []; batches.push(updates); return { update: (ref, data, precondition) => { updates.push([ref, data, precondition]); }, commit: async () => undefined }; } };
  const repairs = Array.from({ length: 2 * BATCH_SIZE + 1 }, (_, index) => ({ ref: `ref-${index}`, after: null, updateTime: index }));
  await applyLastMessageRepairs(fake, repairs);
  assert.equal(BATCH_SIZE, 400); assert.deepEqual(batches.map(updates => updates.length), [400, 400, 1]);
  assert.deepEqual(batches[2][0], ['ref-800', { lastMessage: null }, { lastUpdateTime: 800 }], 'each write waits for the conversation to be as it was read');
});

test('migrate lastMessage: a dry run plans every repair and writes nothing', options, async () => {
  await seedAll();
  const before = await updateTimes();
  const plan = await planLastMessageRepairs(db);
  assert.equal(plan.scanned, 15); assert.equal(plan.whole, 2, 'whole and no-time'); assert.equal(plan.empty, 2, 'empty and inconsistent');
  assert.deepEqual(Object.fromEntries(plan.repairs.map(repair => [repair.path.split('/').at(-1), plain(repair.after)])), repaired);
  assert.deepEqual(plan.unfixable.map(item => item.path.split('/').at(-1)), ['broken']); assert.match(plan.unfixable[0].reason, /message m1 has no usable seq or author/);
  assert.deepEqual(plan.warnings.map(item => item.path.split('/').at(-1)), ['inconsistent']);
  assert.match(plan.repairs.find(repair => repair.path.endsWith('/missing')).source, /the newest message \(the one it names is gone\)/);
  assert.equal(plan.repairs.find(repair => repair.path.endsWith('/nothing')).source, 'no messages');
  assert.deepEqual(await updateTimes(), before, 'planning reads and nothing else');
  const report = describePlan(plan, false).join('\n');
  assert.match(report, /would repair teams\/migrate-team\/conversations\/legacy from the message it names/); assert.match(report, /Dry run: nothing was written/);
  assert.match(report, /\(no lastMessage field\)/); assert.match(report, /15 conversations: 2 whole, 2 without messages, 10 to repair, 1 that cannot be repaired\./);
});

test('migrate lastMessage: applying repairs every incomplete pointer, leaves the rest alone, and a second run finds nothing', options, async () => {
  await seedAll();
  const before = await updateTimes();
  const plan = await planLastMessageRepairs(db);
  assert.equal(await applyLastMessageRepairs(db, plan.repairs), 10);
  for (const [id, expected] of Object.entries(repaired)) assert.deepEqual(await pointer(id), expected, id);
  for (const id of ['whole', 'empty', 'no-time', 'inconsistent', 'broken']) assert.equal((await updateTimes())[id], before[id], `${id} was not written`);
  assert.deepEqual(await pointer('no-time'), { id: 'm1', seq: 1, at: null, author: alice, preview: 'Pending stamp' });
  assert.equal(isWholePointer((await conversationRef('legacy').get()).get('lastMessage')), true);
  const settled = await updateTimes();
  const again = await planLastMessageRepairs(db);
  assert.deepEqual(again.repairs, []); assert.equal(await applyLastMessageRepairs(db, again.repairs), 0);
  assert.deepEqual(await updateTimes(), settled, 'a second run writes nothing');
  assert.deepEqual(again.unfixable.map(item => item.path.split('/').at(-1)), ['broken'], 'what cannot be repaired is reported every time');
});

test('migrate lastMessage: a conversation that changed since it was read fails its batch whole, and the next run repairs it', options, async () => {
  await seedAll();
  const plan = await planLastMessageRepairs(db);
  await conversationRef('legacy').update({ title: 'renamed meanwhile' });
  await assert.rejects(applyLastMessageRepairs(db, plan.repairs));
  assert.deepEqual(await pointer('legacy'), { id: 'm2', seq: 2, at: at(2).toMillis() }, 'untouched');
  assert.deepEqual(await pointer('recalled'), { id: 'm1', seq: 1, at: at(1).toMillis() }, 'the batch is all or nothing');
  const next = await planLastMessageRepairs(db);
  assert.equal(await applyLastMessageRepairs(db, next.repairs), 10);
  assert.deepEqual(await pointer('legacy'), repaired.legacy);
});

test('migrate lastMessage: a repaired pointer keeps its message in step through later edits and recalls', options, async () => {
  const now = Date.now();
  const teams = new TeamsStore(db, () => now), messages = new TeamMessagesStore(db, () => now), owner = { uid: 'owner', name: 'Owner' };
  const scope = { teamId: 'live-team', generation: 1, conversationId: 'live-chat' };
  await teams.execute(owner, { action: 'create', teamId: 'live-team', name: 'Live team' });
  const { code } = await teams.execute(owner, { action: 'invite', teamId: 'live-team', requestId: 'invite001', role: 'member', maxUses: 10, ttlSeconds: 3600 });
  await teams.execute({ uid: 'alice', name: 'Alice' }, { action: 'join', code, requestId: 'join-alice' });
  await messages.execute(owner, { action: 'create', ...scope, kind: 'dm', title: 'Owner and Alice', participantUids: ['owner', 'alice'] });
  await messages.execute(owner, { action: 'mutate', ...scope, messageId: 'message01', changeId: 'change001', operation: 'create', text: 'First   message\nsecond line' });
  const live = db.doc('teams/live-team/conversations/live-chat');
  const { id, seq, at: created } = (await live.get()).get('lastMessage');
  await live.update({ lastMessage: { id, seq, at: created } });   // the shape older data has
  const plan = await planLastMessageRepairs(db);
  assert.deepEqual(plan.repairs.map(repair => repair.path), ['teams/live-team/conversations/live-chat']);
  await applyLastMessageRepairs(db, plan.repairs);
  assert.deepEqual(plain((await live.get()).get('lastMessage')), { id: 'message01', seq: 1, at: created.toMillis(), author: { kind: 'user', uid: 'owner' }, preview: 'First message second line' });
  await messages.execute(owner, { action: 'mutate', ...scope, messageId: 'message01', changeId: 'change002', operation: 'edit', text: 'Edited' });
  assert.equal(plain((await live.get()).get('lastMessage')).preview, 'Edited');
  await messages.execute(owner, { action: 'mutate', ...scope, messageId: 'message01', changeId: 'change003', operation: 'recall', text: '' });
  const last = (await live.get()).get('lastMessage');
  assert.equal(isWholePointer(last), true); assert.equal(last.preview, ''); assert.equal(last.recalled, true);
  assert.deepEqual((await planLastMessageRepairs(db)).repairs, []);
});

test('migrate lastMessage: the command refuses without a target it was told about, before it connects to anything', () => {
  const none = { FIRESTORE_EMULATOR_HOST: undefined, GCLOUD_PROJECT: undefined };
  for (const args of [[], ['--apply'], ['--project', 'some-project'], ['--project', 'some-project', '--apply'], ['--i-understand-this-writes-production', '--apply'], ['--bogus']]) {
    const run = cli(args, none);
    assert.equal(run.status, 1, args.join(' ')); assert.match(run.output, /Refusing to run without a target|Unknown argument/);
  }
  assert.equal(cli(['--project'], none).status, 1);
  const remote = cli(['--apply'], { FIRESTORE_EMULATOR_HOST: 'firestore.example.com:8080' });
  assert.equal(remote.status, 1); assert.match(remote.output, /must be a local address/);
  const both = cli(['--project', 'some-project', '--i-understand-this-writes-production', '--apply'], { FIRESTORE_EMULATOR_HOST: '127.0.0.1:9' });
  assert.equal(both.status, 1); assert.match(both.output, /would not reach production/);
  const help = cli(['--help'], none);
  assert.equal(help.status, 0); assert.match(help.output, /Dry run unless --apply/);
});

test('migrate lastMessage: the command is a dry run until --apply, and says what cannot be repaired', options, async () => {
  await seedAll();
  const before = await updateTimes();
  const dry = cli(['--project', projectId]);
  assert.equal(dry.status, 2, 'one conversation cannot be repaired'); assert.match(dry.output, /Target: Firestore emulator/);
  assert.match(dry.output, /would repair teams\/migrate-team\/conversations\/legacy/); assert.match(dry.output, /cannot repair teams\/migrate-team\/conversations\/broken: message m1 has no usable seq or author/);
  assert.match(dry.output, /Dry run: nothing was written/);
  assert.deepEqual(await updateTimes(), before);
  const applied = cli(['--project', projectId, '--apply']);
  assert.equal(applied.status, 2); assert.match(applied.output, /repaired teams\/migrate-team\/conversations\/legacy/); assert.doesNotMatch(applied.output, /Dry run/);
  for (const [id, expected] of Object.entries(repaired)) assert.deepEqual(await pointer(id), expected, id);
  await conversationRef('broken').delete();
  const again = cli(['--project', projectId, '--apply']);
  assert.equal(again.status, 0); assert.match(again.output, /0 to repair|0 repaired/);
  assert.doesNotMatch(again.output, /repaired teams\//);
});
