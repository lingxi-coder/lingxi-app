import { buildDeepLink, type DeepLinkTarget } from '../../../../packages/deep-links';
import { channelInvitePath } from '../../../../packages/teams/channel-settings';
import { friendLink } from '../../../../packages/teams/friends';
import { chatRoute, remoteSessionRoute, type ChatTarget } from '../app/router';
import { friendViewerPath } from '../friends/viewer';

/** The query a shared link adds when it continues on the web, so the Chat workspace can offer the app instead. */
export const VIA_LINK = 'via=link';
const [VIA_KEY, VIA_VALUE] = VIA_LINK.split('=') as [string, string];

/** `/chat#prompt=…` becomes `/chat?via=link#prompt=…`. */
export function withViaLink(path: string): string {
  const at = path.indexOf('#');
  return at < 0 ? `${path}?${VIA_LINK}` : `${path.slice(0, at)}?${VIA_LINK}${path.slice(at)}`;
}

/** Whether this page was reached from a shared link that the app did not take. */
export const cameViaLink = (search: string): boolean => new URLSearchParams(search).get(VIA_KEY) === VIA_VALUE;
/** `search` without the shared-link marker (with its `?`, or empty). */
export function withoutViaLink(search: string): string {
  const query = new URLSearchParams(search);
  query.delete(VIA_KEY);
  const rest = query.toString();
  return rest ? `?${rest}` : '';
}

/**
 * Where a shared link goes in this browser when the app did not take it, the one table for every route:
 * - `now`: a conversation continues in the Chat workspace at once (a shared prompt goes into a new chat's composer, never
 *   sent), and the workspace offers the app;
 * - `offer`: the "Open LingXi" page stays, with this page as the browser alternative;
 * - `null`: the app only. A new-chat link bound to a project or mode is that: only the app can open it as it was meant.
 */
export function linkInBrowser(target: DeepLinkTarget): { path: string; at: 'now' | 'offer' } | null {
  const { team, conversation, bot, uid, code } = target.params;
  const now = (path: string) => ({ path, at: 'now' as const }), offer = (path: string) => ({ path, at: 'offer' as const });
  switch (target.route) {
    case 'team/chat': return now(chatRoute({ kind: 'team', teamId: team!, conversationId: conversation! }));
    case 'team/post': return now(chatRoute({ kind: 'team', teamId: team!, conversationId: conversation!, postId: target.params.post! }));
    case 'team/bot': return now(chatRoute({ kind: 'bot', teamId: team!, botId: bot! }));
    case 'friend/chat': return now(chatRoute({ kind: 'friend', uid: uid! }));
    case 'new': {
      if (Object.keys(target.params).length) return null;
      const prompt = typeof target.payload?.prompt === 'string' ? target.payload.prompt : '';
      return now(prompt ? `/chat#prompt=${encodeURIComponent(prompt)}` : '/chat');
    }
    // A full page load, so /remote starts under its own strict policy.
    case 'team/inbox': return offer('/teams');
    case 'team/join-channel': return offer(channelInvitePath({ teamId: team!, conversationId: conversation!, code: code! }));
    case 'team/join': return offer(`/teams/join?code=${encodeURIComponent(code!)}`);
    default: {
      const [kind, hostId, sessionId] = target.route.split('/');
      if (kind === 'remote' && hostId && sessionId) return offer(remoteSessionRoute(hostId, sessionId));
      const friend = friendLink(target);
      return friend && friend.kind !== 'chat' ? offer(friendViewerPath(friend)) : null;
    }
  }
}

/** The prompt a shared `new` link brought to `/chat#prompt=…`, if any. Reading it leaves the address as it is. */
export function sharedPrompt(hash: string): string | undefined {
  const match = /^#prompt=(.*)$/su.exec(hash);
  if (!match) return undefined;
  try { return decodeURIComponent(match[1]!).slice(0, 20_000) || undefined; } catch { return undefined; }
}

/**
 * The app link for a place in the Chat workspace, for "Open in the LingXi app"; `null` where the app has no such place. A
 * new chat carries the prompt its shared link brought, unless that no longer fits in a link.
 */
export function appLinkForChat(target: ChatTarget, prompt?: string): string | null {
  let link: DeepLinkTarget;
  switch (target.kind) {
    case 'team': link = target.postId
      ? { route: 'team/post', params: { team: target.teamId, conversation: target.conversationId, post: target.postId } }
      : { route: 'team/chat', params: { team: target.teamId, conversation: target.conversationId } }; break;
    case 'bot': link = { route: 'team/bot', params: { team: target.teamId, bot: target.botId } }; break;
    case 'friend': link = { route: 'friend/chat', params: { uid: target.uid } }; break;
    case 'home':
      link = { route: 'new', params: {} };
      if (prompt) try { return buildDeepLink({ ...link, payload: { prompt } }, 'scheme'); } catch { /* the app opens a new chat without it */ }
      break;
    default: return null;
  }
  try { return buildDeepLink(link, 'scheme'); } catch { return null; }
}
