import type { FriendLink } from '../../../../packages/teams/friends';

/** D17: where the Web viewer opens a friend link. The code stays in the query only until the page has kept it and stripped it. */
export function friendViewerPath(link: FriendLink): string {
  if (link.kind === 'add') return `/friends/add?code=${encodeURIComponent(link.code)}`;
  return link.kind === 'requests' ? '/friends' : `/friends?chat=${encodeURIComponent(link.uid)}`;
}
