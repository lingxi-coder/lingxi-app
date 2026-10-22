import { isRemoteRoute } from '../app/router';

/** Where to go after signing in: only a Remote Control page of this site, never an arbitrary URL. */
export function safeNext(search: string, origin: string): string {
  const raw = new URLSearchParams(search).get('next');
  if (raw === null || !raw.startsWith('/') || raw.startsWith('//') || raw.includes('\\')) return '/remote';
  try {
    const url = new URL(raw, origin);
    if (url.origin !== origin || !isRemoteRoute(url.pathname)) return '/remote';
    return `${url.pathname}${url.search}`;
  } catch {
    return '/remote';
  }
}
