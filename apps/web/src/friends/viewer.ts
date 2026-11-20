import type { FriendLink } from '../../../../packages/teams/friends';

/**
 * D17: where the Web viewer opens a friend invite or the request list. The code stays in the query only until the page
 * has kept it and stripped it. A friend chat link opens in the Chat workspace instead (`linkInBrowser`).
 */
export function friendViewerPath(link: Exclude<FriendLink, { kind: 'chat' }>): string {
  return link.kind === 'add' ? `/friends/add?code=${encodeURIComponent(link.code)}` : '/friends';
}
