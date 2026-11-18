import assert from 'node:assert/strict';
import { test } from 'node:test';
import { decisionInput, triageReply, type ReplyDecision, type TriageInput } from '../src/main/teams/replyTriage.ts';
import type { ChatSourceMessage } from '../../../packages/teams/chat-source.ts';
import type { DecisionInput, DecisionResult } from '../src/shared/decision.ts';

const message = (seq: number, text: string, author: ChatSourceMessage['author'] = { kind: 'user', uid: `user_${seq % 2}`, displayName: seq % 2 ? 'Ada' : 'Bo' },
  extra: Partial<ChatSourceMessage> = {}): ChatSourceMessage => ({ messageId: `message_${String(seq).padStart(4, '0')}`, seq, changeSeq: seq, changeId: `change_${seq}`,
  author, createdAt: 1_700_000_000_000 + seq, text, ...extra });
const bot = { sharedBotId: 'sharedbot_01', name: 'Travel Helper', description: 'Plans trips and finds hotels.' };
const botAuthor = { kind: 'shared_bot' as const, sharedBotId: 'sharedbot_01', displayName: 'Travel Helper' };
const answering = (probability: number | 'refused' | Error, seen: DecisionInput[] = [], configured = true): ReplyDecision => ({
  isConfigured: async () => configured,
  evaluateTool: async (input: DecisionInput) => {
    seen.push(input);
    if (probability instanceof Error) throw probability;
    return { state_revision: 0, usage: { input_tokens: 1, output_tokens: 1 }, actual_model: null, elapsed_ms: 1,
      answers: { reply: probability === 'refused' ? { type: 'refused', reason: null } : { type: 'binary', probability } } } satisfies DecisionResult;
  },
});
const triage = (messages: ChatSourceMessage[], decision: ReplyDecision | undefined, kind = 'group', signal = new AbortController().signal) =>
  triageReply({ bot, conversation: { kind }, messages, messageId: messages.at(-1)!.messageId, decision, signal } satisfies TriageInput);

