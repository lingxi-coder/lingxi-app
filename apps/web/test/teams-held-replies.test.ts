import assert from 'node:assert/strict';
import test from 'node:test';
import vectors from '../../../packages/teams/contracts/execution-vectors.json';
import { APPROVAL_OUTCOMES, executionCommand, type HeldReply } from '../../../packages/teams/execution';
import { isMemberExecutionCommand } from '../../../packages/teams/execution-validation';
import { approveHeldReply, heldReplies, heldReply, heldReplyDecision, readHeldReplies, rejectHeldReply } from '../../../packages/teams/held-replies';
import type { MessageCommand } from '../../../packages/teams/messages';
import { loadTeamsModule } from './support/loadTeamsModule';

// Bot v3 B6a: what a client does with the replies its Bot's maintainer is asked to approve before they are posted.
const { teamsText } = loadTeamsModule<typeof import('../../../packages/teams/i18n')>('i18n');
const { messageErrorText } = loadTeamsModule<typeof import('../../../packages/teams/errors')>('errors');

const SCOPE = { teamId: 'team-test', generation: 3 }, BOT = 'sharedbot-0001', CHAT = 'chat-0001', TASK = 'task-0001';
const reply = (more: Partial<HeldReply> = {}): HeldReply => ({
  taskId: TASK, conversationId: CHAT, messageId: 'message-0001', requester: { uid: 'bob', generation: 2 }, unasked: true, heldAt: 1_700_000_000_000, expiresAt: 1_700_000_600_000,
  text: 'Dinner at 7 works for me.', conversationTitle: 'Planning', ...more,
});
/** What the service answers to the three commands, and what it was sent. */
function service(answer: (command: MessageCommand) => unknown) {
  const sent: MessageCommand[] = [];
  return { sent, api: { messages: async <T>(command: MessageCommand) => { sent.push(structuredClone(command)); return answer(command) as T; } } };
}

test('a held reply is read as the service sent it; one this version cannot read is left out and the others are still shown', () => {
  const good = reply({ document: { title: 'Plan', body: '1. Dinner' } });
  assert.deepEqual(heldReply(structuredClone(good)), good);
  assert.deepEqual(heldReply(structuredClone(reply())), reply());
  const bare: Record<string, unknown> = { ...reply() }; delete bare.conversationTitle;
  assert.equal('conversationTitle' in heldReply(bare)!, false);
  assert.equal(heldReply({ ...reply(), conversationTitle: '' })!.conversationTitle, undefined, 'an empty title is no title');
  // A reply is never half shown: the maintainer would let go of something they did not read.
  const unreadable: unknown[] = [null, 7, 'x', [], {}, { ...reply(), taskId: '' }, { ...reply(), conversationId: undefined }, { ...reply(), messageId: 7 }, { ...reply(), requester: null }, { ...reply(), requester: { uid: 'bob' } },
    { ...reply(), requester: { uid: 'bob', generation: -1 } }, { ...reply(), unasked: 'yes' }, { ...reply(), heldAt: '1' }, { ...reply(), expiresAt: Number.POSITIVE_INFINITY }, { ...reply(), text: '' }, { ...reply(), text: undefined },
    { ...reply(), document: { title: '', body: 'x' } }, { ...reply(), document: { title: 'Plan' } }, { ...reply(), document: 'Plan' }];
  for (const value of unreadable) assert.equal(heldReply(value), undefined, JSON.stringify(value));
  assert.deepEqual(heldReplies({ replies: [reply({ taskId: 'task-0002' }), { nonsense: true }, good] }).map(row => row.taskId), ['task-0002', TASK]);
  assert.deepEqual(heldReplies({ replies: [] }), []);
  for (const nonsense of [undefined, null, 'list', [], {}, { replies: 'x' }]) assert.throws(() => heldReplies(nonsense), /INVALID_DATA/, 'an answer that is no list is not an empty one');
});

