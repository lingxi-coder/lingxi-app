// Bot v3 B6a: what the service takes of a reply the Bot's maintainer reads before it is posted: the host's `holdReply` and the maintainer's three commands.
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { approvalOutcomes, sharedBotCommand } from '../src/shared-bot-commands.mjs';
import { approvalMs, approvalOutcome } from '../src/held-replies.mjs';
import { HELD_REPLY_COPY, becameHeld } from '../src/held-reply-jobs.mjs';

const command = (action, payload) => ({ schemaVersion: 1, commandId: 'command_0001', teamId: 'team_0001', generation: 1, action, payload });
const host = { deploymentId: 'deployment_01', hostId: 'host_0001', accountEpoch: 1 };
const reply = (more = {}) => ({ ...host, taskId: 'task_0001', permitId: 'permit_0001', outcome: 'completed', text: 'Dinner at 7 works.', finishedAt: 1_700_000_000_000,
  inputGrantIds: ['grant_0001'], outputAudience: { mode: 'published_inputs' }, usage: { status: 'missing' }, ...more });
const contract = path => readFileSync(new URL(`../../../packages/teams/${path}`, import.meta.url), 'utf8');

test('the codes of how a held reply came out are the ones the clients decode', () => {
  // The service does not import the contract of `packages/teams`; this keeps the two lists one list.
  const codes = [...contract('execution.ts').match(/export const APPROVAL_OUTCOMES = \[([^\]]*)\] as const;/)[1].matchAll(/'([a-z_]+)'/gu)].map(match => match[1]);
  assert.deepEqual(codes, approvalOutcomes);
  assert.deepEqual(JSON.parse(contract('contracts/execution-vectors.json')).approvalOutcomes, approvalOutcomes);
  assert.ok(JSON.parse(contract('contracts/execution-vectors.json')).taskStates.includes('awaiting_approval'));
});

test('a reply is held with exactly what completeTask would carry for a run that completed, and for nothing else', () => {
  const complete = more => sharedBotCommand.safeParse(command('completeTask', reply(more))).success;
  const hold = more => sharedBotCommand.safeParse(command('holdReply', reply(more))).success;
  for (const more of [{}, { document: { title: 'Plan', body: '1. Dinner' } }, { readFiles: [{ attachmentId: 'attachment_01', sha256: 'a'.repeat(64) }] },
    { usage: { status: 'complete', inputTokens: 10, outputTokens: 5 } }, { document: { title: 'Plan', body: 'x' }, revises: { artifactId: 'artifact_0001', expectedRevision: 1 } }]) {
    assert.equal(hold(more), true, JSON.stringify(more));
    assert.equal(hold(more), complete(more), 'the same payload');
  }
  // A failure, a decline and a run on private data have no reply to approve: they are sent as they always were.
  for (const more of [{ outcome: 'failed' }, { outcome: 'declined', text: '' }, { privateData: true, text: '' }, { text: '' }, { outcome: 'declined' }]) {
    assert.equal(hold(more), false, JSON.stringify(more));
  }
  assert.equal(complete({ outcome: 'failed' }), true);
  // Nothing besides the host's own fields, and a revision needs its document, as for completeTask.
  for (const more of [{ extra: 1 }, { approval: 'always' }, { revises: { artifactId: 'artifact_0001', expectedRevision: 1 } }, { text: 'x'.repeat(8193) }, { deploymentId: undefined }, { permitId: undefined }]) {
    assert.equal(hold(more), false, JSON.stringify(more));
  }
});

test('the maintainer reads the replies of a Bot by its id and decides one by its task; an approval may carry their own text', () => {
  const parse = (action, payload) => sharedBotCommand.safeParse(command(action, payload)).success;
  for (const payload of [{ sharedBotId: 'sharedbot_01' }, { sharedBotId: 'sharedbot_01', conversationId: 'chat_0001' }]) assert.equal(parse('listHeldReplies', payload), true, JSON.stringify(payload));
  for (const payload of [{}, { sharedBotId: 'x' }, { sharedBotId: 'sharedbot_01', conversationId: 'x' }, { sharedBotId: 'sharedbot_01', limit: 5 }, { sharedBotId: 'sharedbot_01', ...host }]) {
    assert.equal(parse('listHeldReplies', payload), false, JSON.stringify(payload));
  }
  for (const action of ['approveReply', 'rejectReply']) {
    assert.equal(parse(action, { taskId: 'task_0001' }), true, action);
    for (const payload of [{}, { taskId: 'short' }, { taskId: 'task_0001', ...host }, { taskId: 'task_0001', outcome: 'completed' }]) assert.equal(parse(action, payload), false, `${action} ${JSON.stringify(payload)}`);
  }
  assert.equal(parse('approveReply', { taskId: 'task_0001', text: 'Dinner at 8 is better.' }), true);
  // The text is the maintainer's as they left it, trimmed, with something in it, and no longer than a Bot's own reply.
  assert.equal(sharedBotCommand.parse(command('approveReply', { taskId: 'task_0001', text: '  Dinner at 8.  ' })).payload.text, 'Dinner at 8.');
  for (const text of ['', '   ', '\n', 'x'.repeat(8193), '€'.repeat(2731), 7, null]) assert.equal(parse('approveReply', { taskId: 'task_0001', text }), false, JSON.stringify(text));
  for (const text of ['x'.repeat(8192), '€'.repeat(2730)]) assert.equal(parse('approveReply', { taskId: 'task_0001', text }), true, `${text.length} characters`);
  assert.equal(parse('rejectReply', { taskId: 'task_0001', text: 'No' }), false, 'a rejection says nothing');
});

