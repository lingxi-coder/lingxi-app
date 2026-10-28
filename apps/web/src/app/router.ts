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
] as const;

export type AppRoute =
  | (typeof appRoutes)[number]
  | `/open/${string}`
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
export type SecurityZone = 'remote' | 'sign-in' | 'site';

export function securityZone(route: string): SecurityZone {
  if (isRemoteRoute(route)) return 'remote';
  return route === '/login/remote' ? 'sign-in' : 'site';
}

export function resolveRoute(path: string): AppRoute {
  if (path.split(/[?#]/u)[0]?.startsWith("/open/")) return path.split(/[?#]/u)[0] as AppRoute;
  if (parseRemoteRoute(path) || parseRemoteBotRoute(path)) return path.split(/[?#]/u)[0]!.replace(/\/$/u, '') as AppRoute;
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
