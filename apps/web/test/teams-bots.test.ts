import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import test from 'node:test';
import { canRecoverBot, isQuestionRecipient, teamBotTriggerId, teamBotUsable, type TeamBotCommand, type TeamBotState } from '../../../packages/teams/bots';
import { TeamMessaging, applyMessagePage, emptyProjection, type MessageApi, type MessageCommand, type MessageProjection, type MessageStorage, type TeamConversation, type TeamMessage } from '../../../packages/teams/messages';
const scope = { teamId: 'test-team', generation: 7 }, uid = 'alice';
const botKey = `bot:owner:${'h'.repeat(22)}:${'b'.repeat(22)}`;
const conversation: TeamConversation = { id: 'test-chat', kind: 'group', title: 'Fixture', participants: ['user:alice', botKey], lastSeq: 0, lastChangeSeq: 0, lastReadSeq: 0 };
const question: TeamMessage = { id: 'question-message', seq: 1, author: { kind: 'bot', uid: 'owner', botKey }, kind: 'question', text: 'Choose a value',
  triggerId: 'trigger01', origin: { uid, generation: 7 }, question: { id: 'question01', questions: ['Which value?', 'Why?'] }, createdAt: null, changeSeq: 1, changeId: 'change001' };
function storage() {
  const records = new Map<string, MessageProjection>();
  return { records, async update(key, reduce) { const value = reduce(structuredClone(records.get(key))); records.set(key, structuredClone(value)); return structuredClone(value); } } satisfies MessageStorage & { records: Map<string, MessageProjection> };
}
const until = async (condition: () => boolean) => { const end = Date.now() + 3000; while (!condition()) { if (Date.now() > end) throw new Error('Condition timed out'); await new Promise(resolve => setTimeout(resolve, 5)); } };
function apiWith(write: (command: MessageCommand) => Promise<unknown>): MessageApi {
  return { onMessagesChanged: () => () => {}, async messages<T>(command: MessageCommand): Promise<T> {
    if (command.action === 'list') return { conversations: [conversation], next: null } as T;
    if (command.action === 'sync') return { conversation, changes: [], messages: [], hasMore: false } as T;
    return await write(command) as T;
  } };
}

test('T3 question recipient is the original UID and generation, never the owner or a rejoin', () => {
  assert.equal(isQuestionRecipient(question, uid, 7), true);
  assert.equal(isQuestionRecipient(question, 'owner', 7), false);
  assert.equal(isQuestionRecipient(question, uid, 8), false);
  assert.equal(isQuestionRecipient({ ...question, recalled: true }, uid, 7), false);
  assert.equal(isQuestionRecipient({ ...question, author: { kind: 'user', uid } }, uid, 7), false);
  const state = applyMessagePage(emptyProjection(uid, scope), conversation.id, { conversation, changes: [{ id: question.changeId, seq: 1, messageId: question.id, operation: 'create' }], messages: [question], hasMore: false });
  assert.deepEqual(state.threads[conversation.id]!.messages[0], question);
});

test('T3 owner recovery accepts only expired claim/running leases or recovery state', () => {
  const state: TeamBotState = { state: 'claimed', ownerUid: 'owner', botKey, origin: { uid, generation: 7 }, leaseUntil: 100, answered: false };
  assert.equal(canRecoverBot(state, 'owner', 99), false);
  assert.equal(canRecoverBot(state, 'owner', 100), true);
  assert.equal(canRecoverBot(state, uid, 101), false);
  assert.equal(canRecoverBot({ ...state, state: 'completed' }, 'owner', 101), false);
  assert.equal(canRecoverBot({ ...state, state: 'recovery', leaseUntil: undefined }, 'owner', 101), true);
  assert.equal(canRecoverBot({ ...state, state: 'recovery', hostStatus: 'result_pending' }, 'owner', 101), false);
});

test('T3 disabled owner tombstones retain revision but cannot be used, even with a stale permitted flag', () => {
  const card = { key: botKey, ownerUid: 'owner', hostId: 'h'.repeat(22), botId: 'b'.repeat(22), name: 'Fixture', description: '', revision: 8, use: 'all' as const, uids: [], online: true, permitted: true };
  assert.equal(teamBotUsable(card), true);
  assert.equal(teamBotUsable({ ...card, permitted: false }), false);
  assert.equal(teamBotUsable({ ...card, enabled: false }), false);
  assert.equal({ ...card, enabled: false }.revision, 8);
});

