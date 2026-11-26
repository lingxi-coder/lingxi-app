// Bot v3 B7a: what the service decides about a message several Bots may answer, without a database: the codes it shares with the clients, the commands it takes,
// whom a message speaks to, and which writes start a round's timers. The rounds themselves, in transactions, are `reply-rounds.emulator.test.mjs`.
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { bidTiers, humanMentionModes, maxRepliesLimit, roundOutcomes, sharedBotCommand } from '../src/shared-bot-commands.mjs';
import { addressedBots, calledBy } from '../src/bot-address.mjs';
import { becameCandidate, becameWinner } from '../src/reply-round-jobs.mjs';
import { roundState, roundWindowMs, startWindowMs } from '../src/reply-rounds.mjs';

const command = (action, payload) => ({ schemaVersion: 1, commandId: 'command_0001', teamId: 'team_0001', generation: 1, action, payload });
const host = { deploymentId: 'deployment_01', hostId: 'host_0001', accountEpoch: 1 };
const contract = path => readFileSync(new URL(`../../../packages/teams/${path}`, import.meta.url), 'utf8');
const list = (source, name) => [...source.match(new RegExp(`export const ${name} = \\[([^\\]]*)\\] as const;`))[1].matchAll(/'([a-z_]+)'/gu)].map(match => match[1]);
const vectors = JSON.parse(contract('contracts/execution-vectors.json'));

test('the codes of a reply round are the ones the clients decode', () => {
  // The service does not import the contract of `packages/teams`; this keeps the lists one list.
  assert.deepEqual(list(contract('execution.ts'), 'BID_TIERS'), bidTiers);
  assert.deepEqual(list(contract('execution.ts'), 'ROUND_OUTCOMES'), roundOutcomes);
  assert.deepEqual(list(contract('execution-validation.ts'), 'HUMAN_MENTION_MODES'), humanMentionModes);
  assert.equal(Number(contract('execution-validation.ts').match(/export const MAX_REPLIES_LIMIT = (\d+);/)[1]), maxRepliesLimit);
  assert.deepEqual([vectors.bidTiers, vectors.roundOutcomes, vectors.humanMentionModes, vectors.maxRepliesLimit], [bidTiers, roundOutcomes, humanMentionModes, maxRepliesLimit]);
  // Best first: the order is the ranking.
  assert.deepEqual(bidTiers, ['addressed', 'rule', 'run', 'may_decline', 'decline']);
});

test('a round collects bids for fifteen seconds, and its winners have twenty to start', () => {
  assert.equal(roundWindowMs, 15000);
  assert.equal(startWindowMs, 20000);
});

test('a bid is the host\'s command: a task, a tier, a probability and what judged it, from the deployment that holds the task', () => {
  const bid = more => sharedBotCommand.safeParse(command('bidReply', { ...host, taskId: 'task_0001', tier: 'run', score: 0.9, source: 'decision', ...more })).success;
  assert.equal(bid({}), true);
  for (const tier of bidTiers) assert.equal(bid({ tier }), true, tier);
  for (const source of ['quiet', 'cooldown', 'rule', 'decision', 'fallback']) assert.equal(bid({ source }), true, source);
  assert.equal(bid({ score: undefined }), true, 'a bid with no probability: a rule, or an abstention');
  assert.equal(bid({ score: 0 }), true);
  assert.equal(bid({ score: 1 }), true);
  for (const more of [{ tier: 'best' }, { tier: undefined }, { tier: 'Run' }, { score: -0.1 }, { score: 1.1 }, { score: '0.5' }, { score: null }, { score: Number.NaN },
    { source: undefined }, { source: 'model' }, { taskId: undefined }, { taskId: 'x' }, { deploymentId: undefined }, { hostId: undefined }, { accountEpoch: undefined }, { accountEpoch: 0 },
    { extra: true }, { round: 'won' }, { outcome: 'won' }]) {
    assert.equal(bid(more), false, JSON.stringify(more));
  }
  // The command id and the task are the same for a bid sent again: it is one request.
  assert.equal(sharedBotCommand.safeParse(command('bidReply', { ...host, taskId: 'task_0001', tier: 'decline', source: 'fallback' })).success, true);
});

test('the Chat decides how many Bots answer and what a message that mentions only people does: both are optional, checked, and nothing else comes with them', () => {
  const set = more => sharedBotCommand.safeParse(command('setConversationAutoReply', { conversationId: 'chat_0001', expectedRevision: 1, targets: ['sharedbot_01'], ...more })).success;
  assert.equal(set({}), true, 'a caller that changes neither leaves them as they are');
  for (const maxReplies of [1, 2, 3, 4, 5]) assert.equal(set({ maxReplies }), true, String(maxReplies));
  for (const humanMentions of humanMentionModes) assert.equal(set({ humanMentions }), true, humanMentions);
  assert.equal(set({ maxReplies: 2, humanMentions: 'judge' }), true);
  for (const maxReplies of [0, 6, -1, 1.5, '2', null, Number.NaN, 1e9]) assert.equal(set({ maxReplies }), false, String(maxReplies));
  for (const humanMentions of ['ignore', 'Quiet', '', null, true, 1]) assert.equal(set({ humanMentions }), false, String(humanMentions));
  assert.equal(set({ quiet: true }), false);
});

