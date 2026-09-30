import { startTransition, useEffect, useMemo, useState } from 'react';
import { docPages } from '../data/docs';

export const appRoutes = [
  '/',
  '/download',
  '/pricing',
  '/login',
  '/console',
  '/console/usage',
  '/console/api-keys',
  '/console/billing',
  '/docs',
] as const;

export type AppRoute = (typeof appRoutes)[number] | `/docs/${string}`;

export function normalizePath(path: string): string {
  const [pathname = '/'] = path.split(/[?#]/u);
  const withLeadingSlash = pathname.startsWith('/') ? pathname : `/${pathname}`;
  const collapsed = withLeadingSlash.replace(/\/{2,}/gu, '/');
  if (collapsed.length > 1 && collapsed.endsWith('/')) {
    return collapsed.slice(0, -1).toLowerCase();
  }
  return collapsed.toLowerCase();
}

export function resolveRoute(path: string): AppRoute {
  const normalized = normalizePath(path);
  if ((appRoutes as readonly string[]).includes(normalized)) {
    return normalized as AppRoute;
  }
  if (docPages.some((page) => normalized === `/docs/${page.id}`)) {
    return normalized as AppRoute;
  }
  return '/';
}

export function navigateTo(route: AppRoute): void {
  if (resolveRoute(window.location.pathname) === route) {
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