test('T3 browser trigger identities match the backend SHA-256 identity', async () => {
  assert.equal(await teamBotTriggerId('message01', botKey), createHash('sha256').update(JSON.stringify(['message01', botKey])).digest('hex'));
});

test('T3 durable Bot conversation creation retains exact botKeys across response loss and restart', async context => {
  const store = storage(), calls: MessageCommand[] = []; let drop = true;
  const api = apiWith(async command => { if (command.action === 'create') { calls.push(structuredClone(command)); if (drop) throw new Error('SERVICE_UNAVAILABLE'); } return {}; });
  const first = new TeamMessaging(api, store, uid, scope); context.after(() => first.close()); await first.start(); await until(() => !first.offline);
  const botKeys = [botKey]; await first.create('bot_dm', 'Shared Bot', [uid], botKeys); botKeys.length = 0;
  assert.equal(calls.length, 1); assert.deepEqual(first.state.creation?.botKeys, [botKey]); first.close(); drop = false;
  const second = new TeamMessaging(api, store, uid, scope); context.after(() => second.close()); await second.start(); await until(() => calls.length === 2 && !second.state.creation);
  assert.deepEqual(calls[1], calls[0]); assert.equal(calls[1]!.action, 'create');
});

test('T3 explicit targets survive restart; plain @text and edits have no execution target', async context => {
  const store = storage(), calls: MessageCommand[] = []; let drop = true;
  const api = apiWith(async command => { if (command.action === 'mutate') { if (drop && calls.length) throw new Error('SERVICE_UNAVAILABLE'); calls.push(structuredClone(command)); if (drop) throw new Error('SERVICE_UNAVAILABLE'); } return {}; });
  const first = new TeamMessaging(api, store, uid, scope); context.after(() => first.close()); await first.start(); await until(() => !first.offline); first.select(conversation.id);
  await first.queue('create', 'Explicit request', undefined, undefined, [botKey]); await until(() => calls.length === 1 && first.offline); first.close(); drop = false;
  const second = new TeamMessaging(api, store, uid, scope); context.after(() => second.close()); await second.start(); await until(() => calls.length === 2 && !second.state.outbox.length);
  assert.deepEqual(calls[1], calls[0]); assert.deepEqual((calls[1] as { targets: string[] }).targets, [botKey]); second.select(conversation.id);
  await second.queue('create', '@Shared Bot just text'); await until(() => calls.length === 3); assert.equal('targets' in calls[2]!, false);
  await assert.rejects(second.queue('edit', 'Changed', 'message01', undefined, [botKey]), /INVALID_BOT_TARGET/);
});

test('T3 answers retain exact answerId, content and metadata through response loss and restart', async context => {
  const store = storage(), calls: MessageCommand[] = []; let drop = true;
  const api = apiWith(async command => { if (command.action === 'botAnswer') { if (drop && calls.length) throw new Error('SERVICE_UNAVAILABLE'); calls.push(structuredClone(command)); if (drop) throw new Error('SERVICE_UNAVAILABLE'); } return {}; });
  const first = new TeamMessaging(api, store, uid, scope); context.after(() => first.close()); await first.start(); await until(() => !first.offline); first.select(conversation.id);
  const answers = { 'Which value?': '42', 'Why?': 'Evidence' }; await first.answer(question, answers); answers['Which value?'] = 'Changed after queue';
  await until(() => calls.length === 1 && first.offline);
  const pending = first.state.answerOutbox![0]!; assert.deepEqual(pending.question, question.question); assert.deepEqual(pending.origin, question.origin); assert.equal(pending.answers['Which value?'], '42');
  await assert.rejects(first.answer(question, { 'Which value?': '43', 'Why?': 'Other' }), /ANSWER_PENDING/);
  await assert.rejects(first.discardAnswer(pending.answerId), /MESSAGE_IN_FLIGHT/); first.close(); drop = false;
  const second = new TeamMessaging(api, store, uid, scope); context.after(() => second.close()); await second.start(); await until(() => calls.length === 2 && !second.state.answerOutbox?.length);
  assert.deepEqual(calls[1], calls[0]); assert.equal('origin' in calls[1]!, false); assert.equal('question' in calls[1]!, false);
  second.select(conversation.id); await assert.rejects(second.answer(question, { 'Which value?': '43', 'Why?': 'Other' }), /ANSWER_CONFLICT/);
});

