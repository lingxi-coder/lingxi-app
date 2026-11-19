// A non-secret marker that this browser has a signed-in LingXi session. The
// landing page reads it before React (and Firebase) load, to send a returning
// person straight to Chat. It is a hint only: the workspace still resolves the
// real Firebase user and shows sign-in when there is none.
const key = 'lingxi.web.session';

type Store = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>;
const local = (): Store | null => { try { return typeof localStorage === 'undefined' ? null : localStorage; } catch { return null; } };

export function hasSessionHint(store: Store | null = local()): boolean {
  try { return store?.getItem(key) === '1'; } catch { return false; }
}

export function setSessionHint(signedIn: boolean, store: Store | null = local()): void {
  try { if (signedIn) store?.setItem(key, '1'); else store?.removeItem(key); } catch { /* Storage denied: no shortcut, nothing else changes. */ }
}

/** Where a fresh load of `pathname` should go before anything renders, or `null` to stay. `?home` keeps the landing page. */
export function landingRedirect(pathname: string, search: string, signedIn: boolean): string | null {
  if (pathname !== '/' || !signedIn) return null;
  const params = new URLSearchParams(search);
  if (params.has('home')) return null;
  return `/chat${search}`;
}
