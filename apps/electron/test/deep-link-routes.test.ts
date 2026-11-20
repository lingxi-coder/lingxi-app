import assert from 'node:assert/strict';
import { test } from 'node:test';
import { settingsLinkRequest, runtimeLinkItem, terminalCommandInput, teamLinkRequest, teamLinkStep, pairLinkFriend } from '../src/renderer/deepLinkRoutes';
import { parseDeepLink } from '../../../packages/deep-links/index';
import { emptyRuntimeCenterState } from '../src/renderer/bridge/runtimeCenterState';

test('extension and import destinations carry identity, scope and preview payload', () => {
  assert.deepEqual(settingsLinkRequest({ route: 'extensions/mcp/LocalServer', params: { scope: 'project' } }, 'delivery'), {
    pageId: 'mcp', linkRequest: { requestId: 'delivery', kind: 'mcp', id: 'LocalServer', scope: 'project', payload: undefined },
  });
  assert.equal(settingsLinkRequest({ route: 'import/provider', params: {}, payload: { providers: {} } }, 'delivery')?.pageId, 'custom-providers');
  assert.throws(() => settingsLinkRequest({ route: 'import/skill', params: {} }, 'delivery'), /no configuration/);
});

test('runtime links resolve actual local identity and refuse unrelated or missing records', () => {
  const bridge = { runtimeCenter: { ...emptyRuntimeCenterState(), agents: { worker: { agent_id: 'worker' } }, plan: [{ id: 'todo', subject: 'Todo', state: 'pending' }] }, desktop: { tasks: { task: { task_id: 'task' } } }, conversation: { sessionKey: 'active-session', items: [{ type: 'meta', id: 'turn', files: [{ path: 'src/test.ts', additions: 1, removals: 0, diffs: [] }] }] } } as any;
  assert.deepEqual(runtimeLinkItem('agent', 'worker', bridge, []), { kind: 'agent', id: 'worker' });
  assert.deepEqual(runtimeLinkItem('task', 'task', bridge, []), { kind: 'task', id: 'task' });
  assert.deepEqual(runtimeLinkItem('plan', 'todo', bridge, []), { kind: 'plan', id: 'id:todo' });
  assert.deepEqual(runtimeLinkItem('plan', 'document', bridge, [{ id: 'document', content: '# Plan', status: 'submitted' }]), { kind: 'plan-document', id: 'document' });
  assert.equal(runtimeLinkItem('review', 'turn', bridge, []).kind, 'turn-review');
  assert.equal(runtimeLinkItem('activity', 'active-session', bridge, []), 'activity');
  assert.throws(() => runtimeLinkItem('activity', 'missing', bridge, []), /unavailable/);
  assert.throws(() => runtimeLinkItem('activity', 'current', bridge, []), /unavailable/);
  assert.throws(() => runtimeLinkItem('agent', 'missing', bridge, []), /unavailable/);
  assert.throws(() => runtimeLinkItem('review', 'missing', bridge, []), /unavailable/);
});

test('confirmed terminal commands preserve reviewed multiline input and reject control injection', () => {
  assert.equal(terminalCommandInput('echo first\r\necho second'), '\x1b[200~echo first\necho second\x1b[201~\r');
  assert.throws(() => terminalCommandInput('echo\x1b[201~bad'), /control characters/);
  assert.throws(() => terminalCommandInput(' \n '), /Enter a command/);
});

test('a shared team conversation opens in its own team only; a Bot opens the viewer\'s chat with it or the team\'s Bots, never a new chat', () => {
  const chat = teamLinkRequest(parseDeepLink('lingxi://team/chat?team=design&conversation=c1'))!;
  const bot = teamLinkRequest(parseDeepLink('lingxi://team/bot?team=design&bot=helper'))!;
  assert.deepEqual(chat, { kind: 'conversation', teamId: 'design', conversationId: 'c1' });
  assert.deepEqual(bot, { kind: 'bot', teamId: 'design', botId: 'helper' });
  assert.deepEqual(teamLinkRequest(parseDeepLink('lingxi://team/chat?team=f_pair&conversation=friend_chat')), { kind: 'pair', teamId: 'f_pair' });
  assert.equal(teamLinkRequest(parseDeepLink('lingxi://team/inbox')), null);
  if (chat.kind === 'pair' || bot.kind === 'pair') throw new Error('unexpected');

  const card = { sharedBotId: 'helper' } as any;
  const base = { loading: false, uid: 'me', selectedTeamId: 'other', botsLoaded: false, messagingConnecting: true, teams: [{ id: 'design' }, { id: 'other' }], bots: [], conversations: [] };
  assert.deepEqual(teamLinkStep(chat, { ...base, loading: true }), { step: 'wait' });
  assert.deepEqual(teamLinkStep(chat, { ...base, teams: [{ id: 'other' }] }), { step: 'unavailable' }, 'never another team');
  assert.deepEqual(teamLinkStep(chat, base), { step: 'open', teamId: 'design', conversationId: 'c1' });
  assert.deepEqual(teamLinkStep(bot, base), { step: 'team', teamId: 'design' }, 'a Bot link selects the team, keeping its remembered conversation');
  const selected = { ...base, selectedTeamId: 'design' };
  assert.deepEqual(teamLinkStep(chat, selected), { step: 'wait' }, 'waits for the team\'s conversations');
  const listed = { ...selected, messagingConnecting: false };
  assert.deepEqual(teamLinkStep(chat, listed), { step: 'unavailable' }, 'a conversation the viewer cannot see is not the team home');
  assert.deepEqual(teamLinkStep(chat, { ...listed, conversations: [{ id: 'c1' } as any] }), { step: 'select', conversationId: 'c1' });
  assert.deepEqual(teamLinkStep(bot, selected), { step: 'wait' }, 'waits for the Bots and the conversations');
  const ready = { ...selected, botsLoaded: true, messagingConnecting: false };
  assert.deepEqual(teamLinkStep(bot, ready), { step: 'unavailable' }, 'a Bot no longer shared');
  assert.deepEqual(teamLinkStep(bot, { ...ready, bots: [card] }), { step: 'bots' });
  const dm = { id: 'dm1', kind: 'bot_dm', participants: ['user:me', 'shared_bot:helper'] } as any;
  const theirs = { id: 'dm2', kind: 'bot_dm', participants: ['user:them', 'shared_bot:helper'] } as any;
  assert.deepEqual(teamLinkStep(bot, { ...ready, bots: [card], conversations: [theirs] }), { step: 'bots' }, 'someone else\'s chat with the Bot is not the viewer\'s');
  assert.deepEqual(teamLinkStep(bot, { ...ready, bots: [card], conversations: [theirs, dm] }), { step: 'select', conversationId: 'dm1' });

  assert.equal(pairLinkFriend('f_pair', undefined), undefined);
  assert.equal(pairLinkFriend('f_pair', { loading: true, cards: [] }), undefined);
  assert.equal(pairLinkFriend('f_pair', { loading: false, cards: [{ uid: 'b', state: 'friends', teamId: 'f_pair' }] }), 'b');
  assert.equal(pairLinkFriend('f_pair', { loading: false, cards: [{ uid: 'b', state: 'blocked', teamId: 'f_pair' }] }), null);
});