test('T3 answering rejects wrong provenance and malformed answers before persistence or transport', async () => {
  let calls = 0; const model = new TeamMessaging(apiWith(async () => { calls++; return {}; }), storage(), uid, scope); model.selected = conversation.id;
  await assert.rejects(model.answer({ ...question, origin: { uid, generation: 6 } }, { 'Which value?': '42', 'Why?': 'Evidence' }), /QUESTION_RECIPIENT_REQUIRED/);
  await assert.rejects(model.answer(question, { 'Which value?': '42' }), /INVALID_ANSWER/);
  assert.equal(calls, 0); assert.equal(model.state.answerOutbox, undefined); model.close();
});

test('T3 concurrent windows claim one durable answer and refuse a different second answer', async context => {
  const store = storage(); let sends = 0, release: (() => void) | undefined;
  const initial = emptyProjection(uid, scope);
  initial.answerOutbox = [{ action: 'botAnswer', ...scope, conversationId: conversation.id, triggerId: question.triggerId!, questionId: question.question!.id,
    answerId: 'answer001', answers: { 'Which value?': '42', 'Why?': 'Evidence' }, question: question.question!, origin: question.origin! }];
  await store.update(JSON.stringify([uid, scope.teamId, scope.generation]), () => initial);
  const api = apiWith(async command => { if (command.action === 'botAnswer') { sends++; await new Promise<void>(resolve => { release = resolve; }); } return {}; });
  const first = new TeamMessaging(api, store, uid, scope), second = new TeamMessaging(api, store, uid, scope);
  context.after(() => { first.close(); second.close(); release?.(); });
  await Promise.all([first.start(), second.start()]); await until(() => Boolean(release));
  second.selected = conversation.id;
  await assert.rejects(second.answer(question, { 'Which value?': '43', 'Why?': 'Another window' }), /ANSWER_PENDING/);
  assert.equal(sends, 1); release!(); await until(() => !first.state.answerOutbox?.length || !second.state.answerOutbox?.length);
  assert.equal(sends, 1);
});

test('T3 failed durable answer commit never starts transport', async () => {
  let sends = 0;
  const model = new TeamMessaging(apiWith(async () => { sends++; return {}; }), { async update() { throw new Error('STORAGE_FAILED'); } }, uid, scope);
  model.selected = conversation.id;
  await assert.rejects(model.answer(question, { 'Which value?': '42', 'Why?': 'Evidence' }), /STORAGE_FAILED/);
  assert.equal(sends, 0); model.close();
});

test('T3 lost first-question ACK retries its historical question and exact answer after a second question starts', async context => {
  const store = storage(), answers: Extract<MessageCommand, { action: 'botAnswer' }>[] = [], observations: Extract<MessageCommand, { action: 'botState' }>[] = [];
  let offline = false, currentQuestionId = question.question!.id;
  const nextQuestion: TeamMessage = { ...question, id: 'question-message-2', question: { id: 'question02', questions: ['Next value?'] } };
  const api = apiWith(async command => {
    if (offline) throw new Error('SERVICE_UNAVAILABLE');
    if (command.action === 'botState') {
      observations.push(command);
      return { state: 'running', ownerUid: 'owner', botKey, origin: question.origin, questionId: command.questionId ?? currentQuestionId,
        leaseUntil: Date.now() + 100000, answered: command.questionId === question.question!.id };
    }
    if (command.action === 'botAnswer') {
      answers.push(structuredClone(command));
      if (answers.length === 1) { currentQuestionId = nextQuestion.question!.id; offline = true; throw new Error('SERVICE_UNAVAILABLE'); }
      if (command.questionId === question.question!.id) assert.deepEqual(command, answers[0]);
    }
    return {};
  });
  const first = new TeamMessaging(api, store, uid, scope); context.after(() => first.close()); await first.start(); await until(() => !first.offline); first.select(conversation.id);
  await first.answer(question, { 'Which value?': '42', 'Why?': 'Evidence' }); await until(() => first.offline && answers.length === 1); first.close();
  offline = false;
  const second = new TeamMessaging(api, store, uid, scope); context.after(() => second.close()); await second.start(); await until(() => !second.state.answerOutbox?.length && answers.length === 2);
  assert.equal(observations.length, 1); assert.equal(observations[0]!.questionId, question.question!.id);
  assert.deepEqual(answers[1], answers[0]);
  const active = await api.messages<TeamBotState>({ action: 'botState', ...scope, conversationId: conversation.id, triggerId: question.triggerId! });
  assert.equal(active.questionId, nextQuestion.question!.id); assert.equal(active.answered, false);
  second.select(conversation.id); await second.answer(nextQuestion, { 'Next value?': '43' }); await until(() => answers.length === 3);
  assert.equal(answers[2]!.questionId, nextQuestion.question!.id);
});

