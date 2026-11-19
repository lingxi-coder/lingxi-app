import { buildDeepLink, type DeepLinkTarget } from '../../../../packages/deep-links';
import { chatRoute, type ChatTarget } from '../app/router';

/** The query a shared link adds when it continues on the web, so the Chat workspace can offer the app instead. */
export const VIA_LINK = 'via=link';

/**
 * Where a shared link continues in this browser when the app did not take it: the conversation in the Chat workspace, or
 * a new Agent chat with the shared prompt in its composer (never sent). `null` keeps the "Open LingXi" page.
 */
export function openLinkTarget(target: DeepLinkTarget): string | null {
  const { team, conversation, bot, uid } = target.params;
  switch (target.route) {
    case 'team/chat': return chatRoute({ kind: 'team', teamId: team!, conversationId: conversation! });
    case 'team/bot': return chatRoute({ kind: 'bot', teamId: team!, botId: bot! });
    case 'friend/chat': return chatRoute({ kind: 'friend', uid: uid! });
    case 'new': {
      const prompt = typeof target.payload?.prompt === 'string' ? target.payload.prompt : '';
      return prompt ? `/chat#prompt=${encodeURIComponent(prompt)}` : '/chat';
    }
    default: return null;
  }
}

/** The app link for a place in the Chat workspace, for "Open in the LingXi app"; `null` where the app has no such place. */
export function appLinkForChat(target: ChatTarget): string | null {
  let link: DeepLinkTarget;
  switch (target.kind) {
    case 'team': link = { route: 'team/chat', params: { team: target.teamId, conversation: target.conversationId } }; break;
    case 'bot': link = { route: 'team/bot', params: { team: target.teamId, bot: target.botId } }; break;
    case 'friend': link = { route: 'friend/chat', params: { uid: target.uid } }; break;
    case 'home': link = { route: 'new', params: {} }; break;
    default: return null;
  }
  try { return buildDeepLink(link, 'scheme'); } catch { return null; }
}