test('the vectors say the same to the service as to the clients', () => {
  const { commands } = JSON.parse(contract('contracts/execution-vectors.json'));
  const rows = commands.filter(vector => ['listHeldReplies', 'approveReply', 'rejectReply'].includes(vector.input.action));
  assert.ok(rows.length >= 18, `${rows.length} rows`);
  for (const action of ['listHeldReplies', 'approveReply', 'rejectReply']) {
    assert.ok(rows.some(vector => vector.input.action === action && vector.valid) && rows.some(vector => vector.input.action === action && !vector.valid), action);
  }
  for (const vector of rows) assert.equal(sharedBotCommand.safeParse(vector.input).success, vector.valid, vector.name);
  // `holdReply` is the host's command: the clients' validators refuse it from a member (the service takes it from a host, which the tests above show).
  assert.equal(commands.find(vector => vector.name === 'reject-holdReply-member')?.valid, false);
});

test('a hosted Bot service lists none of the four commands: its gate refuses what it does not list', () => {
  // The schema is shared with the service scope, whose gate (`hosted-bot-service.mjs`) names the commands a service takes.
  assert.doesNotMatch(readFileSync(new URL('../src/hosted-bot-service.mjs', import.meta.url), 'utf8'), /holdReply|listHeldReplies|approveReply|rejectReply/u);
});

test('what a held reply came to is told to the log from the task: nothing for a reply that was never held, the time as it is read', () => {
  const held = { state: 'awaiting_approval', heldAt: 1, deadline: 10 };
  assert.equal(approvalOutcome(held, 'awaiting_approval'), 'held');
  assert.equal(approvalOutcome(held, 'declined'), 'timeout', 'the time is up even if the job that ends the task has not run yet');
  assert.equal(approvalOutcome({ state: 'completed', approval: { decision: 'approved' }, heldAt: 1 }, 'completed'), 'approved');
  for (const [declineReason, outcome] of [['approval_rejected', 'rejected'], ['approval_timeout', 'timeout'], ['superseded', 'superseded']]) {
    assert.equal(approvalOutcome({ state: 'declined', declineReason, heldAt: 1 }, 'declined'), outcome, declineReason);
  }
  // A task that was never held says nothing, whatever else it came to, and a held one that could no longer be posted is the same as a decline.
  for (const task of [{ state: 'completed' }, { state: 'declined', declineReason: 'triage' }, { state: 'declined', declineReason: 'superseded' }, { state: 'failed' }, { state: 'declined', declineReason: 'revoked', heldAt: 1 }]) {
    assert.equal(approvalOutcome(task, task.state), undefined, JSON.stringify(task));
  }
  assert.equal(approvalMs, 10 * 60 * 1000, 'ten minutes');
});

test('the time and the push of a held reply start when its task moves into the state, and not on any other write', () => {
  const task = state => ({ state });
  assert.equal(becameHeld(task('running'), task('awaiting_approval')), true);
  assert.equal(becameHeld(undefined, task('awaiting_approval')), true, 'created held');
  for (const [before, after] of [['awaiting_approval', 'awaiting_approval'], ['awaiting_approval', 'declined'], ['awaiting_approval', 'completed'], ['running', 'completed'], ['queued', 'running']]) {
    assert.equal(becameHeld(task(before), task(after)), false, `${before} to ${after}`);
  }
  assert.equal(becameHeld(task('awaiting_approval'), undefined), false, 'a task that was deleted');
  assert.equal(becameHeld(undefined, undefined), false);
});

test('the push says only that a reply waits, never what it says', () => {
  assert.deepEqual(Object.keys(HELD_REPLY_COPY).sort(), ['en', 'zh']);
  for (const text of Object.values(HELD_REPLY_COPY)) assert.ok(text.length > 0 && text.length < 80, text);
  assert.doesNotMatch(JSON.stringify(HELD_REPLY_COPY), /assistant|助手/iu);
});
