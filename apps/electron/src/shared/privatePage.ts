/**
 * Private pages on this computer.
 *
 * A page opens in a `<webview>` on an in-memory partition. Its URL is
 * `lingxi-page://page/<session>/<id>/<revision>`. The session's bridge serves
 * the document. There is no public URL and no file URL.
 */

/** Privileged scheme registered before `app.ready`. */
export const PRIVATE_PAGE_SCHEME = 'lingxi-page';

/** The only origin a private-page guest may load. */
export const PRIVATE_PAGE_ORIGIN = `${PRIVATE_PAGE_SCHEME}://page`;

/** In-memory partition (no `persist:` prefix). */
export const PRIVATE_PAGE_PARTITION = 'lingxi-page';

const SESSION_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const PAGE_ID = /^[A-Za-z0-9_-]{1,64}$/;

export interface PrivatePageTarget {
  readonly sessionId: string;
  readonly id: string;
  readonly revision: number;
}

/** The page URL for one session's bridge. Invalid parts return null. */
export function privatePageUrl(sessionId: string, id: string, revision: number): string | null {
  if (!SESSION_ID.test(sessionId) || !PAGE_ID.test(id) || !positiveRevision(revision)) return null;
  return `${PRIVATE_PAGE_ORIGIN}/${sessionId}/${id}/${revision}`;
}

/** Parse a private-page URL. Anything else, including a public URL, is null. */
export function parsePrivatePageUrl(raw: string): PrivatePageTarget | null {
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    return null;
  }
  if (url.protocol !== `${PRIVATE_PAGE_SCHEME}:` || url.host !== 'page') return null;
  if (url.username || url.password || url.port || url.search || url.hash) return null;
  const parts = url.pathname.split('/');
  if (parts.length !== 4 || parts[0] !== '') return null;
  const sessionId = parts[1] ?? '';
  const id = parts[2] ?? '';
  const revisionText = parts[3] ?? '';
  if (!SESSION_ID.test(sessionId) || !PAGE_ID.test(id)) return null;
  if (!/^[1-9][0-9]*$/.test(revisionText)) return null;
  const revision = Number(revisionText);
  if (!positiveRevision(revision)) return null;
  return { sessionId, id, revision };
}

function positiveRevision(revision: number): boolean {
  return Number.isSafeInteger(revision) && revision >= 1 && revision <= 0xffff_ffff;
}
