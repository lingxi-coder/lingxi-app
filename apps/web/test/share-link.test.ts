import assert from 'node:assert/strict';
import test from 'node:test';
import { parseDeepLink } from '../../../packages/deep-links/index';
import { FRIEND_CHAT } from '../../../packages/teams/friends';
import { conversationShareLink } from '../../../packages/teams/share';

test('a team chat is shared by team and conversation; a Bot chat as the Bot; a friend chat as its pair conversation', () => {
  const chat = conversationShareLink('design-team', { id: '2f1c9a7e-55b1-4c1e-9d0a-6b7f2e3c4d5a', kind: 'group', participants: ['user:a', 'user:b'] });
  assert.equal(chat, 'https://lingxios.net/open/team/chat?conversation=2f1c9a7e-55b1-4c1e-9d0a-6b7f2e3c4d5a&team=design-team');
  assert.deepEqual(parseDeepLink(chat!).params, { team: 'design-team', conversation: '2f1c9a7e-55b1-4c1e-9d0a-6b7f2e3c4d5a' });
  assert.equal(conversationShareLink('design-team', { id: 'c-bot-1', kind: 'bot_dm', participants: ['user:a', 'shared_bot:helper_01'] }),
    'https://lingxios.net/open/team/bot?bot=helper_01&team=design-team');
  assert.equal(conversationShareLink('f_abcdef123456', { id: FRIEND_CHAT, kind: 'dm', participants: ['user:a', 'user:b'] }),
    'https://lingxios.net/open/team/chat?conversation=friend_chat&team=f_abcdef123456');
  assert.equal(conversationShareLink('design-team', { id: 'c1', kind: 'bot_dm', participants: ['user:a'] }), null);
  assert.equal(conversationShareLink('design team', { id: 'c1', kind: 'group', participants: [] }), null);
});