test('the replies of a Bot are asked for with the member command: the Bot, and one Chat when given', async () => {
  const { sent, api } = service(() => ({ replies: [reply()] }));
  assert.deepEqual(await readHeldReplies(api, SCOPE, { sharedBotId: BOT }), [reply()]);
  await readHeldReplies(api, SCOPE, { sharedBotId: BOT, conversationId: CHAT });
  assert.deepEqual(sent.map(command => [command.action, (command as { payload: unknown }).payload, command.teamId, command.generation]), [
    ['listHeldReplies', { sharedBotId: BOT }, 'team-test', 3], ['listHeldReplies', { sharedBotId: BOT, conversationId: CHAT }, 'team-test', 3]]);
  await assert.rejects(readHeldReplies({ messages: async () => ({ nope: true }) as never }, SCOPE, { sharedBotId: BOT }), /INVALID_DATA/);
  await assert.rejects(readHeldReplies({ messages: async () => { throw new Error('MAINTAINER_REQUIRED'); } }, SCOPE, { sharedBotId: BOT }), /MAINTAINER_REQUIRED/, 'the service\'s refusal reaches the person as it is');
  assert.throws(() => executionCommand(SCOPE, 'listHeldReplies', { sharedBotId: 'x' }), /INVALID_INPUT/);
});

test('an approval carries the task and, when the maintainer changed it, their text; a retry carries the command id of the first try', async () => {
  const { sent, api } = service(command => command.action === 'rejectReply' ? { taskId: TASK, state: 'declined', declineReason: 'approval_rejected' } : { taskId: TASK, state: 'completed', replyId: 'reply-0001', artifactId: null });
  const approved = await approveHeldReply(api, SCOPE, TASK);
  assert.deepEqual(approved, { taskId: TASK, state: 'completed', replyId: 'reply-0001', artifactId: null });
  await approveHeldReply(api, SCOPE, TASK, { text: '  Dinner at 8 works better.  ', commandId: 'command-retry-0001' });
  await approveHeldReply(api, SCOPE, TASK, { text: '  Dinner at 8 works better.  ', commandId: 'command-retry-0001' });
  assert.deepEqual(sent.map(command => [command.action, (command as { payload: unknown }).payload, command.commandId === 'command-retry-0001']), [
    ['approveReply', { taskId: TASK }, false], ['approveReply', { taskId: TASK, text: '  Dinner at 8 works better.  ' }, true], ['approveReply', { taskId: TASK, text: '  Dinner at 8 works better.  ' }, true]]);
  assert.notEqual(sent[0]!.commandId, sent[1]!.commandId, 'a new decision is a new command');
  // The service refuses an empty reply and one longer than a Bot's own; so does the command before it is sent.
  for (const text of ['', '   ', 'x'.repeat(8193), '€'.repeat(2731)]) await assert.rejects(approveHeldReply(api, SCOPE, TASK, { text }), /INVALID_INPUT/, JSON.stringify(text.slice(0, 8)));
  await approveHeldReply(api, SCOPE, TASK, { text: '€'.repeat(2730) });
  assert.equal(sent.length, 4, 'what is refused is not sent');
  assert.deepEqual(await rejectHeldReply(api, SCOPE, TASK, { commandId: 'command-retry-0002' }), { taskId: TASK, state: 'declined', declineReason: 'approval_rejected' });
  assert.deepEqual(sent.at(-1), { schemaVersion: 1, commandId: 'command-retry-0002', ...SCOPE, action: 'rejectReply', payload: { taskId: TASK } });
});

