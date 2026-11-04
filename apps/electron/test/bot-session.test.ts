import assert from 'node:assert/strict';
import test from 'node:test';

import {
  botActivity,
  botIdOfSession,
  botRuntimes,
  isBotSession,
  pendingBotId,
  pendingBotPath,
  relativeActivity,
  removeBot,
  startBotChat,
} from '../src/renderer/bridge/botSession';
import { pinEnglish } from './bot-ui-helpers';

pinEnglish();

const runtime = (sessionId: string, botId: string | undefined, overrides: Partial<{ turnActive: boolean; pendingInteractions: number; pendingAskUserQuestions: number }> = {}) => ({
  sessionId, ...(botId === undefined ? {} : { botId }), turnActive: false, pendingInteractions: 0, pendingAskUserQuestions: 0, ...overrides,
});

test('the folder a Bot chat waits on while it starts names the Bot and is no project path', () => {
  assert.equal(pendingBotPath('abc'), 'bot:abc');
  assert.equal(pendingBotId(pendingBotPath('abc')), 'abc');
  assert.equal(pendingBotId('/Users/me/work/bot:abc'), undefined);
  assert.equal(pendingBotId('/Users/me/work'), undefined);
  assert.equal(pendingBotId(undefined), undefined);
});

test('a displayed chat is a Bot\'s when the host marked its runtime, or while it is still being started', () => {
  const runtimes = [runtime('s-bot', 'bot-1'), runtime('s-project', undefined)];
  assert.equal(botIdOfSession(runtimes, { sessionId: 's-bot', projectPath: '/data/bots/bot-1/workspace' }), 'bot-1');
  assert.equal(botIdOfSession(runtimes, { sessionId: 's-project', projectPath: '/work/app' }), undefined);
  assert.equal(botIdOfSession(runtimes, { sessionId: 'pending-new:3', projectPath: pendingBotPath('bot-2') }), 'bot-2');
  assert.equal(botIdOfSession(runtimes, { sessionId: 'pending-new:4', projectPath: '/work/app' }), undefined);
  assert.equal(botIdOfSession(runtimes, undefined), undefined);
  assert.equal(isBotSession(runtimes, 's-bot'), true);
  assert.equal(isBotSession(runtimes, 's-project'), false);
  assert.equal(isBotSession(runtimes, 'unknown'), false);
});

test('a Bot is working while a chat of it has a turn, waiting while one needs an answer, idle otherwise', () => {
  const runtimes = botRuntimes([
    runtime('a1', 'a'),
    runtime('a2', 'a', { turnActive: true }),
    runtime('b1', 'b', { turnActive: true, pendingInteractions: 1 }),
    runtime('c1', 'c', { pendingAskUserQuestions: 2 }),
    runtime('p1', undefined, { turnActive: true }),
  ], () => undefined);
  assert.equal(runtimes.length, 4, 'a project\'s chat is not a Bot\'s');
  assert.equal(botActivity('a', runtimes), 'working');
  assert.equal(botActivity('b', runtimes), 'waiting');
  assert.equal(botActivity('c', runtimes), 'waiting');
  assert.equal(botActivity('nobody', runtimes), 'idle');
  assert.equal(botActivity('a', runtimes.filter((entry) => entry.turnActive === false)), 'idle');
});

test('what the window has heard of a chat since the host\'s summary wins over the summary', () => {
  const summaries = [runtime('a1', 'a'), runtime('b1', 'b', { turnActive: true })];
  const live = (sessionId: string) => sessionId === 'a1' ? { turnActive: true } : sessionId === 'b1' ? { turnActive: false, pendingInteractions: 1 } : undefined;
  const runtimes = botRuntimes(summaries, live);
  assert.equal(botActivity('a', runtimes), 'working');
  assert.equal(botActivity('b', runtimes), 'waiting');
});

test('last activity reads as a time ago, and a number that is no date reads as nothing', () => {
  const now = Date.UTC(2026, 0, 10, 12, 0, 0);
  assert.equal(relativeActivity(now - 30_000, now), 'just now');
  assert.equal(relativeActivity(now - 5 * 60_000, now), '5 minutes ago');
  assert.equal(relativeActivity(now - 3 * 3_600_000, now), '3 hours ago');
  assert.equal(relativeActivity(Number.NaN, now), '');
  assert.equal(relativeActivity(Number.POSITIVE_INFINITY, now), '');
  assert.equal(relativeActivity(8.64e15 + 1, now), '');
});

test('a chat that cannot start is reported in words, with the reason the host gave', async () => {
  assert.deepEqual(await startBotChat(async () => undefined), { ok: true });
  const refused = await startBotChat(async () => { throw new Error("Error invoking remote method 'lingxi:bots:sessions:open': Error: The Bot's folder is missing."); });
  assert.deepEqual(refused, { ok: false, message: "The Bot could not start the chat: The Bot's folder is missing." });
});

test('a Bot that cannot be deleted is reported in words, with the reason the host gave', async () => {
  assert.deepEqual(await removeBot(async () => undefined), { ok: true });
  const refused = await removeBot(async () => { throw new Error('The Bot folder could not be moved.'); });
  assert.deepEqual(refused, { ok: false, message: 'The Bot could not be deleted: The Bot folder could not be moved.' });
});
