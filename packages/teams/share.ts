import { buildDeepLink } from '../deep-links/index.js';
import type { TeamConversation } from './messages.js';

export interface ChannelPostTarget { teamId: string; conversationId: string; postId: string }
export const channelPostPath = (team: string, channel: string, post: string) =>
  `/chat/team/${encodeURIComponent(team)}/${encodeURIComponent(channel)}/post/${encodeURIComponent(post)}` as const;
/** Web first, including on phones with an older app. Desktop can open the same post from the link landing page. */
export const postShareLink = (team: string, channel: string, post: string) =>
  `https://lingxios.net${channelPostPath(team, channel, post)}?via=link`;

/**
 * The link that opens a team conversation for anyone who can read it: in the LingXi app where it is installed, otherwise
 * on the web. A Bot's direct chat is shared as the Bot (`team/bot`), since each person has their own chat with it; a friend
 * chat is its pair team's conversation, so both friends can open it. `null` for a Bot chat without its Bot, or an id
 * a link cannot carry.
 */
export function conversationShareLink(teamId: string, conversation: Pick<TeamConversation, 'id' | 'kind' | 'participants'>): string | null {
  const bot = conversation.kind === 'bot_dm' ? conversation.participants.find(party => party.startsWith('shared_bot:'))?.slice('shared_bot:'.length) : undefined;
  if (conversation.kind === 'bot_dm' && !bot) return null;
  try {
    return buildDeepLink(bot ? { route: 'team/bot', params: { team: teamId, bot } } : { route: 'team/chat', params: { team: teamId, conversation: conversation.id } });
  } catch { return null; }
}
