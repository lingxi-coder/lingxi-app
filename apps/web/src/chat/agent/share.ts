import { buildDeepLink } from '../../../../../packages/deep-links';
import type { AgentChat } from './history';

/** The longest question a shared link carries; a longer one is cut, since the link is for starting a chat, not archiving it. */
export const SHARED_PROMPT_LIMIT = 2000;

/**
 * An Agent chat's link: a new chat, in the app or on the web, with the chat's first question in the composer and nothing sent.
 * Agent chats stay in the browser they were made in, so the answers are never part of it. `null` before a question was asked.
 */
export function agentShareLink(chat: Pick<AgentChat, 'messages'>): string | null {
  const prompt = chat.messages.find(message => message.role === 'user')?.content.trim();
  if (!prompt) return null;
  try { return buildDeepLink({ route: 'new', params: {}, payload: { prompt: [...prompt].slice(0, SHARED_PROMPT_LIMIT).join('') } }); } catch { return null; }
}
