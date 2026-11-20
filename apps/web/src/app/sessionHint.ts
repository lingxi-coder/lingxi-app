// A non-secret marker that this browser has a signed-in LingXi session. The
// inline script in index.html reads it before any module loads, to send a
// returning person from the landing page straight to Chat (not for `?home` or a
// `#…` anchor). It is a hint only: the workspace still resolves the real
// Firebase user and shows sign-in when there is none. `app/firebase.ts` keeps it
// in step with every sign-in and sign-out, on any page.
export const SESSION_HINT_KEY = 'lingxi.web.session';

type Store = Pick<Storage, 'setItem' | 'removeItem'>;
const local = (): Store | null => { try { return typeof localStorage === 'undefined' ? null : localStorage; } catch { return null; } };

export function setSessionHint(signedIn: boolean, store: Store | null = local()): void {
  try { if (signedIn) store?.setItem(SESSION_HINT_KEY, '1'); else store?.removeItem(SESSION_HINT_KEY); } catch { /* Storage denied: no shortcut, nothing else changes. */ }
}