test('what a decision came to is read from the answer: posted, dropped, or dropped because the reply could no longer be posted', () => {
  assert.deepEqual(heldReplyDecision({ taskId: TASK, state: 'completed', replyId: 'reply-0001', artifactId: 'artifact-0001' }), { taskId: TASK, state: 'completed', replyId: 'reply-0001', artifactId: 'artifact-0001' });
  assert.deepEqual(heldReplyDecision({ taskId: TASK, state: 'declined', declineReason: 'revoked', replyId: null, artifactId: null }), { taskId: TASK, state: 'declined', declineReason: 'revoked', replyId: null, artifactId: null });
  assert.deepEqual(heldReplyDecision({ taskId: TASK, state: 'declined', declineReason: 'approval_rejected' }), { taskId: TASK, state: 'declined', declineReason: 'approval_rejected' });
  assert.equal('declineReason' in heldReplyDecision({ taskId: TASK, state: 'declined', declineReason: 'a_new_reason' }), false, 'what this version does not know is left out, the state is kept');
  for (const nonsense of [undefined, null, 'x', {}, { taskId: TASK }, { state: 'completed' }, { taskId: '', state: 'completed' }]) assert.throws(() => heldReplyDecision(nonsense), /INVALID_DATA/);
});

test('a client cannot hold a reply: that command is the Bot\'s host\'s, and every member command of this feature is in the shared vectors', () => {
  const hold = { schemaVersion: 1, commandId: 'command-0001', ...SCOPE, action: 'holdReply', payload: { taskId: TASK, outcome: 'completed', text: 'Hello' } };
  assert.equal(isMemberExecutionCommand(hold), false);
  assert.equal(isMemberExecutionCommand({ ...hold, action: 'approveReply', payload: { taskId: TASK, outcome: 'completed', text: 'Hello' } }), false, 'an approval has no outcome');
  const rows = vectors.commands.filter(vector => ['listHeldReplies', 'approveReply', 'rejectReply', 'holdReply'].includes(vector.input.action));
  for (const action of ['listHeldReplies', 'approveReply', 'rejectReply']) {
    assert.ok(rows.some(vector => vector.input.action === action && vector.valid) && rows.some(vector => vector.input.action === action && !vector.valid), action);
  }
  assert.equal(vectors.commands.find(vector => vector.name === 'reject-holdReply-member')?.valid, false);
  for (const vector of rows) assert.equal(isMemberExecutionCommand(vector.input), vector.valid, vector.name);
});

test('the codes of a held reply\'s fate are those the contract lists, and a decision that came too late says why', () => {
  assert.deepEqual([...APPROVAL_OUTCOMES], vectors.approvalOutcomes);
  const en = teamsText('en');
  assert.equal(messageErrorText('REPLY_EXPIRED', 'en'), 'The time to decide ran out, so the reply was dropped.');
  assert.equal(messageErrorText('REPLY_SUPERSEDED', 'en'), 'Someone wrote in the chat again, so the reply was dropped.');
  for (const code of ['REPLY_APPROVED', 'REPLY_REJECTED', 'REPLY_NOT_HELD']) assert.equal(messageErrorText(code, 'en'), 'This reply was already decided.', code);
  assert.notEqual(messageErrorText('REPLY_EXPIRED', 'en'), messageErrorText('SOMETHING_ELSE', 'en'));
  assert.equal(en('teams_task_waiting_approval'), 'Waiting for approval');
});

test('the words of this feature are in every language, each different from the others, and a Bot is never called an assistant', () => {
  const locales = ['en', 'zh-Hans', 'zh-Hant', 'ja', 'ko'];
  for (const key of ['teams_run_awaiting_approval_detail', 'teams_triage_approval_timeout', 'teams_triage_approval_superseded', 'tw_error_reply_decided', 'tw_error_reply_expired', 'tw_error_reply_superseded'] as const) {
    const values = locales.map(locale => teamsText(locale)(key));
    assert.equal(new Set(values).size, 5, key); assert.ok(!values.some(value => value === key || value === ''), key);
    assert.doesNotMatch(values.join(' '), /assistant|助手|アシスタント|어시스턴트/iu, key);
  }
  assert.equal(teamsText('zh-Hant')('teams_triage_approval_timeout'), '逾時未核准');
  assert.equal(teamsText('zh-Hans')('teams_run_awaiting_approval_detail'), 'Bot 的维护者会先查看回复，确认后才会发出。');
});