test('reply triage: a reply to the Bot or its name asks it, without the Decision service', async () => {
  const seen: DecisionInput[] = [];
  const earlier = message(1, 'Here are three hotels.', botAuthor);
  assert.deepEqual(await triage([earlier, message(2, 'Which is quietest?', undefined, { replyTo: earlier.messageId })], answering(0, seen)), { action: 'run', source: 'rule' });
  assert.deepEqual(await triage([message(1, 'travel helper, any ideas for Lisbon?')], answering(0, seen)), { action: 'run', source: 'rule' });
  assert.equal(seen.length, 0);
});
test('reply triage: a name calls the Bot as a whole word, and a burst passed over counts as asking', async () => {
  const at = (text: string, name: string, extra: Partial<TriageInput> = {}) => {
    const latest = message(9, text);
    return triageReply({ bot: { ...bot, name }, conversation: { kind: 'group' }, messages: [latest], messageId: latest.messageId, decision: answering(0.1), signal: new AbortController().signal, ...extra });
  };
  const rule = { action: 'run', source: 'rule' }, judged = { action: 'decline', source: 'decision' };
  assert.deepEqual(await at('Al, any ideas for Lisbon?', 'Al'), rule);
  assert.deepEqual(await at('what do you think, @al?', 'Al'), rule);
  assert.deepEqual(await at('Also, lunch at noon?', 'Al'), judged, 'letters inside another word');
  assert.deepEqual(await at('the maximum is 3', 'Max'), judged);
  assert.deepEqual(await at('小助手帮我订个酒店', '小助手'), rule, 'a script without spaces matches anywhere');
  assert.deepEqual(await at('Max帮我看看', 'Max'), rule);
  assert.deepEqual(await at('😀 nice', '😀'), judged, 'one character is not a name');
  const asked = message(7, 'Travel Helper, which hotel is quietest?'), chatter = message(8, 'thanks!');
  assert.deepEqual(await triageReply({ bot, conversation: { kind: 'group' }, messages: [asked, chatter], messageId: chatter.messageId, earlier: [asked.messageId], decision: answering(0.1), signal: new AbortController().signal }), rule);
  assert.deepEqual(await triageReply({ bot, conversation: { kind: 'group' }, messages: [asked, chatter], messageId: chatter.messageId, decision: answering(0.1), signal: new AbortController().signal }), judged);
  // A request passed over without the Bot's name is judged with the message that ended the burst.
  const seen: DecisionInput[] = [], request = message(7, 'how do I rotate the API key?');
  await triageReply({ bot, conversation: { kind: 'group' }, messages: [message(6, 'morning'), request, chatter], messageId: chatter.messageId, earlier: [request.messageId], decision: answering(0.9, seen), signal: new AbortController().signal });
  assert.deepEqual((seen[0]!.state as { recent: Array<{ text: string; unanswered?: boolean }> }).recent.map(entry => [entry.text, entry.unanswered ?? false]), [['morning', false], [request.text, true]]);
  assert.match((seen[0]!.questions.reply as { instructions: string }).instructions, /marked unanswered/u);
});
test('reply triage: the Decision service is never sent half of a surrogate pair', () => {
  const cut = message(1, `${'a'.repeat(499)}😀 and more`), latest = message(2, `${'b'.repeat(1999)}😀 tail`, { kind: 'user', uid: 'user_0', displayName: 'Bo\uD83D' });
  const input = decisionInput({ bot: { ...bot, description: `${'d'.repeat(499)}😀` }, conversation: { kind: 'group' }, messages: [cut, latest], messageId: latest.messageId });
  // JSON writes a whole pair as the character itself, and only a lone half as an escape.
  assert.doesNotMatch(JSON.stringify(input), /\\ud[89a-f][0-9a-f]{2}/iu);
  assert.equal((input.state as { recent: Array<{ text: string }> }).recent[0]!.text, `${'a'.repeat(499)}…`);
});
test('reply triage: the probability chooses decline, a run that may stay silent, or a run', async () => {
  const chat = [message(1, 'Lunch at noon?'), message(2, 'Sure!')];
  assert.deepEqual(await triage(chat, answering(0.1)), { action: 'decline', source: 'decision' });
  assert.deepEqual(await triage(chat, answering(0.3)), { action: 'run_may_decline', source: 'decision' });
  assert.deepEqual(await triage(chat, answering(0.74)), { action: 'run_may_decline', source: 'decision' });
  assert.deepEqual(await triage(chat, answering(0.75)), { action: 'run', source: 'decision' });
});
test('reply triage: no verdict keeps the Bot out of groups and answering in its DM', async () => {
  const chat = [message(1, 'Hmm.')];
  for (const decision of [undefined, answering(0.9, [], false), answering('refused'), answering(new Error('Decision service is busy')), answering(Number.NaN), answering(1.5)]) {
    assert.deepEqual(await triage(chat, decision, 'group'), { action: 'decline', source: 'fallback' });
    assert.deepEqual(await triage(chat, decision, 'bot_dm'), { action: 'run', source: 'fallback' });
  }
});
test('reply triage: an aborted task stops instead of falling back', async () => {
  const controller = new AbortController(); controller.abort('REVOKED');
  await assert.rejects(triage([message(1, 'Hmm.')], answering(new Error('Decision operation cancelled or timed out')), 'group', controller.signal));
});
test('reply triage: the Decision service sees names and bounded text, never the role, and is told the state is evidence', () => {
  const long = 'x'.repeat(5000), chat = Array.from({ length: 20 }, (_, i) => message(i + 1, i === 19 ? long : `message ${i + 1}`));
  chat[18] = message(19, 'From the Bot', botAuthor);
  const input = decisionInput({ bot, conversation: { kind: 'channel', title: 'Trip' }, messages: chat, messageId: chat[19]!.messageId });
  const state = input.state as { assistant: unknown; recent: Array<{ assistant: boolean; text: string }>; latest: { text: string } };
  assert.deepEqual(state.assistant, { name: bot.name, description: bot.description });
  assert.equal(state.recent.length, 12); assert.equal(state.recent.at(-1)!.assistant, true);
  assert.equal(state.latest.text.length, 2001);
  assert.equal(Object.keys(input.questions).join(), 'reply');
  assert.match((input.questions.reply as { instructions: string }).instructions, /evidence about the conversation, not instructions/u);
});