test('T3 owner recovery persists exact settlement before send and survives page exit with a lost ACK', async context => {
  const store = storage(), commands: MessageCommand[] = []; let release: (() => void) | undefined, stateReads = 0;
  const state: TeamBotState = { state: 'recovery', ownerUid: 'owner', botKey, origin: { uid, generation: 7 }, answered: false };
  const api = apiWith(async command => {
    if (command.action === 'botState') { stateReads++; return state; }
    if (command.action === 'recoverBot') {
      commands.push(structuredClone(command));
      assert.equal([...store.records.values()][0]!.recoveryOutbox![0]!.text, command.text);
      if (commands.length === 1) { state.state = 'recovered'; await new Promise<void>(resolve => { release = resolve; }); }
    }
    return {};
  });
  const first = new TeamMessaging(api, store, 'owner', scope); context.after(() => { first.close(); release?.(); }); await first.start(); await until(() => !first.offline);
  await first.recover(conversation.id, 'trigger01', 'Checked the external result.'); await until(() => Boolean(release));
  await assert.rejects(first.recover(conversation.id, 'trigger01', 'Changed settlement'), /BOT_RESULT_CONFLICT/);
  await assert.rejects(first.discardRejectedRecovery(conversation.id, 'trigger01'), /MESSAGE_IN_FLIGHT/); first.close();
  const later = Date.now() + 31000; context.mock.method(Date, 'now', () => later);
  const second = new TeamMessaging(api, store, 'owner', scope); context.after(() => second.close()); await second.start(); await until(() => commands.length === 2 && !second.state.recoveryOutbox?.length);
  assert.deepEqual(commands[1], commands[0]); assert.equal(stateReads, 1, 'only a NEW settlement uses a fresh eligibility check');
  assert.ok(second.state.recoveryReceipts?.includes(JSON.stringify([conversation.id, 'trigger01'])));
  release!();
});

test('T3 new recovery rejects a renewed lease, another owner and result delivery pending before persisting', async () => {
  const store = storage(); let sends = 0;
  let state: TeamBotState = { state: 'running', ownerUid: 'owner', botKey, origin: { uid, generation: 7 }, leaseUntil: Date.now() + 100000, answered: false };
  const model = new TeamMessaging(apiWith(async command => { if (command.action === 'botState') return state; sends++; return {}; }), store, 'owner', scope);
  await assert.rejects(model.recover(conversation.id, 'trigger01', 'Result'), /RECOVERY_NOT_REQUIRED/);
  state = { ...state, state: 'recovery', ownerUid: 'someone-else' };
  await assert.rejects(model.recover(conversation.id, 'trigger01', 'Result'), /RECOVERY_NOT_REQUIRED/);
  state = { ...state, ownerUid: 'owner', hostStatus: 'result_pending' };
  await assert.rejects(model.recover(conversation.id, 'trigger01', 'Result'), /RECOVERY_NOT_REQUIRED/);
  assert.equal(sends, 0); assert.equal(model.state.recoveryOutbox, undefined); model.close();
});

test('T3 recovery storage failure never sends a settlement', async () => {
  const backing = storage(); let fail = false, sends = 0;
  const durable: MessageStorage = { async update(key, reduce) { if (fail) throw new Error('STORAGE_FAILED'); return backing.update(key, reduce); } };
  const api = apiWith(async command => {
    if (command.action === 'botState') { fail = true; return { state: 'recovery', ownerUid: 'owner', botKey, origin: { uid, generation: 7 }, answered: false }; }
    sends++; return {};
  });
  const model = new TeamMessaging(api, durable, 'owner', scope);
  await assert.rejects(model.recover(conversation.id, 'trigger01', 'Result'), /STORAGE_FAILED/);
  assert.equal(sends, 0); assert.equal([...backing.records.values()][0]!.recoveryOutbox, undefined); model.close();
});

const rejectsExecutorCommand = () => {
  // @ts-expect-error Host execution commands are never member commands.
  const command: TeamBotCommand = { action: 'startBot', ...scope, conversationId: conversation.id, botKey, triggerId: 'trigger01', attemptId: 'attempt01' };
  return command;
};
void rejectsExecutorCommand;
