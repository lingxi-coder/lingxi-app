import { startTransition, useEffect, useMemo, useState } from 'react';
import { docPageIds } from './docRoutes';

const docRoutes = new Set(docPageIds.map((id) => `/docs/${id}`));

export const appRoutes = [
  '/',
  '/download',
  '/pricing',
  '/login',
  '/admin/models',
  '/privacy',
  '/data-deletion',
  '/console',
  '/console/usage',
  '/console/api-keys',
  '/console/billing',
  '/docs',
  '/remote',
  '/login/remote',
  '/inbox',
  '/teams',
  '/teams/join',
  '/friends',
  '/friends/add',
  '/services',
  '/chat',
] as const;

export type AppRoute =
  | (typeof appRoutes)[number]
  | `/inbox/${string}`
  | `/chat/${string}`
  | `/open/${string}`
  | `/services/${string}`
  | `/remote/${string}/${string}`
  | `/remote/${string}/bots/${string}`
  | `/docs/${string}`;

export function normalizePath(path: string): string {
  const [pathname = '/'] = path.split(/[?#]/u);
  const withLeadingSlash = pathname.startsWith('/') ? pathname : `/${pathname}`;
  const collapsed = withLeadingSlash.replace(/\/{2,}/gu, '/');
  if (collapsed.length > 1 && collapsed.endsWith('/')) {
    return collapsed.slice(0, -1).toLowerCase();
  }
  return collapsed.toLowerCase();
}

/**
 * `/remote/<host_id>/<session_id>` names a session on one of the user's
 * computers. Both ids are case-sensitive (base64url and engine ids), so like
 * `/open/...` the path is kept as written; `null` for anything that is not
 * exactly two non-empty segments after `/remote/`.
 */
export function parseRemoteRoute(path: string): { hostId: string; sessionId: string } | null {
  const pathname = path.split(/[?#]/u)[0] ?? '';
  const match = /^\/remote\/([^/]+)\/([^/]+)\/?$/u.exec(pathname);
  if (!match) return null;
  try {
    return { hostId: decodeURIComponent(match[1]!), sessionId: decodeURIComponent(match[2]!) };
  } catch {
    return null;
  }
}

/** The route of one session; the inverse of {@link parseRemoteRoute}. */
export function remoteSessionRoute(hostId: string, sessionId: string): AppRoute {
  return `/remote/${encodeURIComponent(hostId)}/${encodeURIComponent(sessionId)}`;
}

/**
 * `/remote/<host_id>/bots/<bot_id>` names one Bot of one of the user's
 * computers. Kept as written, like the session route, and told apart from it by
 * its three segments: {@link parseRemoteRoute} does not match it.
 */
export function parseRemoteBotRoute(path: string): { hostId: string; botId: string } | null {
  const pathname = path.split(/[?#]/u)[0] ?? '';
  const match = /^\/remote\/([^/]+)\/bots\/([^/]+)\/?$/u.exec(pathname);
  if (!match) return null;
  try {
    return { hostId: decodeURIComponent(match[1]!), botId: decodeURIComponent(match[2]!) };
  } catch {
    return null;
  }
}

/** The route of one Bot; the inverse of {@link parseRemoteBotRoute}. */
export function remoteBotRoute(hostId: string, botId: string): AppRoute {
  return `/remote/${encodeURIComponent(hostId)}/bots/${encodeURIComponent(botId)}`;
}

/** Any Remote Control page (the computer list, one Bot, or one session). */
export function isRemoteRoute(route: string): boolean {
  return route === '/remote' || route.startsWith('/remote/');
}

/**
 * Pages whose security headers differ are separate documents: the Remote
 * Control pages get a strict Content-Security-Policy (see `firebase.json`) that
 * only applies to a document *loaded* from such a path. Moving between zones by
 * `pushState` would keep the old document's policy, so {@link navigateTo} loads
 * the new page instead.
 */
/** `/services/<service_id>` names a hosted Bot service. The id is case-sensitive, so the path is kept as written. */
export function parseServiceRoute(path: string): string | null {
  const match = /^\/services\/(s_[A-Za-z0-9_-]{6,62})\/?$/u.exec(path.split(/[?#]/u)[0] ?? '');
  return match ? match[1]! : null;
}

export type SecurityZone = 'remote' | 'sign-in' | 'site' | 'workspace';

const workspaceRoots = ['/chat', '/inbox', '/teams', '/friends', '/services'];

/** The signed-in workspace (Chat, Inbox, Teams, Friends, Services) shares one document policy, so moving inside it stays in-page. */
export function securityZone(route: string): SecurityZone {
  if (workspaceRoots.some((root) => route === root || route.startsWith(`${root}/`))) return 'workspace';
  if (isRemoteRoute(route)) return 'remote';
  return route === '/login/remote' ? 'sign-in' : 'site';
}

/** One place in the Chat workspace. Ids are case-sensitive, so the path is parsed as written. */
export type ChatTarget =
  | { kind: 'home' }
  | { kind: 'providers' }
  | { kind: 'agent'; id: string }
  | { kind: 'team'; teamId: string; conversationId: string }
  | { kind: 'friend'; uid: string }
  | { kind: 'bot'; teamId: string; botId: string };

const chatId = /^[A-Za-z0-9_:.-]{1,128}$/u;

/** `/chat`, `/chat/agent/<id>`, `/chat/team/<team>/<conversation>`, `/chat/friend/<uid>`, `/chat/bot/<team>/<bot>`, `/chat/settings/providers`; `null` otherwise. */
export function parseChatRoute(path: string): ChatTarget | null {
  const pathname = (path.split(/[?#]/u)[0] ?? '').replace(/\/$/u, '');
  if (pathname === '/chat') return { kind: 'home' };
  if (pathname === '/chat/settings/providers') return { kind: 'providers' };
  const parts = pathname.split('/').slice(2);
  let ids: string[];
  try { ids = parts.slice(1).map((part) => decodeURIComponent(part)); } catch { return null; }
  if (!ids.every((id) => chatId.test(id))) return null;
  const [first, second] = ids;
  if (parts[0] === 'agent' && ids.length === 1) return { kind: 'agent', id: first! };
  if (parts[0] === 'friend' && ids.length === 1) return { kind: 'friend', uid: first! };
  if (parts[0] === 'team' && ids.length === 2) return { kind: 'team', teamId: first!, conversationId: second! };
  if (parts[0] === 'bot' && ids.length === 2) return { kind: 'bot', teamId: first!, botId: second! };
  return null;
}

/** The route of one place in the Chat workspace; the inverse of {@link parseChatRoute}. */
export function chatRoute(target: ChatTarget): AppRoute {
  const e = encodeURIComponent;
  switch (target.kind) {
    case 'home': return '/chat';
    case 'providers': return '/chat/settings/providers';
    case 'agent': return `/chat/agent/${e(target.id)}`;
    case 'friend': return `/chat/friend/${e(target.uid)}`;
    case 'team': return `/chat/team/${e(target.teamId)}/${e(target.conversationId)}`;
    case 'bot': return `/chat/bot/${e(target.teamId)}/${e(target.botId)}`;
  }
}

export function resolveRoute(path: string): AppRoute {
  const chat = parseChatRoute(path);
  if (chat) return chatRoute(chat);
  if ((path.split(/[?#]/u)[0] ?? '').toLowerCase().startsWith('/chat/')) return '/chat';
  const inbox = /^\/inbox\/([^/?#]+)\/?$/u.exec(path.split(/[?#]/u)[0] ?? '');
  if(inbox) {
    try {
      const key=decodeURIComponent(inbox[1]!);
      if(/^(?:assistant|team|service):[A-Za-z0-9_-]{1,128}:[A-Za-z0-9_-]{1,128}$/u.test(key))return `/inbox/${encodeURIComponent(key)}`;
    } catch {return '/inbox';}
    return '/inbox';
  }
  if (path.split(/[?#]/u)[0]?.startsWith("/open/")) return path.split(/[?#]/u)[0] as AppRoute;
  if (parseRemoteRoute(path) || parseRemoteBotRoute(path)) return path.split(/[?#]/u)[0]!.replace(/\/$/u, '') as AppRoute;
  const serviceId = parseServiceRoute(path);
  if (serviceId) return `/services/${serviceId}`;
  const normalized = normalizePath(path);
  if ((appRoutes as readonly string[]).includes(normalized)) {
    return normalized as AppRoute;
  }
  if (docRoutes.has(normalized)) {
    return normalized as AppRoute;
  }
  return '/';
}

export function navigateTo(route: AppRoute): void {
  if (resolveRoute(window.location.pathname) === route) {
    return;
  }
  if (securityZone(resolveRoute(window.location.pathname)) !== securityZone(route)) {
    window.location.assign(route);
    return;
  }
  window.history.pushState({}, '', route);
  window.dispatchEvent(new PopStateEvent('popstate'));
}

export function useRouter(): AppRoute {
  const [route, setRoute] = useState<AppRoute>(() => resolveRoute(window.location.pathname));

  useEffect(() => {
    const handleRoute = () => {
      startTransition(() => {
        setRoute(resolveRoute(window.location.pathname));
      });
    };
    window.addEventListener('popstate', handleRoute);
    return () => window.removeEventListener('popstate', handleRoute);
  }, []);

  return useMemo(() => route, [route]);
}