test('the vectors say the same to the service as to the clients', () => {
  const rows = vectors.commands.filter(vector => vector.input.action === 'setConversationAutoReply');
  assert.ok(rows.length >= 17, `${rows.length} rows`);
  assert.ok(rows.some(vector => vector.valid && vector.input.payload.maxReplies !== undefined) && rows.some(vector => vector.valid && vector.input.payload.humanMentions !== undefined));
  for (const vector of rows) assert.equal(sharedBotCommand.safeParse(vector.input).success, vector.valid, vector.name);
  // `bidReply` is the host's command: the clients' validators refuse it from a member, and so does the service when it carries no host (the service takes it from a host, which the tests above show).
  const member = vectors.commands.find(vector => vector.name === 'reject-bidReply-member');
  assert.equal(member?.valid, false);
  assert.equal(sharedBotCommand.safeParse(member.input).success, false);
});

test('a hosted Bot service takes no bid: its tasks are asked for, and have no round', () => {
  // The schema is shared with the service scope, whose gate (`hosted-bot-service.mjs`) names the commands a service takes.
  assert.doesNotMatch(readFileSync(new URL('../src/hosted-bot-service.mjs', import.meta.url), 'utf8'), /bidReply/u);
});

test('a name calls a Bot as a whole word where it is a word, with case and width folded, and as a part of a text where it is not', () => {
  const calls = (text, name) => calledBy(text, name);
  assert.equal(calls('Al, any ideas for Lisbon?', 'Al'), true);
  assert.equal(calls('what do you think, @al?', 'Al'), true, 'a mention mark is no letter');
  assert.equal(calls('Also, lunch at noon?', 'Al'), false, 'letters inside another word');
  assert.equal(calls('the maximum is 3', 'Max'), false);
  assert.equal(calls('MAX, are you there', 'max'), true, 'case');
  assert.equal(calls('ＭＡＸ，你好', 'max'), true, 'width: full-width letters');
  assert.equal(calls('小助手帮我订个酒店', '小助手'), true, 'a script without spaces matches anywhere');
  assert.equal(calls('Max帮我看看', 'Max'), true, 'a word of a script with spaces ends where another script begins');
  assert.equal(calls('Max2 is here', 'Max'), false, 'a digit goes on a word');
  assert.equal(calls('😀 nice', '😀'), false, 'one character is not a name');
  assert.equal(calls('a', 'a'), false);
  assert.equal(calls('Travel Helper, which hotel?', ' Travel Helper '), true, 'the name is trimmed');
  assert.equal(calls('travel-helper rocks', 'Travel Helper'), false);
  assert.equal(calls('anything', ''), false);
  assert.equal(calls('anything', '   '), false);
});

test('a message speaks to the Bot it answers and to the Bots it calls by name, and to nobody else', () => {
  const bots = [{ sharedBotId: 'sharedbot_01', name: 'Travel Helper' }, { sharedBotId: 'sharedbot_02', name: 'Max' }, { sharedBotId: 'sharedbot_03', name: undefined }];
  const reply = sharedBotId => ({ author: { kind: 'shared_bot', sharedBotId, displayName: 'x' } });
  assert.deepEqual(addressedBots(bots, { text: 'thanks!', replied: reply('sharedbot_01') }), ['sharedbot_01']);
  assert.deepEqual(addressedBots(bots, { text: 'Max, which hotel?' }), ['sharedbot_02']);
  assert.deepEqual(addressedBots(bots, { text: 'travel helper and max, which hotel?' }), ['sharedbot_01', 'sharedbot_02']);
  assert.deepEqual(addressedBots(bots, { text: 'thanks max', replied: reply('sharedbot_01') }), ['sharedbot_01', 'sharedbot_02'], 'both the answered Bot and a named one');
  assert.deepEqual(addressedBots(bots, { text: 'the maximum is 3' }), []);
  assert.deepEqual(addressedBots(bots, { text: 'thanks!', replied: { author: { kind: 'user', uid: 'someone' } } }), [], 'an answer to a person is for the person');
  assert.deepEqual(addressedBots(bots, { text: 'thanks!', replied: reply('sharedbot_99') }), [], 'nor to a Bot of another Chat');
  assert.deepEqual(addressedBots(bots, { text: 'anyone?' }), []);
  assert.deepEqual(addressedBots([], { text: 'Max?', replied: reply('sharedbot_01') }), []);
});

test('the timers of a round start from the write of a task that has just joined it, or just won it, and from no other', () => {
  const task = more => ({ roundId: 'message_0001', state: 'queued', ...more });
  assert.equal(becameCandidate(undefined, task()), true, 'created into a round');
  assert.equal(becameCandidate(task(), task({ state: 'declined' })), false, 'a write of a task that was one already');
  assert.equal(becameCandidate(undefined, { state: 'queued' }), false, 'an asked task has no round');
  assert.equal(becameCandidate(undefined, undefined), false);
  assert.equal(becameCandidate(task(), undefined), false, 'deleted');
  const won = task({ round: { outcome: 'won', tier: 'run' } });
  assert.equal(becameWinner(task(), won), true);
  assert.equal(becameWinner(undefined, won), true);
  assert.equal(becameWinner(won, { ...won, state: 'running' }), false, 'once');
  assert.equal(becameWinner(task(), task({ round: { outcome: 'lost' } })), false);
  assert.equal(becameWinner(task(), task({ round: { outcome: 'promoted', tier: 'run' } })), false, 'a promotion is not timed again: the round promotes once');
  assert.equal(becameWinner(task(), undefined), false);
});

test('a task says where its round stands for its host: open until the round has said, then what became of it; nothing for a task outside a round', () => {
  assert.equal(roundState({}), undefined);
  assert.equal(roundState({ optional: true }), undefined);
  assert.equal(roundState({ roundId: 'message_0001' }), 'open');
  for (const outcome of roundOutcomes) assert.equal(roundState({ roundId: 'message_0001', round: { outcome } }), outcome);
});
